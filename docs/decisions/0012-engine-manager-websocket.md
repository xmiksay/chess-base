# 0012 — UCI engine manager + analysis streamed over a WebSocket

**Context.** Analysis is inherently incremental: a UCI engine emits a stream of
`info` lines at increasing depth and a terminal `bestmove`. The scaffold only had
`EngineConfig` + a single-line `parse_uci_line`. We need to run an engine as a
child process, configure it, drive a search, and push results to the SPA as they
arrive — with the ability to stop and reconfigure mid-search.

**Decision.** Split `engine` into a pure core and a thin process adapter, and
stream over a WebSocket:

- `engine/command.rs` (pure) builds UCI command text from a `Limits` model;
  `engine/analysis.rs` (pure) maps `vampirc-uci` messages to a flat, serializable
  `AnalysisEvent` (`info` / `bestmove`). Both are unit-tested without any process.
- `engine/manager.rs` holds `Engine`: a Tokio child process with `kill_on_drop`,
  the `uci`/`isready` handshake on spawn, `setoption`/`position`/`go`/`stop`, and a
  one-event-at-a-time `next_event`. It is deliberately single-search and owns no
  read loop — callers interleave reads with their own control flow.
- `server/engine_ws.rs` exposes `GET /api/engine/analyse`, an authenticated
  WebSocket that spawns the engine on `AppState.engine` (from `--engine` /
  `CHESS_BASE_ENGINE`; `None` ⇒ `503`) and `select!`s between client messages and
  engine events, restarting cleanly (stop → drain → re-`go`) on a new position.

WebSocket over SSE: analysis is bidirectional (the client also sends
`stop`/`analyse`), which a single WS gives us without a second channel.

The engine binary is operator-configured (not client-supplied) so a socket can
never make the server spawn an arbitrary process. A real engine is integration-
tested behind `CHESS_BASE_TEST_ENGINE` (skipped when unset); the command/parse
layer is covered by ordinary unit tests.

**Consequences.** Live, increasing-depth analysis reaches the UI with stop/
reconfigure support. The manager is one search at a time — concurrent multi-engine
or multi-board analysis would need a registry of `Engine`s keyed per session,
layered on top without changing the pure core. Engine auto-download (ADR 0005)
still owns discovering and registering the binary path this manager runs.

**Addendum (Plans overlay).** The handler defaults `MultiPV` to `3` when omitted
and emits an additive `{"type":"planline",…}` frame per PV-bearing `info`, wrapping
`plan_from_pv` trajectories (ADR 0017) alongside the unchanged `info`/`bestmove`
framing. A plan-computation failure degrades to empty `trajectories`, never
dropping the line. The wrapper lives in the server layer; `engine/analysis.rs`
stays untouched.
