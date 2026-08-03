# 0044 — OAuth consent screen + CSRF, refresh-token reuse detection, scoped service tokens

**Context.** Issue #193: three gaps in ADR-0016's OAuth 2.1 + service-token
auth for `/mcp`, all found in the same security pass as #192 (ADR-0043).

1. `GET /oauth/authorize` auto-consented: a logged-in user hitting it was taken
   to have approved *any* client, with no CSRF token binding the request to
   the browser that initiated it. An attacker who registers their own OAuth
   client (`POST /oauth/register` was open to anyone) and forces the victim's
   browser to navigate to `/oauth/authorize?client_id=<attacker's>&…` gets a
   real access+refresh token for that victim, silently.
2. `refresh` deleted the old `(access_token, refresh_token)` row and minted a
   fresh pair on every use — so a stolen refresh token and the legitimate
   client's next refresh would race, and whichever lost the race had no way to
   detect it had happened (the winner's new pair looks perfectly normal).
3. `service_tokens` had no `scope` column: every row was full read+write
   impersonation of `owner_id`, and there was no route or CLI to mint one
   short of hand-inserting a DB row.

**Decision.**

*Consent + CSRF.* `GET /oauth/authorize`, once the caller is resolved as
logged-in, now checks `oauth_consents` for `(user_id, client_id)`. A prior
approval issues a code exactly as before (extracted into
`oauth::issue_code_and_redirect`, shared with the consent-approval path).
Otherwise it inserts an `oauth_consent_requests` row (10-minute TTL, single-use,
carrying every parameter the eventual code needs) and redirects to
`GET /oauth/consent?csrf_token=…`. That route (`oauth_consent.rs`) renders a
minimal hand-rolled HTML page — no template engine dependency needed — naming
the requesting client (HTML-escaped: `client_name` is attacker-controlled free
text from `POST /oauth/register`, and this is exactly the page a malicious
client's victim lands on) and what the `chess` scope grants. The page posts
back to `POST /oauth/consent` with the same `csrf_token` in a hidden field.
That token **is** the CSRF defense: it's delivered only inside this rendered
page, so a forged cross-site POST — which under `SameSite=Lax` typically has no
session cookie at all — additionally cannot supply the one value that makes
the request valid, since the attacker's forced navigation of the initial
`GET /oauth/authorize` never lets them read the resulting page (same-origin
policy). The consent-request row is deleted the moment it's looked up in the
POST handler, approved or denied, so replaying that exact POST always fails
afterward. Approving records `oauth_consents` (idempotent upsert) so later
authorizations for the same pair skip the screen — the pre-ADR-0044
"auto-consent" behavior, now opt-in per client instead of universal.
`POST /oauth/register` (client registration) is now also gated on an
authenticated caller — an anonymous request 401s before a client can be
registered at all, closing the "anyone can register a client" half of the
attack surface (local mode is unaffected: its `CurrentUser` is always the
implicit admin).

*Refresh-token reuse detection.* `oauth_tokens` gains `family_id` (shared
across every row descended from one authorization-code exchange) and
`revoked` (bool). Rotation (`rotate_oauth_tokens`) now marks the old row
`revoked = true` **in place** instead of deleting it, then inserts a fresh
pair reusing the same `family_id`. `refresh` checks, in order: (1) row not
found → `invalid_grant`; (2) row `revoked` → **reuse detected** — every row
sharing `family_id` is revoked and the caller gets `invalid_grant`, so a
stolen-and-replayed refresh token cannot keep racing the legitimate client
indefinitely, both eventually lose; (3) the family's oldest row is older than
`ABSOLUTE_REFRESH_TTL_DAYS` (30) → the family is revoked and `invalid_grant`
returned, a hard ceiling independent of how often the token has legitimately
rotated. `oauth_token_user` (the `/mcp` access-token resolver) treats a
`revoked` row the same as an expired one.

*Scoped service tokens.* `service_tokens` gains `scope` (`"full"` |
`"read_only"` | `"global_read"`, default `"full"` so every existing row keeps
today's behavior) and a non-secret `id` (the bearer secret stays `token`, the
PK — `id` is what admin list/revoke operate on so the secret is never
re-displayed). `CurrentUser` gains two independent axes composing with
ADR-0043's `public`: `read_only` (never write, regardless of ownership) and
`global_only` (drop the own-rows arm of `scope()` — global rows only).
`anonymous()` now sets both explicitly (it always implied them, this makes it
explicit rather than piggybacking on `public` alone); `local_admin()` sets
neither. `service_token_user` maps a row's `scope` onto these axes:
`read_only` → `(true, false)`, `global_read` → `(true, true)`, anything else
(including a legacy pre-ADR-0044 row, always `"full"` after backfill) →
`(false, false)`. `assert_admin`/`assert_can_write` hard-deny a `read_only`
caller the same way they already hard-denied a `public` one — even over the
caller's own resources. The MCP dispatch layer additionally gates any tool
`ai::agent::requires_approval` flags as mutating: a `read_only` caller's
`tools/call` on one is rejected before the handler runs, and `tools/list`
filters those out so the advertised surface matches what's actually callable.

New minting surface: `service_tokens::ServiceTokenService` (admin-gated
create/list/revoke, mirroring `ai::providers::ProviderService`'s shape) backs
`POST`/`GET`/`DELETE /api/admin/service-tokens` and a
`chess-base service-token create|list|revoke` CLI subcommand — previously the
*only* service token in existence was the auto-seeded local one, with no way
to mint another short of a hand-written DB row.

**Migration** (`m0010_oauth_hardening`, schema-builder + raw-SQL backfills):
`service_tokens` gets `scope` (default `"full"`) and `id` (backfilled from
`token`, unique-indexed); `oauth_tokens` gets `family_id` (backfilled from
`access_token`, so every pre-existing row starts its own singleton family) and
`revoked` (default `false`); two new tables, `oauth_consents` (composite PK
`(user_id, client_id)`) and `oauth_consent_requests` (PK `csrf_token`).

**Consequences.** A first-time OAuth authorization now costs the user one
extra screen (approve/deny) instead of being silent — the acceptable tradeoff
for closing the forced-navigation token theft. Revoked `oauth_tokens` rows
accumulate with no GC yet (mirrors ADR-0016's own deferred
hashing-at-rest note) — fine at the scale of a self-hosted single/few-tenant
deployment, but a future cleanup job (or `expires_at`-based deletion once a
row is old enough that reuse detection no longer matters) is a reasonable
follow-up. `global_read`'s "global rows only" restriction only bites on
services that already route reads through `identity::scope()` — a future
service that queries ownership some other way would need its own check, same
caveat ADR-0043 already noted for `public`. Adding the two new `CurrentUser`
fields touched every existing `CurrentUser { .. }` construction site
(mechanical `read_only: false, global_only: false` additions across ~30 test
helpers and the handful of production sites), same shape of churn ADR-0043
already went through for `public`.

**Links.** ADR-0016 (MCP auth baseline), ADR-0043 (anonymous public MCP tier —
the `public`/`scope()` seam this builds on), ADR-0011 (`CurrentUser`/`scope`
seam), ADR-0025 (`GATED_TOOLS`/approval gate, reused verbatim for the
`read_only` MCP gate), ADR-0036 (MCP tool surface symmetry).
