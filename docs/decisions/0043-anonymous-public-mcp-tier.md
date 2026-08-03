# 0043 — Anonymous public MCP tier: data reads on global databases only

**Context.** Issue #192 (part of #200, sibling of #193). `/mcp` was
all-or-nothing (ADR-0016): a valid bearer resolved to a full `CurrentUser` (own
∪ global reads, writes on own data), anything else was a `401`. There was no
way to expose the global (admin-managed) game databases to a public,
unauthenticated consumer without also handing out a token that carries a real
user's private data and write surface.

**Decision.**

- **`CurrentUser` gains a `public: bool` axis** (`server/identity.rs`) instead
  of a parallel `Caller`/`Option<CurrentUser>` type. `CurrentUser` is passed by
  value or reference into ~40 transport-agnostic services (HTTP routes *and*
  MCP tools) that already destructure `id`/`is_admin` directly; wrapping it in
  an enum would mean threading a match through every one of those call sites
  for no behavioral gain, since the anonymous caller still needs to flow
  through the exact same `scope()`/`assert_can_write` seam every other caller
  does. A field is the minimal change that preserves that seam.
  `CurrentUser::anonymous()` mints the sentinel (`id: "anonymous"`, `is_admin:
  false, public: true`) — the `id` value is never actually compared once
  `public` is set (see below), it just makes the identity readable in logs.
- **`scope(owner_col, user)` drops its own-rows branch for a public caller**:
  `owner_col.eq(user.id) OR owner_col IS NULL` becomes `owner_col IS NULL`
  only. This is the one seam every read-scoped service already goes through
  (`DatabaseService`, `GameService`, `HeaderSearchService`,
  `PositionReportService`, …, ADR 0007/0011), so fixing it here is what makes
  every existing MCP data tool anonymous-safe with no further code — no tool
  needed a bespoke "is this row global" check.
- **`assert_admin`/`assert_can_write` hard-deny a public caller** before their
  normal admin/ownership logic runs, returning `AuthError::Unauthorized`
  ("authentication required") rather than `Forbidden` ("admin privileges
  required") — the accurate message is "sign in", not "you're signed in but
  not allowed". Defense in depth: the MCP dispatch allowlist below should
  already keep an anonymous caller from ever reaching a write path, but the
  identity-layer check holds even if a future tool forgets the gate.
- **`authenticate_mcp` (`server/auth.rs`) mints the anonymous identity only
  when *no* credential is presented, and only in server mode.** A credential
  that fails to resolve (expired, garbage, revoked) still `401`s with the
  bearer challenge — a caller with a broken token needs to notice, not get
  silently downgraded to anonymous. Local mode is **unchanged**: a request
  with no credential still `401`s, because the single-user local install's
  printed service token is the only door in — there is no "the caller's own
  data" concept to distinguish from "everyone's data" when there's one user.
- **Dispatch-level allowlist (`server/routes/mcp/mod.rs`)**:
  `ANONYMOUS_ALLOWLIST` = `list_databases`, `db_list_games`, `db_read_game`,
  `db_position_report`, `db_reference_games`, `db_export_games`,
  `search_headers`, `echo` — plain data reads, nothing else. `tools/list`
  filters to this set for a public caller (so a client never sees a tool it
  can't call); `tools/call` rejects anything else with a JSON-RPC error
  telling the caller to authenticate, before the registry is even consulted.
  Engine tools (`engine_analyse`, `analyse_position`, `analyse_game`,
  `position_threats`, `position_concepts`, `opening_tree`, `danger_map`) are
  excluded on purpose: Stockfish search is CPU-bound and an unauthenticated
  caller could otherwise burn arbitrary server CPU for free (a DoS surface no
  amount of read-scoping fixes). Study/folder/game-mutation/import tools are
  excluded because they either mutate or expose per-user structure the reads
  above don't need to.

**Consequences.** No HTTP route behavior changed — the anonymous tier is
`/mcp`-only; every `/api/*` route still resolves `CurrentUser` via
`AppState::resolve_current_user`, which still `401`s server-mode with no
session/bearer. Adding the `public` field touched every existing `CurrentUser
{ .. }` construction site (mechanical `public: false` additions in ~30 test
helpers and 3 production sites in `auth/service.rs` / `ai/agent/tools.rs` /
`server/auth.rs`) but changed no behavior there. A future MCP tool that reads
data must still be added to `ANONYMOUS_ALLOWLIST` deliberately if it should be
public — the default for a new tool is authenticated-only, which is the safer
default for a project whose data can include private repertoires.

**Links.** ADR-0016 (MCP auth), ADR-0011 (`CurrentUser`/`scope` seam), ADR-0007
(database ownership), ADR-0036 (MCP tool surface symmetry).
