# 0034 — Position search gains a player/color/date filter

**Context.** `PositionSearchService::opening_tree`/`games_with_position` (ADR-0003,
issue #7) answer "what's played from this position" and "which games reach it",
but always over *every* game the caller can see. There was no way to narrow
either to one player's games — e.g. "what does Carlsen play here as White" — or
to a date range, even though header search (issue #6) already resolves exactly
that filter (`player`/`color`/`date_from`/`date_to`) for its own `games` query.
Issue #153 established the rule for scoping these queries: filter via a
condition joined/subqueried in the same DB round trip, never pull ids into Rust
and bind them straight back as an `IN (...)` list.

**Decision.** A single `PositionFilter { player, color, date_from, date_to }`
struct (`search::position`), threaded through every layer between the position
query and its callers.

- **`PositionFilter` and its resolution live in `search::position`,** not
  `search::headers`, even though header search defined the equivalent shape
  first: `position.rs` already sits below `headers.rs` in the dependency graph
  (`headers.rs` imports `GameHit`/`PositionSearchService`/`SearchError` from it),
  so putting the filter here — and moving `Color`, `player_ids_matching`, and
  the `LIKE`-escaping helpers (`contains_like`/`escape_like`) down from
  `headers.rs` — avoids a new circular import. `headers.rs` now imports `Color`
  and calls the moved `player_ids_matching`/`contains_like` back.

- **Binding stays a `games`-column condition, no id round-trips (#153).**
  `PositionSearchService::filter_condition` resolves `player` to matching
  `players` ids (case-insensitive substring, same escaped `LIKE` header search
  uses) and builds a `sea_orm::Condition` over `games.white_player_id` /
  `black_player_id` / `date` — added to `opening_tree`'s existing join on
  `games` and to `games_with_position`'s `games::Entity::find()` directly. A
  `player` that matches nobody short-circuits to an empty result (not an
  error), so the query is never issued.

- **`color` only narrows when `player` is set**, exactly mirroring
  `HeaderSearchService::search`'s existing rule: every game already has both a
  White and a Black side, so a bare `color` with no player is meaningless and is
  silently ignored rather than rejected or misinterpreted. `PositionFilter`'s
  `Default` (all `None`) is a total no-op, so every existing caller that passes
  it is unaffected.

- **Threaded through, not reimplemented, at every layer:**
  `PositionReportService::position_report`/`position_reports`/`references` take
  the same `&PositionFilter`; `GET /api/search/tree` and `GET /api/search/games`
  gain `player`/`color`/`date_from`/`date_to` query params, validated the same
  way `HeaderParams`/`SearchQuery` are (blank ⇒ unset, a bad `color` ⇒ `400`);
  `GenerateParams.filter` (`study_gen::generate`) narrows which games feed a
  generated study's tree, exposed on `POST /api/studies/generate`; the MCP
  `opening_tree` tool gains the same four schema properties, parsed the same
  way. `ReportContinuations` (the `ContinuationSource` adapter shared by both
  generators) now carries a `PositionFilter` field.

- **Danger-map generation is explicitly out of scope.** `walk_danger_spine_live`,
  `generate_danger_study_live` and `danger_generate`'s internal
  `ReportContinuations::new` calls all pass `PositionFilter::default()`
  unconditionally — the danger-map walk always sees every scoped game. Filtering
  a repertoire spine by player/color/date wasn't part of issue #172 and the
  spine walk's semantics (walking *the user's own* prepared line) don't map onto
  "whose games" the same way an opening tree's continuations do.

**Consequences.** A caller can now ask "what does Carlsen play here as White" via
`GET /api/search/tree?player=Carlsen&color=white` or generate a study from only
one player's games via `POST /api/studies/generate`'s `player`/`color` body
fields — both reusing header search's player/color resolution rather than a
second implementation. The cost is `Color`/`player_ids_matching`/
`contains_like`/`escape_like` moving modules (a one-time churn, not a duplication)
and `ReportContinuations::new`/`build_variation_tree` gaining a filter parameter
that every non-filtering caller (danger-map, and any future caller that doesn't
care) must pass `PositionFilter::default()` for explicitly.
