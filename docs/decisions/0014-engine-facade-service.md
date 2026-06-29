# 0014 — Engine facade: one UCI service, two consumption paths

**Context.** The engine is consumed two ways depending on whether the model is
in the loop. The **batch** study-generation pipeline (ADR 0009) needs engine
eval/PV as *ground truth* it orchestrates in code — and that eval/PV must never
enter any LLM context. The **interactive** MCP client (ADR 0008) needs the same
analysis exposed as a callable tool. The Epic 5 `manager::Engine` (ADR 0012) is
a single-search-at-a-time process handle whose caller owns the read loop — the
right primitive, but too low-level to hand to two unrelated callers, and there
must be **one** engine pool behind both so a host isn't running redundant
engines.

**Decision.** Add `engine/service.rs` with `EngineService`: a small bounded pool
over `Engine` exposing one one-shot method,
`analyse(fen, limits, options) -> Analysis`.

- `Analysis` is a flat, `Serialize` struct — `bestmove`, `ponder`, `score`,
  `depth`, `pv` — distilled from the primary (MultiPV 1) line plus the terminal
  `bestmove`. The event-folding (`fold_primary`, `Analysis::from_search`,
  `bounded`) is pure and unit-tested without a process; unbounded limits get a
  default depth so a one-shot call always returns.
- The pool spawns engines lazily, reuses idle ones, and caps live processes with
  a semaphore (permit held for the whole search). A failed search discards its
  engine rather than returning a mid-state handle to the pool.
- Because the pool is single-permit, user-supplied limits are bounded so one
  request can't pin it (issue #93): `Limits::clamped` caps `depth`/`movetime_ms`
  (`MAX_DEPTH` 60, `MAX_MOVETIME_MS` 30s) — applied both at the MCP arg boundary
  (which also rejects `< 1` and avoids the `depth as u32` wrap) and inside
  `bounded` as defence in depth — and the whole search runs under an overall
  deadline (movetime + grace, else a 60s ceiling) so a stuck engine can't hang
  forever; a timed-out search discards its engine.
- **Two facades, one pool.** The batch pipeline calls `analyse` directly
  in-process — the eval/PV is plain Rust data that never touches the LLM. The MCP
  endpoint registers an `engine_analyse` tool that routes through the *same*
  `analyse`. `AppState.engine_service` holds an `Arc<EngineService>` built from
  the same `EngineConfig`; `None` ⇒ both facades are disabled (the tool returns
  an `isError` outcome).

The streaming WebSocket (ADR 0012) keeps its own per-socket engine: it needs
incremental `info` updates and a mid-search `stop`, which the one-shot pool
deliberately does not model. The facade is a peer consumer of `Engine`, not a
replacement for the WebSocket.

**Consequences.** Batch analysis is a plain in-process call with no LLM
involvement — the ADR-0009 architectural guard holds by construction, not
convention. Interactive clients get the identical analysis over MCP, backed by
the same pool. The default pool size is one (batch + MCP serialized) so a
multi-threaded engine isn't oversubscribed against itself on a shared host;
raising it is a one-line change. `setoption` values persist on a reused engine,
which is harmless because the result only reads the primary PV. A real engine is
integration-tested behind `CHESS_BASE_TEST_ENGINE` (skipped when unset).
