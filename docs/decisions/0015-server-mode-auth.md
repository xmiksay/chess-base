# 0015 — Server-mode auth: opaque sessions (Bearer/cookie) + first-user-is-admin

**Context.** Server mode is multi-user (ADR 0002), and the ownership model (ADR
0007) already distinguishes a caller's resources from global (`owner_id IS NULL`)
ones that only an admin may mutate. ADR 0011 introduced `CurrentUser` and a single
resolution seam (`AppState::resolve_current_user`), with server-mode resolution
left as a `401` stub for #14. We now need real accounts, a login, and an `admin`
role — without disturbing local mode (a single implicit admin) or any handler.

**Decision.** Add a server-mode-only `auth` module backed by two tables
(migration `m0003_auth`):

- `users(id, username, password_hash, is_admin, created_at)` — `id` is the string
  that lands in `owner_id`; passwords are hashed with **Argon2** (PHC string, salt
  inline). `username` is unique.
- `sessions(token, user_id, created_at, expires_at)` — **opaque** random tokens
  (two v4 UUIDs ≈ 244 bits), 30-day expiry, cascade-deleted with their user. The
  same token serves an `Authorization: Bearer <token>` header (API clients) and a
  `session=<token>` cookie (the browser SPA); `auth::token_from_headers` prefers
  the Bearer header.

`AuthService` (register / login / logout / `authenticate`) is transport-agnostic;
`auth/routes.rs` exposes `POST /api/auth/{register,login,logout}`, mode-gated to
`400` in local mode. Server-mode `resolve_current_user` reads the token and calls
`AuthService::authenticate`, returning `CurrentUser { id, is_admin }` or `401` — so
**only that one method changed**, not any handler or service signature.

**Bootstrap:** the *first* user to register becomes admin, giving a fresh
deployment a way to manage global databases without a separate provisioning step.
Login/credential errors are deliberately coarse (`invalid username or password`)
to avoid user enumeration; raw `DbErr`/hashing errors are never surfaced.

**Consequences.** The role gate reuses the existing `assert_admin` helper, so the
admin-only rule for global databases/studies is enforced in exactly one place and
cannot drift. Sessions are opaque DB rows (revocable on logout; no JWT key
management). Local mode is untouched — `auth` never runs there. A chosen-token
session model means stolen tokens are valid until expiry/logout; rotation and
cookie `Secure`/CSRF hardening can layer on later. The first-user-is-admin rule
trades a provisioning step for a deploy-time race that a real deployment closes by
registering the admin before opening signups.
