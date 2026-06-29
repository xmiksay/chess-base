//! Build script.
//!
//! 1. Ensures `frontend/dist` exists at compile time so `rust-embed` always
//!    compiles, even before the Vue app has been built. The real assets are
//!    produced by `make frontend` (vite build) and embedded in release builds.
//!
//! 2. When the opt-in `bundled-stockfish` feature is on (issue #54), ensures the
//!    per-target Stockfish binary is present under `engines-bundled/<target>/`
//!    and **checksum-verifies it at build time** — a mismatch fails the build
//!    rather than surfacing as a runtime surprise. The binary itself is fetched
//!    by `make bundle-stockfish` (kept out of the build script so the feature
//!    build stays offline-capable and pulls no HTTP stack into the toolchain).
//!
//!    LICENSING: Stockfish is GPLv3, so enabling this feature makes the build
//!    artifact GPLv3. The default download build embeds nothing and is unaffected.

use std::path::Path;

fn main() {
    ensure_frontend_dist();

    // `CARGO_FEATURE_<NAME>` is set by Cargo when the feature is active.
    if std::env::var_os("CARGO_FEATURE_BUNDLED_STOCKFISH").is_some() {
        ensure_bundled_stockfish();
    }
}

/// Guarantee `frontend/dist` (with an index) exists so the SPA `rust-embed`
/// folder is never empty before `make frontend` runs.
fn ensure_frontend_dist() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
    }
    let index = dist.join("index.html");
    if !index.exists() {
        let _ = std::fs::write(
            &index,
            "<!doctype html><meta charset=utf-8><title>chess-base</title>\
             <body>Frontend not built. Run <code>make frontend</code>.</body>",
        );
    }
    println!("cargo:rerun-if-changed=frontend/dist");
}

/// Verify the embedded Stockfish binary is present and matches its pinned
/// checksum, failing the build otherwise. Runs only under `bundled-stockfish`.
fn ensure_bundled_stockfish() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("engines-bundled")
        .join(&target);
    let binary = if target.contains("windows") {
        "stockfish.exe"
    } else {
        "stockfish"
    };
    let path = dir.join(binary);

    println!("cargo:rerun-if-changed=engines-bundled/{target}");
    println!("cargo:rerun-if-env-changed=CHESS_BASE_BUNDLED_STOCKFISH_SHA256");

    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "bundled-stockfish: missing engine binary {}: {e}.\n\
             Run `make bundle-stockfish` to fetch it (needs network once), or place a\n\
             Stockfish binary there manually for an offline build. Stockfish is GPLv3 —\n\
             enabling this feature makes the build artifact GPLv3 (see the build docs).",
            path.display()
        )
    });

    match expected_sha256(&dir, binary) {
        Some(expected) => {
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected.trim()) {
                panic!(
                    "bundled-stockfish: checksum mismatch for {}\n  expected {expected}\n  got      {actual}",
                    path.display()
                );
            }
            println!("cargo:warning=bundled-stockfish: embedding checksum-verified {binary} for {target}");
        }
        // Mirrors the auto-download catalog policy (download.rs `sha256: None`):
        // pinning per-release digests is ongoing maintenance; until one exists the
        // binary is embedded unverified and the build warns.
        None => println!(
            "cargo:warning=bundled-stockfish: no pinned checksum for {binary} ({target}); embedding unverified"
        ),
    }
}

/// The expected checksum for the bundled binary: the `CHESS_BASE_BUNDLED_STOCKFISH_SHA256`
/// env var (overrides), else a `<binary>.sha256` sidecar `make bundle-stockfish`
/// writes, else `None` (unverified).
fn expected_sha256(dir: &Path, binary: &str) -> Option<String> {
    if let Ok(env) = std::env::var("CHESS_BASE_BUNDLED_STOCKFISH_SHA256") {
        if !env.trim().is_empty() {
            return Some(env);
        }
    }
    let sidecar = dir.join(format!("{binary}.sha256"));
    let contents = std::fs::read_to_string(sidecar).ok()?;
    contents.split_whitespace().next().map(str::to_string)
}

/// Lowercase-hex SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
