# 0001 — Single crate, not a workspace

**Context.** The app spans chess logic, DB, collectors, an engine manager and an
HTTP server. A Cargo workspace with one crate per concern was considered, but the
reference `design` app is a single crate and the user asked to avoid sub-crates
unless necessary.

**Decision.** One crate (`chess-base`) with a library + a binary, organized into
modules (`position`, `pgn_tree`, `db`, `collectors`, `engine`, `server`). Enforce
modularity with the 500-line file cap instead of crate boundaries.

**Consequences.** Simpler builds and dependency management; faster iteration. If a
module ever needs independent versioning or reuse, it can be promoted to a crate
later. Module discipline (pure core vs adapters) substitutes for crate isolation.
