# ADR-0055 — Password change and admin password reset

Status: accepted

## Context

Server mode had register, login and logout, but no way to change a password.
A user with a leaked password could not rotate it, and an admin could not
help someone who had forgotten theirs. There is no email infrastructure, so
reset-by-mail is not an option.

## Decision

1. **Self-service change.** `PUT /api/auth/password {current_password,
   new_password}`. The current password is checked first. A wrong one returns
   `403`, not `401`, because the SPA treats `401` as "signed out". The new
   password must pass the register rule (≥ 8 chars) and differ from the old one.
2. **Login sessions only.** The route reads the raw token and resolves it as a
   *session*. An OAuth access token or a service token gets `403`
   (`NotASession`), so an MCP client can't change a password.
3. **What a change revokes.** All of the user's *other* sessions are deleted
   (the calling one survives). All of their OAuth tokens are marked `revoked`
   in place, the same way refresh rotation does it (ADR-0044), so MCP clients
   must re-authorize. **Service tokens are kept.** They are admin-minted
   machine credentials with their own revoke path, and tying them to a
   password would break automations. The hash update and the revocations run
   in one transaction.
4. **Admin reset.** `GET /api/admin/users` lists accounts (never the hash).
   `PUT /api/admin/users/{id}/password {new_password}` sets a password the
   admin hands over out-of-band. The target loses **every** session and their
   OAuth tokens. An admin can't reset their own password this way; that goes
   through (1), which asks for the current password. No temporary password
   and no forced-change flag, so no migration is needed.
5. **Surfaces.** Server mode only (`400` in local mode, like the other
   `/api/auth` routes). No MCP twins: these are symmetry carve-outs (ADR-0036).
   The SPA gets an "Account" section in Settings for everyone, plus a
   "Users" section for admins.

## Consequences

- A forgotten admin password with no second admin still needs database
  access. A CLI reset was considered and left out for now.
- Changing a password disconnects the claude.ai/MCP connectors. This is
  intended: a leaked password should not leave a live OAuth grant behind.
