# 0046 — Assistant: per-session model choice, switchable mid-session

**Context.** Issue #215 (depends on #214's `ModelSelect`/`ProviderInfo.models`
catalog). The embedded assistant (ADR-0040) always started a conversation on
the calling user's stored default provider/model (the `DEFAULT_PIN` sentinel,
resolved at session start) and had no way to change it afterward. The
entanglement engine (0.6.0) already ships both halves of the wire protocol
this needs — `InMsg::SetModel` / `OutEvent::ModelChanged` (#218) — wire-allowed
and persisted like any other frame; chess-base had simply never exposed them.

**Decision.**

- **Mid-session switch is a pure wiring exercise, not new engine work.**
  `InMsg::SetModel` was already `wire_allowed()`, already carries a `session`
  (so the existing WS ownership gate in `assistant_ws.rs` covers it for free),
  and `OutEvent::ModelChanged` already flows through `history_records`
  (protocol.rs's generic `LogPayload::Out(ev)` arm, no per-kind filtering) and
  replays on resume. No backend change was needed for the switch itself —
  only for making it reachable and displayable end to end (below).
- **Creation-time choice needs a new seam, because of the `Spawn` ordering
  finding (sessions.rs).** A session's `Spawn` frame queues its prompt
  *immediately*; a `SetModel` sent right after would land behind it and the
  first turn would already have run on the engine-default `EchoLlm`. The only
  binding that lands before that queued prompt is the profile's own model pin,
  which every `build` session shares as the static sentinel `DEFAULT_PIN`
  resolved via `AgentProviderStore::build_resolver`. To let `create` honor an
  explicit per-session pick without forking the profile per session,
  `AgentProviderStore` gained a **pending-pin seam**: `set_pending_pin(user,
  provider, model)` stashes the choice keyed by user id; the next
  `DEFAULT_PIN` resolution for that user consumes (and clears) it via
  `take_pending_pin`, falling back to `default_for` when nothing is queued.
  Keyed by user rather than session because the resolver closure only ever
  sees the user id — two `create` calls for the same user in the same instant
  could in principle race, accepted as out of scope (a single browser tab
  issues them serially).
- **Validate before touching the DB — the "EchoLlm stub trap".** An
  unresolvable `(provider, model)` pin fails *silently* at session start (a
  logged warning, session stays on the default backend) — exactly what #214's
  `llm_for_choice` was written to avoid for the batch-LLM paths. `create` now
  validates an explicit choice synchronously against `SessionService`'s own
  `ModelResolver` (the same one the engine dials) before creating the
  `agent_sessions` row or sending `Spawn`; an unresolvable pick is
  `SessionError::InvalidModelChoice` (WS error code `invalid_model`, distinct
  from `no_provider`), mirroring `AppState::llm_for_choice`'s 400-vs-503 split.
  `provider` without `model` is the same error.
- **Wire surface:** `ClientFrame::New` gained optional `provider`/`model`
  fields (both or neither); `SessionService::create` takes them as trailing
  params. No new server → client frame type — `ModelChanged` rides the
  existing `ServerFrame::Out` envelope like any other engine event.
- **Frontend:** `lib/assistantStream.ts`'s `TranscriptState` gained a `model`
  field, folded from `model_changed` (and so restored on history replay for
  free). `stores/assistant.ts` gained `setModel(provider, model)` (sends
  `InMsg::SetModel` for the open conversation) and `newSession` takes an
  optional `ModelChoice` threaded onto the `new` frame. `AssistantView.vue`
  reuses #214's `ModelSelect.vue` unmodified in two places: the composer (only
  before a conversation exists, bound to the creation-time pick) and the
  conversation header (only once one is open, bound to the live model and
  switching immediately on change) — never both at once. Because `SetModel`
  always needs a concrete pick, the header's own "(default)" option resolves
  locally via #214's `effectiveDefault` before sending.

**Consequences.** A user can pick a model per conversation and swap providers
mid-conversation without losing history; both paths reject an invalid pick
before it can silently degrade to the `EchoLlm` stub. The pending-pin's
user-keyed race window (two `create`s from the same user in the same instant)
is accepted, matching the existing `DEFAULT_PIN` design's own scope. No schema
change — the pin is in-memory only, and the resolved model is recovered from
`agent_events`' persisted `ModelChanged` records like every other engine fact.
