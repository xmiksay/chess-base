# 0046 — Assistant per-session model choice, switchable mid-conversation

- Status: accepted
- Issue: #215 (stacks on #214's picker groundwork)

## Context

Assistant conversations always started on the caller's *default* provider row:
the engine's `build` profile pins the `~default` sentinel (`DEFAULT_PIN`,
ADR-0040), which `AgentProviderStore::build_resolver` maps to the caller's
default at session start. Users want to pick a model per conversation and
change it mid-conversation without losing context.

## Decision

**Mid-session switch: ride the engine, no backend change.** entanglement 0.6.0
already ships a wire-allowed `InMsg::SetModel { session, provider, model }`:
the runtime re-resolves via `EngineConfig::model_resolver` with the session's
user, rebuilds the session `Llm`, defers to turn end when a turn is live, and
emits `OutEvent::ModelChanged { provider, model, context_window }` on success
(a recoverable `OutEvent::Error` on an unknown provider — the old binding is
kept). The WS relay already forwards ownership-checked wire-allowed `InMsg`s
inbound and session-scoped events outbound, and the persistence subscriber
already logs `ModelChanged` into `agent_events` (replay re-binds a resumed
session). So the switch is frontend + TS types only.

**Current model is derived from the event stream.** Session start funnels the
pin through the same `rebind` seam, so every transcript begins with a
`ModelChanged` naming the actual `(provider, model)`. The SPA folds the event
last-wins into `TranscriptState.model` (a divider marks a real switch); no
`agent_sessions` column, no migration.

**Creation-time choice: a one-shot pending pin over the `DEFAULT_PIN` seam.**
`Spawn` has no model field, and the profile pin is the only per-user binding
applied *before* the spawn prompt's first turn (see `sessions.rs`'s ordering
finding). `AgentProviderStore` gains `pending_pins: Mutex<HashMap<user,
(provider, model)>>`; `SessionService::create` parks the caller's choice
immediately before `Spawn`, and the resolver's sentinel branch becomes
`take_pending_pin(user).or_else(default_for(user))` — consumed exactly once.

**Guard: validate before spawning.** A pin-resolution failure at session start
only *warns* and leaves the session on the engine-default `EchoLlm` stub, so
`create` refuses a provider absent from the caller's composed catalog
(`knows_provider`, a pure cache read) with the typed
`SessionError::UnknownProvider` → an `unknown_provider` error frame, before
any row or engine session exists.

## Accepted races (narrow, documented)

- Two simultaneous creates by one user can cross pins (each session may start
  on the other's choice). Both sessions still announce their real binding via
  `ModelChanged`, so the UI never lies.
- If `Spawn` never resolves the pin (engine inbox closed after the pin was
  parked), the orphan is consumed by that user's *next* default-pinned
  session. Same self-correcting property: `ModelChanged` names what actually
  bound.

## Alternative considered

Spawn-then-`SetModel`: rejected — the supervisor queues the spawn prompt
immediately, so a `SetModel` sent after `Spawn` lands behind it and the first
turn (often the bulk of the work) would run on the default model.
