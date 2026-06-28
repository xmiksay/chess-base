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
