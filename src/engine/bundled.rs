//! Optional embedded Stockfish (`bundled-stockfish` feature, ADR 0005 amendment,
//! issue #54).
//!
//! When the opt-in `bundled-stockfish` Cargo feature is on, `build.rs` ensures a
//! per-target Stockfish binary sits under `engines-bundled/<target>/` and
//! [`EngineAssets`] embeds it into the binary via `rust-embed` (mirroring the
//! frontend embedding of ADR 0004). At startup [`extract`] writes that binary to
//! the OS cache dir, sets the executable bit, and returns a runnable
//! [`EngineConfig`]; [`bundled_engine`] is the pure resolution seam the registry
//! consults (the embedded build slots in below a user override and above an
//! auto-downloaded binary — see `registry::resolve`).
//!
//! In the **default** build (feature off) both functions are no-ops returning
//! `None`/`Ok(None)`, nothing is embedded, and there is no GPLv3 obligation.
//!
//! Stockfish is **GPLv3**: enabling this feature makes *that build artifact*
//! GPLv3. See the build docs (`README` / `docs/decisions/0005-*`).

use crate::engine::EngineConfig;

/// Registry name (and default selector key) of the embedded engine.
pub const BUNDLED_ENGINE_NAME: &str = "Stockfish (bundled)";

#[cfg(feature = "bundled-stockfish")]
mod imp {
    use super::*;
    use anyhow::{Context, Result};
    use rust_embed::RustEmbed;
    use std::path::{Path, PathBuf};

    /// The per-target Stockfish binary embedded at compile time. `build.rs`
    /// guarantees `engines-bundled/<target>/` holds exactly the binary for this
    /// target, so the embedded set contains a single engine.
    #[derive(RustEmbed)]
    #[folder = "engines-bundled"]
    struct EngineAssets;

    /// Executable name for this target (Windows binaries carry `.exe`).
    fn binary_name() -> &'static str {
        if cfg!(windows) {
            "stockfish.exe"
        } else {
            "stockfish"
        }
    }

    /// The embedded Stockfish asset for this target as (embedded-path, bytes).
    /// `None` when nothing was embedded (e.g. the folder was empty at build).
    fn embedded() -> Option<(String, std::borrow::Cow<'static, [u8]>)> {
        let name = binary_name();
        let path = EngineAssets::iter().find(|p| p.rsplit('/').next() == Some(name))?;
        let file = EngineAssets::get(&path)?;
        Some((path.into_owned(), file.data))
    }

    /// Deterministic extraction target: `<cache>/chess-base/engines-bundled/<bin>`.
    fn dest_path() -> Option<PathBuf> {
        Some(
            dirs::cache_dir()?
                .join("chess-base")
                .join("engines-bundled")
                .join(binary_name()),
        )
    }

    /// Pure resolution seam: the bundled engine's config, or `None` when no
    /// binary was embedded / no cache dir is available. The path is deterministic
    /// and matches what [`extract`] writes, so it is valid once extraction ran.
    pub fn bundled_engine() -> Option<EngineConfig> {
        embedded()?;
        Some(EngineConfig::new(BUNDLED_ENGINE_NAME, dest_path()?))
    }

    /// Extract the embedded binary to the OS cache dir (idempotent), mark it
    /// executable, and return its [`EngineConfig`]. Re-extraction is skipped when
    /// the on-disk file already matches the embedded bytes, so a downgraded build
    /// still refreshes a stale cached binary.
    pub fn extract() -> Result<Option<EngineConfig>> {
        let Some((src, data)) = embedded() else {
            return Ok(None);
        };
        let dest =
            dest_path().context("no OS cache directory available to extract bundled engine")?;
        let parent = dest
            .parent()
            .context("bundled engine destination has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        if !up_to_date(&dest, &data)? {
            write_executable(&dest, &data)
                .with_context(|| format!("extracting bundled engine {src}"))?;
            tracing::info!(path = %dest.display(), "extracted bundled Stockfish");
        }
        Ok(Some(EngineConfig::new(BUNDLED_ENGINE_NAME, dest)))
    }

    /// True when `dest` already holds exactly `data` — the idempotent fast path.
    fn up_to_date(dest: &Path, data: &[u8]) -> Result<bool> {
        match std::fs::read(dest) {
            Ok(existing) => Ok(existing == data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("reading {}", dest.display())),
        }
    }

    /// Write `data` via a temp sibling + rename so a crash mid-write never leaves
    /// a half-written binary a later `up_to_date` check would skip over.
    fn write_executable(dest: &Path, data: &[u8]) -> Result<()> {
        let tmp = dest.with_extension("partial");
        std::fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
        set_executable(&tmp)?;
        std::fs::rename(&tmp, dest).with_context(|| format!("installing {}", dest.display()))?;
        Ok(())
    }

    #[cfg(unix)]
    fn set_executable(path: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .with_context(|| format!("stat {}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("chmod {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn set_executable(_path: &Path) -> Result<()> {
        Ok(())
    }
}

#[cfg(feature = "bundled-stockfish")]
pub use imp::{bundled_engine, extract};

/// Resolution seam in the default (feature-off) build: nothing is embedded.
#[cfg(not(feature = "bundled-stockfish"))]
pub fn bundled_engine() -> Option<EngineConfig> {
    None
}

/// Startup extraction in the default (feature-off) build: a no-op.
#[cfg(not(feature = "bundled-stockfish"))]
pub fn extract() -> anyhow::Result<Option<EngineConfig>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In the default build the feature is off, so both entry points are inert —
    /// no embedding, no GPLv3 obligation, the resolution slot stays empty.
    #[cfg(not(feature = "bundled-stockfish"))]
    #[test]
    fn default_build_bundles_nothing() {
        assert!(bundled_engine().is_none());
        assert!(extract().unwrap().is_none());
    }

    /// With the feature on, the resolution seam and the extracted config agree on
    /// the engine name and on the deterministic cache path (they must, or
    /// resolution would point at a file extraction never wrote).
    #[cfg(feature = "bundled-stockfish")]
    #[test]
    fn resolution_and_extraction_agree() {
        // Only meaningful when a binary was actually embedded for this target.
        if let Some(resolved) = bundled_engine() {
            assert_eq!(resolved.name, BUNDLED_ENGINE_NAME);
            let extracted = extract().unwrap().expect("embedded binary extracts");
            assert_eq!(extracted.path, resolved.path);
        }
    }
}
