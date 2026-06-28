# 0013 — LLM provider client (Anthropic Messages API)

**Context.** Two pieces of Epic 9/7 need a Claude API client: the **batch
annotation pass** (#31, core to the LLM study pipeline — [ADR-0009](0009-llm-study-pipeline.md))
and the **interactive assistant** (#20, later/optional). The client was originally
described only inside the interactive work, which inverted the dependency — the
core batch pass would have transitively depended on optional work. The client also
needs tool-calling (the assistant drives engine/DB tools through it) and must keep
the API key off the SPA.

**Decision.** Extract a standalone, provider-agnostic LLM layer at `src/ai/llm`,
mirroring the `site` project's `ai/llm/registry.rs`:

- A small `LlmProvider` trait with one entry point, `complete(req) -> response`,
  over provider-agnostic `Message`s (user / assistant-with-`ToolCall`s /
  `ToolResults`) and optional `ToolSpec`s. The response carries free text and/or
  `ToolCall`s plus token `Usage`. The batch pass calls `complete` with no tools;
  the interactive assistant reuses the same surface with tools.
- One concrete provider, `anthropic::AnthropicProvider`, over `POST /v1/messages`.
  The trait leaves room to add others later without touching callers.
- The HTTP boundary is a separate `Transport` trait. Production uses
  `ReqwestTransport`; tests inject a stub that records the request and returns
  canned JSON, so wire encoding and response parsing are **fully unit-tested with
  no network**. The single live test is gated behind `ANTHROPIC_API_KEY` and
  returns early when it is unset.
- The model id is configurable. Default is a **Sonnet-class** model
  (`claude-sonnet-4-6`) — the batch pass annotates many positions, so cost matters
  — with Opus available by overriding the model per request.

**Scope.** This is the foundation only: the trait, the Anthropic client, and the
transport seam. The richer `site` registry concerns — DB-backed provider catalog,
per-provider rate limiting, pricing/clearance — are deliberately **out of scope**
here and deferred to the consumers (#31, #20) and a later registry issue if needed.

**Consequences.** #31 no longer depends on optional interactive work. The API key
is held server-side and sent only as the `x-api-key` request header; it never
appears in any client-facing type, so it cannot leak to the SPA. Tests run offline
and deterministically. Adding a second provider means one more `impl LlmProvider`
plus its wire conversion — no change to callers.
