# ADR-0054 — Every engine call finishes by a deadline

Status: accepted

## Context

chess-base now runs on the home desktop (ADR-0052), so Stockfish load competes
with the machine's own work. systemd caps it at `CPUQuota=200%`, but nothing
capped *time*:

- The live analysis WebSocket sent `go infinite` whenever the client set no
  limits, so a forgotten tab searched for as long as the socket stayed open.
- Long jobs loop over hundreds of searches: game review, study analyse (with
  optional shape regeneration), danger map, opening tree, and study generation.
  Each search already had its own deadline (`search_deadline`: movetime + 5s, or
  60s), but the job as a whole had none. A long game at high depth could hold the
  shared engine pool for many minutes.

## Decision

1. **Live analysis is capped at 30s** (`MAX_MOVETIME_MS`).
   - `engine_ws::live_limits` clamps the client's limits and adds
     `movetime 30000` when the client gave no movetime, so `go infinite` is never
     sent. A depth request keeps its depth, and whichever bound hits first ends
     the search.
   - A watchdog sends `stop` if no `bestmove` arrives by 30s + 5s. If the engine
     still hasn't stopped after another 5s, the server sends an error frame and
     closes the session.
   - A new `analyse` restarts the clock.
   - The SPA already treats `bestmove` as "done", so it needed no change.
2. **Long jobs get a 5-minute budget** (`engine::budget`, `JOB_BUDGET`).
   - A job runs inside `budget::job(…)`, which keeps the deadline task-local.
     `EngineService::{analyse, analyse_multi}` bound each call by the time left.
   - Once the budget is spent, a call returns an *empty* result (no score, no
     lines) instead of searching. A call already queued or searching when the
     deadline passes is dropped, which kills its engine process and releases its
     pool permit.
   - So a job takes at most 5 minutes, including the search in flight.
   - Nested jobs share the outer budget, so nesting never extends it.
   - Calls outside a job (the one-shot `engine_analyse` MCP tool) are unchanged.
3. **Partial results, flagged.** Jobs don't fail when the budget runs out; they
   stop and return what they finished, with `truncated: true`:
   - `review_game` reviews only the plies whose before and after positions were
     both searched (`GameReview.truncated`).
   - `analyse_study` stops at the first skipped node. Nodes already analysed keep
     their fresh evals/NAGs and the rest keep their old ones
     (`AnalyseStats.truncated`).
   - Shape regeneration and `apply_shapes` stop rather than strip generated
     arrows from the nodes they didn't reach.
   - The danger walk and the opening tree finish their DB-driven walk without
     engine data for the remaining positions: no danger tag and no eval.
   - Where it surfaces: the HTTP responses (`/api/studies/{id}/analyse`,
     `/api/studies/danger-map`, the generate routes, the game reviews) and the MCP
     tools (`study_analyse`, `opening_tree`, `danger_map` via
     `ToolOutcome::budgeted`).
   - The SPA shows "Stopped at the 5-minute engine limit — results are partial."
     in the study analysis, game review and danger panels.
4. **Task-local instead of a parameter.** The engine is reached through a dozen
   adapters and traits (`Evaluator`, `MultiAnalyzer`, plan/danger analyzers).
   Passing a budget through each would touch all of them to enforce one rule.

## Consequences

- An infinite-analysis UX ("leave it running") is gone. Re-sending the position
  continues it for another 30s.
- A very long study or game at high depth gets a partial review. Re-running it
  continues from the stored state only for study analysis, since nodes are
  persisted as they are analysed.
- Engine work done through `tokio::spawn` would escape the budget, because a task
  local doesn't cross spawns. Today no engine path spawns.
