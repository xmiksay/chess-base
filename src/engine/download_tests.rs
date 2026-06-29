//! Tests for [`super`] (engine binary download). Split out to keep the
//! module under the project's 500-line file cap.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// In-memory [`Fetch`] mapping URL → body, counting hits so idempotency is
/// observable. Returns an error for any URL not registered.
#[derive(Clone, Default)]
struct FakeFetcher {
    files: HashMap<String, Vec<u8>>,
    hits: Arc<AtomicUsize>,
}

impl FakeFetcher {
    fn with(url: &str, body: &[u8]) -> Self {
        let mut files = HashMap::new();
        files.insert(url.to_string(), body.to_vec());
        Self {
            files,
            hits: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Fetch for FakeFetcher {
    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        self.files
            .get(url)
            .cloned()
            .with_context(|| format!("no fake body for {url}"))
    }
}

fn tmpdir() -> PathBuf {
    // Unique per test process+name; cleaned by the OS tmp reaper. Avoids a
    // dev-dependency on `tempfile` for these small fixtures.
    let mut dir = std::env::temp_dir();
    dir.push(format!("chess-base-dl-{}", uuid::Uuid::new_v4()));
    dir
}

fn asset(url: &str, body: &[u8], dest: &str, exec: bool) -> Asset {
    Asset {
        url: url.to_string(),
        sha256: Some(sha256_hex(body)),
        dest: dest.to_string(),
        executable: exec,
    }
}

#[test]
fn sha256_matches_known_vector() {
    // SHA-256("abc"), the canonical NIST test vector.
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn verify_accepts_match_and_is_case_insensitive() {
    assert!(verify_checksum(b"abc", &sha256_hex(b"abc").to_uppercase()).is_ok());
}

#[test]
fn verify_rejects_mismatch() {
    assert!(verify_checksum(b"abc", &sha256_hex(b"xyz")).is_err());
}

#[tokio::test]
async fn first_run_installs_and_registers_engines() {
    let dir = tmpdir();
    let body = b"#!/bin/sh\necho stockfish";
    let fetch = FakeFetcher::with("http://x/sf", body);
    let plan = Plan {
        assets: vec![asset("http://x/sf", body, "stockfish", true)],
        engines: vec![PlannedEngine {
            name: "Stockfish".into(),
            binary: "stockfish".into(),
            weights: None,
        }],
    };

    let configs = Manager::new(&dir, fetch).ensure(&plan).await.unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].name, "Stockfish");
    assert_eq!(configs[0].path, dir.join("stockfish"));
    assert_eq!(tokio::fs::read(dir.join("stockfish")).await.unwrap(), body);
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[tokio::test]
async fn maia_config_carries_binary_and_weights() {
    let dir = tmpdir();
    let lc0 = b"lc0-binary";
    let weights = b"maia-net";
    let mut files = HashMap::new();
    files.insert("http://x/lc0".to_string(), lc0.to_vec());
    files.insert("http://x/maia".to_string(), weights.to_vec());
    let fetch = FakeFetcher {
        files,
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let plan = Plan {
        assets: vec![
            asset("http://x/lc0", lc0, "lc0", true),
            asset("http://x/maia", weights, "maia-1100.pb.gz", false),
        ],
        engines: vec![PlannedEngine {
            name: "Maia 1100".into(),
            binary: "lc0".into(),
            weights: Some("maia-1100.pb.gz".into()),
        }],
    };

    let configs = Manager::new(&dir, fetch).ensure(&plan).await.unwrap();
    assert_eq!(configs[0].path, dir.join("lc0"));
    assert_eq!(configs[0].weights, Some(dir.join("maia-1100.pb.gz")));
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[tokio::test]
async fn checksum_mismatch_is_rejected_and_nothing_installed() {
    let dir = tmpdir();
    let fetch = FakeFetcher::with("http://x/sf", b"the-wrong-bytes");
    let bad = Asset {
        url: "http://x/sf".into(),
        sha256: Some(sha256_hex(b"the-expected-bytes")),
        dest: "stockfish".into(),
        executable: true,
    };
    let plan = Plan {
        assets: vec![bad],
        engines: vec![],
    };

    let err = Manager::new(&dir, fetch).ensure(&plan).await.unwrap_err();
    assert!(format!("{err:#}").contains("checksum mismatch"));
    // The bad download must not have been installed.
    assert!(!dir.join("stockfish").exists());
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[tokio::test]
async fn rerun_is_idempotent_and_skips_refetch() {
    let dir = tmpdir();
    let body = b"engine-bytes";
    let fetch = FakeFetcher::with("http://x/sf", body);
    let plan = Plan {
        assets: vec![asset("http://x/sf", body, "stockfish", true)],
        engines: vec![PlannedEngine {
            name: "Stockfish".into(),
            binary: "stockfish".into(),
            weights: None,
        }],
    };

    let mgr = Manager::new(&dir, fetch.clone());
    mgr.ensure(&plan).await.unwrap();
    mgr.ensure(&plan).await.unwrap();
    // Second run found the checksum-matching file and did not re-download.
    assert_eq!(fetch.hits(), 1);
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[tokio::test]
async fn corrupt_existing_file_is_redownloaded() {
    let dir = tmpdir();
    let body = b"good-engine";
    let fetch = FakeFetcher::with("http://x/sf", body);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    // A stale/corrupt file whose checksum will not match the expected one.
    tokio::fs::write(dir.join("stockfish"), b"corrupt")
        .await
        .unwrap();
    let plan = Plan {
        assets: vec![asset("http://x/sf", body, "stockfish", true)],
        engines: vec![],
    };

    Manager::new(&dir, fetch.clone())
        .ensure(&plan)
        .await
        .unwrap();
    assert_eq!(fetch.hits(), 1);
    assert_eq!(tokio::fs::read(dir.join("stockfish")).await.unwrap(), body);
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[tokio::test]
async fn fetch_failure_is_reported_not_panicked() {
    let dir = tmpdir();
    let fetch = FakeFetcher::default(); // knows no URLs
    let plan = Plan {
        assets: vec![asset("http://x/missing", b"x", "stockfish", true)],
        engines: vec![],
    };
    assert!(Manager::new(&dir, fetch).ensure(&plan).await.is_err());
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[cfg(unix)]
#[tokio::test]
async fn installed_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmpdir();
    let body = b"bin";
    let fetch = FakeFetcher::with("http://x/sf", body);
    let plan = Plan {
        assets: vec![asset("http://x/sf", body, "stockfish", true)],
        engines: vec![],
    };
    Manager::new(&dir, fetch).ensure(&plan).await.unwrap();
    let mode = tokio::fs::metadata(dir.join("stockfish"))
        .await
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "binary should be executable");
    tokio::fs::remove_dir_all(&dir).await.ok();
}

#[test]
fn catalog_offers_stockfish_and_maia_on_linux_x86_64() {
    let plan = catalog(&Platform {
        os: "linux",
        arch: "x86_64",
    })
    .expect("linux/x86_64 is catalogued");
    let names: Vec<_> = plan.engines.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Stockfish"));
    assert!(names.contains(&"Maia 1100"));
    // The Maia entry must reference weights.
    let maia = plan.engines.iter().find(|e| e.name == "Maia 1100").unwrap();
    assert!(maia.weights.is_some());
}

#[test]
fn catalog_is_none_for_unknown_platform() {
    assert!(catalog(&Platform {
        os: "plan9",
        arch: "sparc",
    })
    .is_none());
}
