# 0024 — Toggleable board-overlay layers (Plans / Threats / Master moves)

**Context.** Issue #123 asks for board arrows organised into independent layers
the user can turn on/off (persisted per-user), plus a way to clear arrows. Today
the engine Plans overlay (#60) always writes straight to the board via
`engine.shapes` → `Board :shapes`. Two new layers are wanted: **Threats** (the
opponent's threats / our hanging pieces, red) and **Database master moves** (the
most-played continuations from the DB at this position). The open questions were
where the threat data comes from, and how to compose three sources onto one board
without each writing to it directly.

**Decision.**

- **Threats are a static scan, not an engine search.** A new pure `threats`
  module flags each side-to-move piece that is attacked by the opponent and
  either undefended or defended only behind a cheaper attacker (a capture that
  wins material), emitting a red `threat` arrow (the shared `pgn_tree::Shape`)
  from the cheapest attacker to the target. This is cheap, deterministic and
  fully unit-testable, and needs no engine or new WebSocket protocol — unlike a
  null-move/side-swap engine search, which would have been the only alternative
  data source. It trades completeness (pins, X-rays, deeper tactics) for a fast,
  I/O-free overlay. Exposed as `GET /api/threats?fen=…` → JSON `Shape[]`, a thin
  caller behind the same auth gate as the rest of the API.

- **Compose in one place, not three.** The board renders the **union** of the
  enabled layers, composed by the pure `lib/boardShapes.ts` (`composeBoardShapes`)
  rather than each source calling `setAutoShapes`. `engine.shapes` (Plans) becomes
  one input among three; the Threats and Database layers live in a new
  `stores/overlays.ts`, fed by `/api/threats` and the existing `/api/search/tree`
  (master-move SAN → arrow mapping + frequency sizing/labels in the pure
  `lib/masterShapes.ts`). `AnalysisView` owns the composition and watches the
  position + each toggle.

- **Toggles persist as key/value user settings — no migration.** `UserSettings`
  gains `show_plans` / `show_threats` / `show_master_moves` (`Option<bool>`,
  `serde(default)`/skipped), stored in the same per-user JSON blob as the rest of
  the settings. The frontend store supplies the defaults (plans on, threats and
  master off) when a flag is absent, so older blobs and fresh users behave
  sensibly.

- **Clear = clear hand-drawn arrows.** The computed layers are governed by their
  toggles; the "Clear arrows" control clears the user's own right-click drawings
  via `Board.clearUserShapes` (`cg.setShapes([])`), leaving the auto-shape layers
  intact.

**Consequences.** Threats ship without any engine/analysis dependency, so the
Plans toggle + master-moves layer + persistence + threats all land together. The
union composition means a disabled layer contributes nothing and a new layer is a
one-line addition to `composeBoardShapes`. The static threat scan can miss
tactics an engine would find; if that proves too shallow, an engine-backed threat
source can replace the `threats` module behind the same `GET /api/threats` shape
without touching the frontend composition.
