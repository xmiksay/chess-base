# 0035 — Annotate transpositions in merged repertoire trees

**Context.** `MoveTree::merge_games` (#170, ADR-0033) deliberately keeps every
move-order into the same tabiya as its own branch — the Catalan via 1.d4 / 1.Nf3
/ 1.c4 is all real preparation and must stay visible. But the tree has no idea
two branches reached the *same position*: continuations get duplicated under
each entry order and drift apart as a study is edited by hand afterwards.
ADR-0033 flagged this as a likely follow-up.

**Decision.** A pure Zobrist walk in the `pgn_tree` layer, run automatically at
the end of a merge and available standalone for any other study.

- **`MoveTree::mark_transpositions() -> usize` (pure, `pgn_tree::transpositions`).**
  Walks the tree in **mainline-first preorder** — `children[0]` (the actual
  mainline, already frequency-ordered by `merge_games`) descends to a leaf before
  any variation is touched — hashing each node's position with the existing
  Zobrist (`position::apply_san`, incremental, one call per edge). The first node
  to reach a given hash is canonical; every later node reaching the same hash is
  tagged with a note: `"Transposes to the main line after 2.c4"`, naming the
  *canonical* node's own last move, formatted from the actual side-to-move/
  move-number at that point (not assumed from a standard start, so a set-up
  `start_fen` still reads right).
  - The note is **appended**, never clobbers: a `merge_games` stats comment
    (`"12 games, 71% (…)"`) survives alongside it, since a branch point is
    frequently also a transposition point. A prior note is stripped and
    refreshed on a re-run (idempotent), recognized by a fixed marker string.
  - A tagged node's own subtree is walked too — its continuations typically
    duplicate the canonical line further down, so they get tagged as well
    (`assert_eq!(marked, 2)` in the "keeps getting tagged further down" test).
  - Deliberately **comment-only**: it does not prune, delete, or redirect the
    duplicated subtree (the issue's own "optionally stop duplicating" is left
    for a later pass) — a first cut that surfaces the transposition without
    touching tree structure edits (promote/reorder/delete) already rely on.

- **Wired into the merge.** `pgn_tree::merge::merge_games` calls
  `mark_transpositions()` as its last step, after frequency-ordering and stats —
  so every `POST /api/studies/merge-games` result already carries transposition
  notes, no separate call needed.

- **Standalone endpoint.** `StudyService::mark_transpositions(user, study_id)`
  (`studies/mark_transpositions.rs`, kept out of the over-cap `mod.rs`) loads a
  writable study, runs the same pure pass, and persists it — for a study built
  or hand-edited some other way (a plain PGN import, manual grafts). `POST
  /api/studies/{id}/mark-transpositions` (`studies/mark_transpositions_route.rs`,
  its own tiny router merged in `server/routes/mod.rs`, mirroring
  `danger_route.rs` — both `routes.rs` and `mod.rs` are already over the
  file-size cap) is a thin caller returning the refreshed `StudyView`.

**Consequences.** A merged repertoire's alternate entry orders now point back at
each other instead of silently duplicating a continuation; a user editing the
canonical branch is nudged (via the note) to also check the transposed one. The
note can go stale if the canonical branch is edited after the fact — re-running
the pass (automatically on the next merge, or via the standalone endpoint)
refreshes it. Actually collapsing the duplicated subtree into a single canonical
one (so an edit in one place propagates) is a larger structural change, left as
a possible follow-up.
