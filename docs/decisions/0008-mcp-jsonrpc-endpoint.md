# 0008 — MCP via a hand-rolled JSON-RPC `/mcp` endpoint

**Context.** A core feature is building studies with AI: an AI agent (claude.ai or
Claude Code) should create and annotate studies through tools. The `site` project
(Orechov/Blog) already exposes an MCP server in Rust.

**Decision.** Mirror `site`'s proven pattern: a hand-written **JSON-RPC 2.0
endpoint at `POST /mcp`** mounted in the existing Axum app (`initialize`,
`tools/list`, `tools/call`), not a new MCP server crate. Tool logic lives in a
transport-agnostic **`StudyService`** so the HTTP MCP route — and a possible later
embedded Claude assistant — are both thin callers. Auth: OAuth (server mode) with a
static service-token fallback (local mode), scoping every mutation to `owner_id`.

**Consequences.** No new server-side MCP dependency; works for remote (claude.ai)
and local (Claude Code `claude mcp add --transport http`) clients. Request/response
tools need no SSE. An embedded in-app AI assistant (Direction B) is deferred until
the tool surface proves out. Detailed in Epic 7.
