# 0045 — Public sharing: per-object flags + anonymous HTTP read tier

**Context.** Issue #211. Every `/api/*` route required a session (server mode),
so a game or study could not be shown to a logged-out visitor at all — no way
to send someone a link to a game or an annotated study. The MCP surface already
has an anonymous tier (ADR-0043), but it is scoped to *global databases* and
tool dispatch, not to individual objects a user chooses to share.

**Decision.**

- **Plain deep links, no share tokens.** A shared object is addressable by its
  ordinary URL (`/api/games/{id}`, `/api/studies/{id}`); there is no capability
  token or unguessable slug. Sharing is an explicit per-object opt-in, and ids
  are low-entropy by design — the flag, not secrecy of the URL, is the access
  control. Revoking is flipping the flag back.
- **Independent per-object `public` booleans** — `games.public` and
  `studies.public` (migration `m0011_sharing`, default `false`). No new
  ownership axis: a public game stays in its database, a public study keeps
  its owner; only *reads* widen.
- **An anonymous HTTP read tier via the `PublicUser` extractor.** Six read
  routes (`GET /api/games/{id}`, `/tree`, `/export`, `/api/games/{id}/studies`,
  `GET /api/studies/{id}`, `/export` + `/export/lichess`) swap `CurrentUser`
  for `PublicUser`, whose `AppState::resolve_public_user` mirrors
  `authenticate_mcp` (ADR-0043): a server-mode request with **no** credential
  resolves to `CurrentUser::anonymous()` instead of `401`; a present but
  invalid/expired credential still `401`s; local mode stays the implicit
  admin. Every other route keeps the hard `401`, and the anonymous identity's
  existing hard-denies (`assert_can_write`, `assert_admin`, ADR-0043) keep the
  tier read-only in depth.
- **A public game overrides a private owning database for reads** —
  `GameService::get` skips the database-visibility check when the row is
  flagged. That is the point of the flag: the deep link works even though the
  collection stays private. The pre-existing ADR-0043 behavior (anonymous
  reads on *global* databases) composes unchanged. Writes (`delete`,
  `set_public`) still run the full database-ownership chain.
- **Studies are excluded from the anonymous global arm.** The studies
  `read_scope` gives the anonymous caller **only** `public = true` — never the
  `owner_id IS NULL` arm an authenticated caller gets. ADR-0043 deliberately
  scoped the anonymous tier to game-data reads; a global study becomes visible
  to logged-out visitors only by being explicitly flagged.
- **Annotated export is denied anonymously** (`?annotated=true` → `401`): it
  runs the engine per ply, and the anonymous tier gets no engine compute.
- **Toggles are HTTP-only** (`PUT /api/games/{id}/public`,
  `PUT /api/studies/{id}/public` — the game toggle uses delete's permission
  chain, the study toggle the usual study write guard): sharing is a
  human/UI action, so both are `CARVE_OUTS` in the MCP symmetry manifest
  (ADR-0036), and the MCP `ANONYMOUS_ALLOWLIST` is unchanged.

**Consequences.** A logged-out visitor can open a shared game or study (and
its linked public analyses) by URL; nothing else about the API surface moves.
Anyone who obtains a shared object's id can read it while the flag is set —
accepted, that is what "public" means here. The SPA views for these deep links
ship separately (this change is backend-only); DTOs (`GameSummary`,
`GameDetail`, `StudySummary`, `StudyLink`) already carry `public` for the UI
to render the toggle.
