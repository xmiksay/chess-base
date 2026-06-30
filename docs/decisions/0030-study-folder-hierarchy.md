# 0030 — Study folder hierarchy + game-linked analyses

**Context.** Studies were a flat list: each row scoped to one `database_id`, with
no way to group them and no link back to the game an analysis came from. Game
reviews (Mode A, #119) were computed on demand and discarded — no table, no link,
no way to refetch an old review. So "organize my studies" and "persist this
analysis next to its game" had no home (issue #164). The natural persistence unit
for an analysis is already a study (a `MoveTree` with variations / comments / NAGs
/ shapes / `[%eval]`), so the work is to add an organization axis over studies and
a pointer from a study to its origin game.

**Decision.** A real folder tree as an **adjacency-list table**, account-level and
independent of the game databases, plus two nullable links on `studies`.

- **`folders` table (migration `m0007`).** `id`, `owner_id` (NULL ⇒ global/admin,
  mirroring `studies`/`databases`, ADR 0007/0011), `parent_id` (NULL ⇒ root;
  self-referential FK `ON DELETE CASCADE`), `name`, `created_at`. A unique index
  on `(owner_id, parent_id, name)` blocks duplicate siblings, with `(parent_id)`
  and `(owner_id)` indexes for the tree/scope reads.

- **`studies` gains `folder_id` and `origin_game_id`** (both nullable). `folder_id
  = NULL` is an unfiled study (shown at the root); `origin_game_id` points to the
  exact game an analysis was built from (`NULL` ⇒ standalone). `database_id` stays
  `NOT NULL` and unchanged — a study still scopes to a games collection for
  position context; folders are an orthogonal organization axis.

- **Referential rules live in the service, not the DB.** SQLite has foreign keys
  off by default and cannot `ALTER`-add one, so the columns on `studies` are plain
  nullable integers (no DB FK) and the folder FK cascade is effectively inert on
  SQLite. `FolderService::delete` therefore does the cascade itself: it collects
  the folder's whole subtree, unfiles every contained study (`folder_id = NULL`,
  never deletes it), then deletes the subtree. The unique-sibling guard is also
  enforced in the service — a unique index treats `NULL` as distinct on both
  backends, so a DB index alone can't catch root-level (`parent_id IS NULL`)
  duplicates; the index remains a backstop.

- **`FolderService`** (transport-agnostic, like `StudyService`): `list` (own ∪
  global via `scope()`), `create`, `rename`, `reparent` (rejects a move into the
  folder's own descendant — a cycle), `delete` (cascade + unfile). Writes gated by
  `assert_can_write`; a child folder must share its parent's owner (no interleaving
  an own tree with the global one).

- **`StudyService` extensions.** `set_folder` (validates the target folder is
  visible + writable), `create_from_game` (builds a `MoveTree` from the game's
  mainline and, when `analyse` is set, runs `review::review_game` and grafts
  `[%eval]`/NAGs/why-notes via `games::export::annotated_tree` — the same seam as
  #120/#162 — persisting with `origin_game_id` set and `database_id` = the game's),
  and `studies_for_game` (analyses linked to a game).

- **HTTP.** `GET/POST /api/folders`, `PATCH /api/folders/{id}` (rename and/or
  move, the move gated behind an explicit `reparent` flag so "move to root" is
  distinct from "leave the parent alone"), `DELETE /api/folders/{id}`;
  `PUT /api/studies/{id}/folder`; `POST /api/games/{id}/save-as-study` and
  `GET /api/games/{id}/studies`. All thin callers of the services.

**Consequences.** Studies organize into nested folders (own ∪ global, cycles
rejected, cascade unfiles rather than deletes), unfiled studies surface at the
root, and a game can be saved as an analysis that carries its engine evals and
links back to its origin game (the game view lists its analyses; the study shows a
back-link chip). The migration applies and reverses on SQLite and Postgres (one
`ALTER` column per statement; subtree/cascade handled in code). Future folder
support for other resources (e.g. databases) could reuse the same table by adding
a kind discriminator, but that is out of scope here.
