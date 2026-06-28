# 0011 — Request identity: one `CurrentUser` context, two resolution modes

**Context.** Many features are "scoped to the caller" or "global requires admin"
(ADR 0007; issues #1, #5, #6, #7, #15), yet there was no concept of *who is
calling*. `databases.owner_id` exists, but the current-user idea only arrived with
auth in Epic 6 (#14, late). Without one foundational identity each feature would
reinvent it, and the reinventions would conflict.

**Decision.** Introduce a single request-identity context and the shared
authorization helpers, decoupled from how the caller is authenticated.

- `CurrentUser { id, is_admin }` is the one identity type every service accepts. An
  Axum `FromRequestParts` extractor produces it (`src/server/identity.rs`).
- Resolution is the **only** thing that differs between run modes, and it lives in
  one place — `AppState::resolve_current_user` (`src/server/state.rs`):
  - **Local mode** → always the implicit admin (`local-admin`, `is_admin = true`),
    zero config.
  - **Server mode** → from session / Bearer auth. Not yet wired, so a server-mode
    request is rejected `401`; **#14** fills this in by changing only this method,
    not any handler signature.
- Two shared helpers enforce ADR 0007 in exactly one place:
  - `scope(owner_col, user)` → the read filter `owner_id == caller OR owner_id IS
    NULL`, generic over any owner column.
  - `assert_admin(user)` → gates admin-only actions (e.g. writing a global
    database), returning `403` otherwise.

**Consequences.** Every service takes `CurrentUser` and applies the shared
helpers, so the ownership/admin rule cannot drift per feature. Local mode works
with no auth configuration. #14 swaps in real server-mode resolution without
touching any call site. `AuthError` maps to `401`/`403`. A demonstration
`/api/whoami` endpoint exercises the extractor and lets the SPA gate admin-only UI.
