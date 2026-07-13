# 0026 — Danger-map opening mode: engine as adjudicator, not author

**Context.** The study generator (ADR-0009, issues #29/#31/#115) builds a tree by
keeping the engine-best, most-frequent moves: `tree.rs` `select_continuations`
applies a frequency floor and an `eval_margin_cp` cutoff, dropping every move
that is clearly worse than the best sibling. That is the right target for the
*best-line* study and for grading a **played** game (`review/`, #119), but it is
the wrong target for **opening preparation**. A tree of best moves is a re-render
of Stockfish's top line; it adds nothing over scrolling the analysis board.

The teaching value of an opening study lives in exactly what that pruning
deletes — a tempting move the opponent will mishandle, a position where the
opponent has only one adequate reply, a dangerous-but-unsound attacking plan.
Those are *low-eval* and/or *low-frequency* by nature, so the best-line pruner
removes them on purpose.

**Decision.** Add a second `study_gen` mode — the **danger map** — selecting for
**practical difficulty for the opponent**, not engine eval. The unifying
principle: **the engine is the adjudicator, not the author.** It does not pick
the lines; it vetoes and grades human-surfaced ones. Concretely:

- **Spine.** The mode is driven by a **PGN tree** = the user's intended
  repertoire, walked **from move 0**. A `database_id` supplies the human replies
  to mine from `position_index`. Making the spine explicit also yields a signal
  the best-line builder never had: *reachability*.

- **Signals** (each computed from infrastructure that already exists):
  1. **Reachability / move-order** — an opponent move that leaves the spine
     (you want the Grünfeld, but `1.e4` puts you somewhere else). First-class for
     a repertoire: it forces an answer to every move order.
  2. **Trap** — an *asymmetric* eval test. From our side-to-move perspective:
     `if_refuted` is the eval after the opponent's **best** reply (worst case),
     `if_baited` the eval after the **tempting** reply. A move is a **weapon**
     only when `if_refuted >= downside_floor_cp` **and** `if_baited >=
     baited_upside_cp` — bounded downside *and* real upside. A move that baits
     but drops below the floor when refuted is **hope-chess and is rejected**.
     This encodes the rule: *do not play a blunder because there is a trap.*
  3. **Only-move / narrow path** — a large MultiPV gap (`analyse_multi`,
     `PV1 − PV2 >= only_move_gap_cp`), weighted by how often humans miss the
     unique move in `position_index`.
  4. **Attack** — recurring threat-generating plans (`threats/` #123 +
     `plans.rs` ADR-0017). Shipped in #142: `study_gen/attack.rs` reuses the
     `plans.rs` PV tracer to detect a pawn storm (same-colour pawn pushed
     `>= min_advances` times, finishing within `king_zone_files` of the enemy
     king). The spine walk runs it on the opponent's best line at each searched
     position; a storm toward *our* king is the lowest-priority signal and tags
     the move that conceded it as **Caution**. The heuristic for the opponent's
     *tempting* reply stays open (surfaced via chat, below) — later landed as
     the PV2 proxy described in the issue #139/#176 updates.

- **Roles.** Each kept line is tagged **Weapon** (recommend — must pass the
  bounded-downside test), **Caution** (warn — included *because* its eval is
  bad), or **Off-book** (a reachability break).

- **"Tempting" lines** are not auto-detected by heuristic in v1 — they are
  surfaced **via chat with the embedded study assistant** (#20), which drives the
  same tools under per-step approval. Heuristic baiting detection is left open
  for v1; the spine walk (issue #139, below) is where it actually lands.

- **Compute** is budgeted as **movetime per variation** (not depth), clamped by
  the existing `MAX_MOVETIME_MS`.

- **Thresholds** start at `downside_floor_cp = -80`, `baited_upside_cp = 150`,
  `only_move_gap_cp = 120`; trap/blunder magnitudes align with `review/`
  (`BLUNDER_CP = 200`, `MISTAKE_CP = 100`). They are deliberately easy to retune.

**Layering.** The classification core is pure and I/O-free — `src/study_gen/danger.rs`
takes already-perspective-normalised centipawn evals (our POV) and decides kind +
role, reusing `tree::score_to_cp`. The spine walk, the `analyse_multi` driver, the
annotation/verify pass (`annotate.rs`, reused unchanged), the MCP tool and the
HTTP route are thin callers, per the architecture rule. The verify pass already
adjudicates "wins/loses material / blunder", which is exactly the check a tagged
trap line needs.

**Consequences.** A clean second mode beside the best-line builder; the engine is
used where it is strong (refuting candidate moves) instead of where it is
redundant (re-stating its own top line). The danger map is a metric the engine
can *compute* but would never *optimise for* — which is precisely why a study
built on it is worth more than the best-line tree. Attack detection landed in
#142; a heuristic for "tempting" replies remains the open follow-up (issue #131).
v1 ships the pure classifier (`danger.rs`) with the spine driver, annotation
wiring, and transport layered on in later increments. Transport landed in #141:
the MCP `generate_danger_map` tool and `POST /api/studies/generate-danger-map`
(`studies/danger_route.rs`) are thin callers over `generate_danger_study_live` —
the request carries the spine as PGN, a per-variation `movetime_ms`/`multipv`
budget, and partial `SpineConfig`/`DangerConfig`/`AttackConfig` overrides (all
`serde(default)`).

> **Update (ADR-0027).** The MCP `generate_danger_map` tool was removed: an MCP
> tool must not run an LLM loop internally. The engine-adjudicated `DangerTree` is
> now exposed as the data-only `danger_map` MCP tool, with annotation done by the
> client; the `generate_danger_study_live` orchestrator stays behind
> `POST /api/studies/generate-danger-map`.

> **Update (issue #175).** The trap test's `if_refuted` was only ever the
> **shallow root eval** after the opponent's best reply — never a check that
> *our* position, once that reply is actually played, still holds. A refuted
> trap could pass the `downside_floor_cp` floor on a movetime-limited search and
> still get recommended as a **Weapon**, the single most dangerous false
> positive the mode can produce ("Weapon" reads as safe-to-play). Fixed by
> confirming every `Weapon` candidate one ply deeper: `spine.rs`'s
> `resolve_trap` applies the PV's first move (the opponent's best reply) and
> runs one more `analyse_multi`, then `danger.rs`'s `confirm_weapon` downgrades
> to `HopeChess`/Caution when that follow-up eval is below a new
> `follow_up_floor_cp` (default `-200`, aligned with `BLUNDER_CP`). A missing
> follow-up (no PV, unparseable position, or the search returns nothing) leaves
> the shallow verdict unchanged — nothing on hand to reveal a refutation.

> **Update (issue #176).** Three follow-ups, none of which required retuning
> the `-80`/`+150`/`120`/`30%` thresholds themselves:
>
> 1. **PV2 was the entire "tempting reply" proxy**, and PV2 is an engine
>    ranking, not a human one — it says nothing about whether anyone would
>    actually be drawn to that move (material grabs, natural development,
>    fashion). `spine.rs`'s `resolve_trap` now weights the bait by its real
>    frequency among the DB continuation stats the walk already fetches for
>    reachability/miss-rate (`ContinuationSource`, same `min_frequency` floor
>    used to decide whether a reply is worth expanding at all, default `0.02`):
>    a `Weapon`/`HopeChess` verdict whose tempting move nobody in the corpus
>    ever played downgrades to `Quiet` before the one-ply-deeper confirmation
>    even runs. A reply the engine ranks highly but no human plays cannot bait
>    anyone in practice.
> 2. **The mate-only / single-legal-reply edge case is now explicit and
>    tested**, not just implicit in an early return. When `analyse_multi`
>    surfaces fewer than two lines — a forced mate the search stops expanding
>    past, or a position with exactly one legal reply — there is no second
>    candidate for the opponent to be tempted by, so `resolve_trap` returns
>    `None`: no trap verdict at all (not `Quiet`, which would misreport a real
>    absence-of-data as a considered "nothing special here"). The move may
>    still pick up an `only_move`/`attack` tag from `classify`'s other signals;
>    it just carries no trap figure.
> 3. **`max_replies` rose from `4` to `6`** — 1.e4 c5 alone has four or more
>    mainstream continuations, so the old cap could drop a mainstream reply
>    before the walk ever got to judge it.
>
> **Backtesting against a human game corpus remains unmeasured.** The proposal
> to pick thresholds by replaying real games (Lichess exports) and measuring
> how often a flagged move was actually played, and how the game went, needs an
> engine binary and a sizeable imported corpus to run — infrastructure this
> change does not add. The FE danger overlay is labelled **"experimental"**
> (`DangerMapPanel.vue`) until that measurement pass happens; treat the four
> thresholds above as reasoned defaults, not validated ones.
