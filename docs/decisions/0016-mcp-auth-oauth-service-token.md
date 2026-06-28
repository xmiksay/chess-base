# 0016 — MCP auth & scoping: OAuth 2.1 (server) + service token (local)

**Context.** The `/mcp` JSON-RPC endpoint (ADR 0008) was unauthenticated, and its
tool handlers had no caller — so they could not scope reads/writes per user. Epic
7/9 needs claude.ai to self-onboard against a deployed server, and the local-mode
single binary needs a zero-config way for a Claude client to connect. Server mode
already has users + opaque sessions (ADR 0015) and the ownership model (ADR
0007/0011); MCP must plug into the same `CurrentUser` seam rather than invent a
parallel one.

**Decision.** Authenticate every `/mcp` call and thread the resolved
`CurrentUser` into each tool. Two credential kinds, checked in order
(`server/auth.rs::authenticate_mcp`): an **OAuth 2.1 access token**, then a static
**service token**. A miss returns `401` with
`WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource"`,
the discovery hook OAuth-aware clients follow.

New tables (migration `m0004_oauth`):

- `service_tokens(token, owner_id, is_admin, label, created_at, expires_at?)` —
  static bearers. Local mode seeds one (`label="local"`, `owner_id=local-admin`,
  admin) at startup and prints the `claude mcp add … --header "Authorization:
  Bearer …"` line; the row is reused across restarts. The role rides on the row,
  so the local token needs no `users` row at all.
- `oauth_clients(client_id, client_name, redirect_uris, …)` — public, PKCE-only
  clients created via dynamic registration (RFC 7591) at `POST /oauth/register`.
- `oauth_codes(code, client_id, user_id, redirect_uri, code_challenge, …, used)` —
  short-lived (10 min), single-use authorization codes.
- `oauth_tokens(access_token, refresh_token, client_id, user_id, scope, …)` —
  issued pairs; the access token (1 h) is what `authenticate_mcp` resolves, the
  refresh token mints a fresh pair. Both rotate on refresh.

`server/routes/oauth.rs` implements the authorization-code + refresh-token grants
plus RFC 9728/8414 discovery metadata. `/oauth/authorize` requires a logged-in
user (server-mode session) and **auto-consents** — single-tenant, self-hosted, so
a logged-in user is taken to approve; an anonymous request bounces to the SPA
login carrying `next`. PKCE is **S256-only** (`base64url(SHA-256(verifier))`).
Authorization is by resource ownership, not granular OAuth scopes, so a single
coarse `chess` scope is advertised. Discovery URLs are built from the request
`Host`/`X-Forwarded-Proto`, so the same binary works behind any ingress.

MCP tool handlers now take `(AppState, CurrentUser, args)`. The study tools
(`study_create` / `study_add_move` / `study_annotate`) call the existing
`StudyService` with that caller, so the ownership write-guard (ADR 0007/0011)
rejects mutating a non-owned study — no new authorization code path.

**Consequences.** One auth seam covers both transports: HTTP keeps sessions (ADR
0015), MCP adds OAuth + service tokens, and both land on `CurrentUser`. Tokens are
stored raw (matching the existing `sessions` table) — revocable by row deletion,
no JWT key management; hashing-at-rest and granular scopes can layer on later.
Auto-consent skips a consent screen (acceptable for self-hosted single-tenant);
adding one later is a local change to `/oauth/authorize`. Local mode is now gated
by its printed token, so a stray process on localhost can no longer drive `/mcp`
unauthenticated.
