//! Engine auto-download manager (ADR 0005, issue #11).
//!
//! Detects the host platform, downloads the Stockfish / Lc0 binaries and Maia
//! weights for it into an `engines/` directory, verifies each file against a
//! published SHA-256, and turns the result into runnable [`EngineConfig`]s. The
//! network boundary is the [`Fetch`] trait so the manager is unit-tested with a
//! synthetic fetcher — no real downloads run in the test suite (mirroring the
//! LLM `Transport` seam).
//!
//! Behaviour contract (issue #11):
//! - first run populates `engines/` and yields `EngineConfig`s (Stockfish + Maia);
//! - a checksum mismatch is **rejected** — the file is not installed or registered;
//! - re-running is **idempotent** — present, checksum-matching files are not re-fetched;
//! - every failure is an `Err`, never a panic.
//!
//! The pure pieces ([`catalog`], [`verify_checksum`], [`sha256_hex`],
//! [`Plan`]→[`EngineConfig`] mapping) are unit-tested directly; only [`Manager`]
//! touches the disk and the network.
//!
//! ## Catalog scope
//!
//! [`catalog`] is the maintained per-platform data surface. Entries point at the
//! upstream **direct-download** asset for the binary/weights and carry the
//! published SHA-256. Maia weights are distributed as `.pb.gz`, which Lc0 reads
//! natively, so no decompression is needed. Pinning exact release URLs +
//! checksums (and any archive hosting) is ongoing maintenance; assets with an
//! unknown digest (`sha256: None`) are accepted unverified and logged.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::EngineConfig;

/// Host platform = OS + CPU architecture, named as Rust's `cfg` constants do
/// (`linux`/`macos`/`windows`, `x86_64`/`aarch64`). Drives catalog selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Platform {
    /// The platform this binary is running on.
    pub fn detect() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }
}

/// One file to fetch: where from, what it must hash to, where it lands under the
/// engines dir, and whether it needs the executable bit (binaries do, weights
/// don't).
#[derive(Debug, Clone)]
pub struct Asset {
    pub url: String,
    /// Expected lowercase-hex SHA-256. `None` when upstream publishes no digest
    /// for the asset — the file is then accepted unverified (and logged).
    pub sha256: Option<String>,
    /// Destination path **relative to** the engines directory.
    pub dest: String,
    /// Mark the installed file executable (Unix `0o755`). Engine binaries only.
    pub executable: bool,
}

/// An [`EngineConfig`] described by the catalog in terms of engines-dir-relative
/// destinations; resolved to absolute paths once its assets are on disk.
#[derive(Debug, Clone)]
pub struct PlannedEngine {
    pub name: String,
    /// Engines-dir-relative path of the binary asset.
    pub binary: String,
    /// Engines-dir-relative path of the weights asset (Lc0/Maia), if any.
    pub weights: Option<String>,
}

/// Everything to fetch for one platform plus the engines it yields once present.
/// `assets` is deduplicated, so a binary shared by several engines (the Lc0
/// binary backing every Maia level) is downloaded once.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub assets: Vec<Asset>,
    pub engines: Vec<PlannedEngine>,
}

/// The network seam: fetch the full body at `url`. The production
/// implementation is [`HttpFetcher`]; tests inject a synthetic one.
#[async_trait]
pub trait Fetch: Send + Sync {
    async fn get(&self, url: &str) -> Result<Vec<u8>>;
}

/// `reqwest`-backed [`Fetch`]: a GET, status check, and full-body read.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetch for HttpFetcher {
    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?
            .error_for_status()
            .with_context(|| format!("downloading {url}"))?;
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("reading body from {url}"))?;
        Ok(bytes.to_vec())
    }
}

/// Downloads a [`Plan`] into a directory and registers the result. Generic over
/// the [`Fetch`] seam so the disk-and-verify logic is exercised without a real
/// network in tests.
pub struct Manager<F: Fetch> {
    dir: PathBuf,
    fetch: F,
}

impl<F: Fetch> Manager<F> {
    /// A manager that installs assets under `dir` using `fetch`.
    pub fn new(dir: impl Into<PathBuf>, fetch: F) -> Self {
        Self {
            dir: dir.into(),
            fetch,
        }
    }

    /// Ensure every asset in `plan` is present and verified under the engines
    /// dir, then return the runnable [`EngineConfig`]s. Idempotent: an asset
    /// already on disk with a matching checksum is left untouched.
    pub async fn ensure(&self, plan: &Plan) -> Result<Vec<EngineConfig>> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("creating engines dir {}", self.dir.display()))?;
        for asset in &plan.assets {
            self.ensure_asset(asset)
                .await
                .with_context(|| format!("installing engine asset {}", asset.dest))?;
        }
        Ok(plan.engines.iter().map(|e| self.to_config(e)).collect())
    }

    /// Fetch, verify and install a single asset unless it is already present.
    async fn ensure_asset(&self, asset: &Asset) -> Result<()> {
        let path = self.dir.join(&asset.dest);
        if self.already_present(&path, asset).await? {
            tracing::debug!(asset = %asset.dest, "engine asset already present, skipping");
            return Ok(());
        }
        let bytes = self.fetch.get(&asset.url).await?;
        match &asset.sha256 {
            Some(expected) => verify_checksum(&bytes, expected)
                .with_context(|| format!("verifying {}", asset.dest))?,
            None => tracing::warn!(
                asset = %asset.dest,
                "no published checksum; installing engine asset unverified"
            ),
        }
        self.write_file(&path, &bytes, asset.executable).await?;
        tracing::info!(asset = %asset.dest, "downloaded engine asset");
        Ok(())
    }

    /// True when the file is already on disk and (if a checksum is known) still
    /// matches it — the idempotent fast path that skips re-downloading.
    async fn already_present(&self, path: &Path, asset: &Asset) -> Result<bool> {
        match tokio::fs::read(path).await {
            Ok(bytes) => match &asset.sha256 {
                Some(expected) => Ok(verify_checksum(&bytes, expected).is_ok()),
                None => Ok(true),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write `bytes` to `path` via a temp sibling + rename, so a crash mid-write
    /// never leaves a half-written binary that a later existence check accepts.
    async fn write_file(&self, path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
        let tmp = path.with_extension("partial");
        tokio::fs::write(&tmp, bytes)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        if executable {
            set_executable(&tmp).await?;
        }
        tokio::fs::rename(&tmp, path)
            .await
            .with_context(|| format!("installing {}", path.display()))?;
        Ok(())
    }

    /// Resolve a catalog [`PlannedEngine`] to an absolute-pathed [`EngineConfig`].
    fn to_config(&self, engine: &PlannedEngine) -> EngineConfig {
        let mut cfg = EngineConfig::new(engine.name.clone(), self.dir.join(&engine.binary));
        if let Some(weights) = &engine.weights {
            cfg = cfg.with_weights(self.dir.join(weights));
        }
        cfg
    }
}

/// Convenience for the startup path: detect the platform, look up its catalog
/// and install it under `dir` over real HTTP. Returns an empty vec when the
/// platform has no catalog entry (so the caller just falls back, no error).
pub async fn download_default_engines(dir: impl Into<PathBuf>) -> Result<Vec<EngineConfig>> {
    let platform = Platform::detect();
    let Some(plan) = catalog(&platform) else {
        tracing::info!(
            os = platform.os,
            arch = platform.arch,
            "no engine catalog for platform"
        );
        return Ok(Vec::new());
    };
    Manager::new(dir, HttpFetcher::new()).ensure(&plan).await
}

/// Lowercase-hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        // Writing to a String is infallible.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Verify `bytes` hash to `expected` (case-insensitive hex). A mismatch is an
/// `Err`, so the caller never installs an unverified file.
pub fn verify_checksum(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        bail!("checksum mismatch: expected {expected}, got {actual}");
    }
}

/// The download plan for `platform`, or `None` when no assets are catalogued for
/// it. The single maintained mapping of platform → upstream assets.
pub fn catalog(platform: &Platform) -> Option<Plan> {
    let stockfish = stockfish_asset(platform)?;
    let mut assets = vec![stockfish.clone()];
    let mut engines = vec![PlannedEngine {
        name: "Stockfish".to_string(),
        binary: stockfish.dest,
        weights: None,
    }];

    // Maia = the Lc0 binary + a Maia weights file. Offered only where both the
    // Lc0 binary and the weights are catalogued for the platform.
    if let Some((lc0, maia)) = maia_assets(platform) {
        engines.push(PlannedEngine {
            name: "Maia 1100".to_string(),
            binary: lc0.dest.clone(),
            weights: Some(maia.dest.clone()),
        });
        assets.push(lc0);
        assets.push(maia);
    }

    Some(Plan { assets, engines })
}

/// Stockfish 16.1 binary asset for `platform`, if catalogued. URLs point at the
/// official release; checksums are pinned per release (`None` until filled).
fn stockfish_asset(platform: &Platform) -> Option<Asset> {
    const BASE: &str = "https://github.com/official-stockfish/Stockfish/releases/download/sf_16.1";
    let (slug, dest, executable) = match (platform.os, platform.arch) {
        ("linux", "x86_64") => ("stockfish-ubuntu-x86-64-avx2", "stockfish", true),
        ("linux", "aarch64") => ("stockfish-android-armv8", "stockfish", true),
        ("macos", "x86_64") => ("stockfish-macos-x86-64-avx2", "stockfish", true),
        ("macos", "aarch64") => ("stockfish-macos-m1-apple-silicon", "stockfish", true),
        ("windows", "x86_64") => ("stockfish-windows-x86-64-avx2", "stockfish.exe", false),
        _ => return None,
    };
    Some(Asset {
        url: format!("{BASE}/{slug}"),
        sha256: None,
        dest: dest.to_string(),
        executable,
    })
}

/// The Lc0 binary + Maia-1100 weights for `platform`, if both are catalogued.
/// Maia weights ship as a direct `.pb.gz` Lc0 reads natively (no extraction).
fn maia_assets(platform: &Platform) -> Option<(Asset, Asset)> {
    const MAIA_WEIGHTS: &str =
        "https://github.com/CSSLab/maia-chess/raw/master/maia_weights/maia-1100.pb.gz";
    const LC0_BASE: &str = "https://github.com/LeelaChessZero/lc0/releases/download/v0.31.2";
    let (slug, dest, executable) = match (platform.os, platform.arch) {
        ("linux", "x86_64") => ("lc0", "lc0", true),
        ("windows", "x86_64") => ("lc0.exe", "lc0.exe", false),
        // Lc0 publishes no prebuilt binary we install directly for other
        // targets; Stockfish still works, Maia is simply unavailable there.
        _ => return None,
    };
    let lc0 = Asset {
        url: format!("{LC0_BASE}/{slug}"),
        sha256: None,
        dest: dest.to_string(),
        executable,
    };
    let maia = Asset {
        url: MAIA_WEIGHTS.to_string(),
        sha256: None,
        dest: "maia-1100.pb.gz".to_string(),
        executable: false,
    };
    Some((lc0, maia))
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(path, perms)
        .await
        .with_context(|| format!("chmod {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
