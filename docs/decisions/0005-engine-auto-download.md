# 0005 — Engines via an auto-download manager

**Context.** The app integrates UCI engines: Stockfish and Lc0 (with Maia weights
for human-like play). Requiring users to install and configure engine paths by
hand is friction; bundling engines bloats the binary and complicates licensing.

**Decision.** Ship an **engine manager** that detects the platform and downloads
Stockfish + Lc0 binaries and Maia weights into a local `engines/` directory on
first use, verifies checksums, and registers them. Paths remain overridable in
settings for users who already have engines. (Implemented in Epic 5; the scaffold
provides `EngineConfig` and UCI parsing.)

**Consequences.** Best out-of-the-box UX without a giant binary. Adds
download/update/checksum logic and per-platform asset URLs to maintain. Engines run
as external child processes over UCI (stdin/stdout).

## Amendment (2026-06-28) — optional bundled Stockfish + multi-engine registry

The single-engine manager (ADR 0012, `--engine` / `CHESS_BASE_ENGINE`, `503` when
absent) is implemented. Two refinements to engine *acquisition and selection*
without reversing the download-by-default stance:

**Optional `bundled-stockfish` feature, off by default.** Auto-download stays the
default path. For offline / air-gapped local builds, an opt-in `bundled-stockfish`
Cargo feature embeds a per-target Stockfish binary via `rust-embed` (mirroring the
frontend embedding of ADR 0004), extracts it to the OS cache dir on first use, and
registers it as the default engine. We do **not** bundle by default and never
bundle Lc0/Maia (weights are large and the binary is per-target).

- *Licensing.* Stockfish is **GPLv3**; embedding it makes that specific build
  artifact GPLv3. The default download build is unaffected (a separately-fetched
  child process is mere aggregation). The feature must be documented as carrying
  this implication, with a `LICENSING` note in the build docs.

**Engine resolution order** (first match wins): user-set path/runner in settings →
embedded binary (`bundled-stockfish` on) → auto-downloaded binary (ADR 0005
manager). A default that "just works" in every build flavor, with operator/user
override always winning.

**Multi-engine registry.** ADR 0012 already foresees "a registry of `Engine`s
keyed per session." Persist several `EngineConfig`s (name, path, **runner**,
weights) via the `settings` store plus a `default_engine` selector, behind a
transport-agnostic `EngineRegistry` service that both the WebSocket route and the
planned MCP tools call. The `runner` field allows an optional launch wrapper
(e.g. a script, `wine`, `docker exec`) in front of the engine binary.

Tracked by the Epic 5 / Epic 6 issues for the registry and the bundled feature.

## Amendment (2026-06-28) — download manager implemented (#11)

The auto-download manager now exists in `src/engine/download.rs`. `Platform::detect`
chooses a per-platform `catalog` entry (Stockfish + Lc0/Maia where available);
`Manager<F: Fetch>` downloads each asset, verifies a published SHA-256 (mismatch
rejected, nothing installed), installs via temp-file + atomic rename with the
executable bit on Unix, and is idempotent (a present, checksum-matching file is not
re-fetched). The HTTP boundary is the `Fetch` trait (`HttpFetcher`/`reqwest` in
prod, a synthetic fetcher in tests) so no real downloads run in the suite. Results
persist under a dedicated `downloaded_engines` settings key — separate from the
user-managed `engines` list — and feed the lowest-priority slot of the existing
`resolve_default` order. `serve` runs the manager best-effort at startup
(`--no-engine-download` to disable; `--engines-dir` / `CHESS_BASE_ENGINES_DIR`,
default `engines/`); download/checksum failures are logged, not fatal.

The `catalog` is the maintained per-platform data surface: it pins upstream
direct-download URLs and (eventually) checksums. Where upstream ships only an
archive, hosting a direct binary or adding archive extraction is follow-up
maintenance; until a checksum is pinned, an asset has `sha256: None` and is
installed unverified (logged).
