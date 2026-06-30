# 0025 — Embedded Claude study assistant reuses the in-process MCP tool surface

**Context.** Epic 7 exposes engine / database / study tools over an MCP JSON-RPC
endpoint so an *external* AI client can drive chess-base (Direction A). Issue #20
adds the optional *embedded* counterpart (Direction B): an in-app chat backed by a
Claude API client + agent loop. The hard requirement is that the embedded loop use
the **same** `StudyService` / position / engine functions as in-process tools — no
second implementation — and that mutating actions be gated behind explicit user
approval with a visible iteration cap. The API key must stay server-side and must
not be required for Direction-A deployments.

**Decision.** Three pieces.

1. **One tool surface.** The agent loop drives the existing MCP `ToolRegistry`
   in-process rather than a parallel tool set. `ToolRegistry` gains `tools()` +
   `invoke(name, …)` and `Tool::invoke`; `ai/assistant` builds its `ToolSpec`s from
   the registry and dispatches calls straight back into it. The `/mcp` transport
   and the embedded chat are therefore two callers of one registry. Tools that
   mutate the caller's data (`study_create`, `study_import_pgn`, `study_add_move`,
   `study_annotate`, `generate_study`) are gated; read-only tools run automatically.

2. **A resumable, persisted loop (`ai/assistant`).** `AssistantService::drive`
   asks `LlmProvider::complete`, records the assistant turn, and either finishes
   (no tool calls), runs the read-only calls and loops, or **pauses** when any call
   needs approval. The pause is represented purely by transcript shape — a trailing
   assistant turn whose tool calls have no `ToolResults` yet — so it survives across
   HTTP requests with no extra state: the SPA renders `pending_approvals`, the user
   decides per call, and `respond` runs the approved calls (a denial becomes an
   error tool result the model sees) and continues. The cap is `MAX_ITERATIONS`
   tool rounds since the last user message, derived from the transcript
   (`iterations_since_user`) — no counter to persist — and surfaced to the SPA.
   Sessions are private, owner-scoped (`assistant_sessions`); the transcript
   (`assistant_messages`) stores one `ai::llm::Message` serialized per row, so
   loading a session is a parse, not a translation. Gating + view derivation are
   pure and unit-tested; the loop is unit-tested with a stub provider over a real
   in-memory store.

3. **Provider registry (`llm_providers` / `ai/providers`).** Providers are
   admin-managed rows; the default row builds the `LlmProvider` at startup, else
   the `ANTHROPIC_API_KEY` env fallback (`resolve`). API keys are **write-only** —
   the `ProviderInfo` DTO and every route omit them — so the key is consumed
   server-side to build a client and never reaches the SPA. The HTTP surface is
   `/api/assistant/*` (sessions CRUD, `messages`, `respond`, admin `providers`),
   thin callers returning a `503` when no provider is configured.

**Consequences.** Building a study from chat ("build me a repertoire vs the
Sicilian") goes through the exact services the REST API and MCP tools use, with
ownership/admin gating in one place; the loop cannot silently mutate data (every
write is approved) and cannot run away (capped rounds), both visible in the UI.
Direction-A deployments are unaffected — they configure a provider (or not) the
same way and never load the chat. The reused registry means a new tool is exposed
to both transports at once; a tool's mutating-ness is declared once in
`GATED_TOOLS`. Trade-off: a turn that mixes read-only and mutating calls holds the
read-only ones until approval too (Anthropic requires a result for every
`tool_use` before the next turn), accepted for a simpler, batch-coherent pause.
