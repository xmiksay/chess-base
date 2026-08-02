# 0040 — Embed the entanglement agent engine, replacing the hand-rolled assistant

**Context.** The embedded study assistant (ADR-0025) was a hand-rolled loop:
one blocking HTTP POST per model round (no streaming — the SPA stared at a
spinner for a whole multi-tool turn), an Anthropic-only client behind a
`Transport` seam, one admin-managed provider resolved at startup into a
process-wide `AppState.llm_provider`, approvals encoded as transcript shape,
and its own `assistant_sessions`/`assistant_messages` transcript store. Every
one of those is a feature the author's entanglement engine
(`projects/personal/entanglement`) already implements better, and its 0.6.0
release added the missing piece — **multi-user mode** (session-scoped user
identity, a `UserProviderStore` seam, policy/persistence seams; entanglement
ADR-0147) — so one in-process engine can serve every chess-base user. Issue
#198 (follow-ups #200) replaces the whole assistant stack with an embed.

**Decision.** Full embed of entanglement **0.6.0 from crates.io**
(`entanglement-core`, `entanglement-runtime` with only the `provider` feature,
`entanglement-provider`), the old `ai/assistant` + `ai/llm/anthropic` deleted.

- **Module map** (`src/ai/agent/`): `mod.rs` keeps the surviving pieces
  (`SYSTEM_PROMPT`, `GATED_TOOLS`, `requires_approval`); `provider_store.rs`
  (`AgentProviderStore`: sync-cached `UserProviderStore` over `llm_providers`);
  `policy.rs` (`AgentPolicy`: `PermissionResolver` + `GrantStore` over SeaORM);
  `tools.rs` (`BridgedTool`: the MCP registry re-exposed 1:1, output capped);
  `persistence.rs` (`DbRecordSink` → `agent_events`); `sessions.rs`
  (`SessionService`); `engine.rs` (`AgentEngine::start` boots Holly + tool
  executor + persistence tap + throttle responder + index subscriber,
  `idle_ttl` 30 min). Schema is `m0008_agent_engine`: drops the assistant
  tables, extends `llm_providers` (`owner_id` NULL ⇒ global, `wire`,
  `base_url`, unique `(owner_id, name)`), adds `agent_grants` /
  `agent_events` / `agent_sessions`.
- **Sentinel model pin.** The engine's `build` profile pins provider and model
  to the `~default` sentinel (`DEFAULT_PIN`); `build_resolver` maps it to the
  *calling user's* default provider row. The profile pin is the only per-user
  binding that resolves before the queued spawn prompt's first turn, so
  `create` sends exactly one `Spawn { prompt, user }` frame — no racy
  `SetModel` chaser.
- **WS relay, not HTTP polling.** `server/assistant_ws.rs` exposes
  `GET /api/assistant/ws` (`CurrentUser`-gated upgrade): a thin envelope over
  the engine's kind-tagged `InMsg`/`OutEvent` plus session-CRUD verbs.
  Ownership is enforced fail-closed in both directions — inbound frames naming
  a foreign session are refused before the engine (then `send_from_wire`
  refuses privileged variants), outbound events are forwarded only to their
  owner (`SessionList` filtered per user, `Throttle` reduced to a boolean).
  Opening a session replays its persisted history.
- **Per-user providers with a house fallback.** `/api/assistant/providers` is
  per-user CRUD (keys write-only, `has_key` out); a user's rows compose over
  global rows, and users with nothing fall back to the `~house` context —
  global rows, else `ANTHROPIC_API_KEY` read once at startup (the env var is
  now *only* that fallback). Wires: `anthropic`/`openai`/`gemini`, unknown ⇒
  OpenAI-compatible when `base_url` is set. Batch LLM callers (study-gen,
  danger-map) get the same resolution via `AppState::llm_for(&user)` →
  `StackLlmProvider`, so the process-wide `llm_provider` field is gone.
- **Approvals + persistence over SeaORM.** `GATED_TOOLS` grade `Ask` in the
  base profile; an approval is honoured Once, per-Session (in-memory), or
  **Always** — a durable `agent_grants` row. Every engine record lands in
  `agent_events` (bounded channel, `ord`-ordered per root); resume is
  `integrity_gap`-guarded — a gapped log resumes the intact prefix and tells
  the user.
- **Aux on the user's model.** No `aux_llm_resolver`: compaction/summarize run
  on the session's own (user-chosen) model. No title generator — titles come
  only from user rename (`SetSessionMeta`).

**Consequences.** The assistant streams token-by-token, supports multiple
providers/models per user with real isolation (fail-closed ownership on every
path), remembers "always allow" approvals, and survives restarts (sessions
resume from the event log; the engine hibernates idle ones). One tool surface
remains: the MCP registry, bridged. Costs accepted: **two reqwest stacks**
(chess-base 0.12 + entanglement 0.13, different TLS roots — webpki vs native
certs); **old chat history is dropped** (m0008, no transcript migration —
different data model, low value); a turn interrupted mid-tool-call may
**re-offer mutating tools** after crash-resume (safe: still approval-gated);
**single-instance sessions** — the engine assumes one process owns the event
log (holds: k8s runs one replica; a DB lease is needed before scale-out); and
**per-user ceilings are deferred** (policy has a JSON-column extension point;
generated catalog entries default to 60 RPM). See chess-base #198/#200 and
entanglement ADR-0147.
