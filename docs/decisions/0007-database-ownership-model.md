# 0007 — Databases are ownable; NULL owner = global

**Context.** Users add their own game databases (a Lichess sync, a Chess.com sync,
an imported PGN). Additionally, an administrator publishes shared databases (e.g.
the master games DB) that everyone can search.

**Decision.** A **database** is a first-class entity (`databases` table) with an
`owner_id`. `owner_id IS NULL` ⇒ a **global**, admin-managed database searchable by
all users; otherwise it belongs to that user. A user's search scope is *their*
databases ∪ all global databases. Games and the position index carry `database_id`.
In local mode the single user is implicitly admin, so the model collapses cleanly.

**Consequences.** One uniform model serves both local and multi-tenant server
deployments. Read queries filter `owner_id == caller OR owner_id IS NULL`; writes
to global databases require admin. Auth/roles arrive with Epic 6; the schema is
ready now.
