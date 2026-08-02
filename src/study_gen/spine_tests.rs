//! Unit tests for the danger-map spine walk (#139). The walk is driven by two
//! injected fakes — a multi-PV analyzer and a continuation source keyed by FEN —
//! so no engine or DB is needed. The spine is a hand-built [`MoveTree`].

use super::*;

use std::collections::HashMap;

use async_trait::async_trait;

use crate::engine::{Analysis, Score};
use crate::pgn_tree::MoveTree;
use crate::position::{apply_san, replay, STARTPOS_FEN};
use crate::search::report::MoveReport;

const STD: CastlingMode = CastlingMode::Standard;

// --- Fakes ----------------------------------------------------------------

struct FakeAnalyzer(HashMap<String, Vec<Analysis>>);
struct FakeStats(HashMap<String, Vec<MoveReport>>);

#[async_trait]
impl MultiAnalyzer for FakeAnalyzer {
    async fn analyse_multi(&self, fen: &str) -> anyhow::Result<Vec<Analysis>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

#[async_trait]
impl ContinuationSource for FakeStats {
    async fn continuations(&self, fen: &str) -> anyhow::Result<Vec<MoveReport>> {
        Ok(self.0.get(fen).cloned().unwrap_or_default())
    }
}

// --- Builders -------------------------------------------------------------

fn fen_after(sans: &[&str]) -> String {
    if sans.is_empty() {
        return STARTPOS_FEN.to_string();
    }
    replay(STARTPOS_FEN, sans, STD)
        .unwrap()
        .last()
        .unwrap()
        .fen
        .clone()
}

/// One MultiPV line: a best move (UCI) and its score from the side-to-move's
/// perspective. The PV is irrelevant to the classifier, so it is left minimal.
fn line(uci: &str, cp: i32) -> Analysis {
    Analysis {
        bestmove: uci.to_string(),
        ponder: None,
        score: Some(Score::Cp { value: cp }),
        depth: None,
        pv: vec![uci.to_string()],
    }
}

fn report(san: &str, frequency: f64) -> MoveReport {
    MoveReport {
        san: san.to_string(),
        count: (frequency * 100.0) as u64,
        white: 0,
        draws: 0,
        black: 0,
        frequency,
        score: 0.5,
    }
}

/// A White repertoire: 1.e4, prepared only against 1...c5.
fn white_spine() -> MoveTree {
    let mut t = MoveTree::new();
    let e4 = t.add_move(t.root, "e4");
    let c5 = t.add_move(e4, "c5");
    t.add_move(c5, "Nf3");
    t
}

fn cfg() -> SpineConfig {
    SpineConfig {
        max_depth: 2,
        ..SpineConfig::default()
    }
}

/// Find the (single) node reached by `san` from the root.
fn node_by_san<'a>(tree: &'a DangerTree, san: &str) -> &'a DangerNode {
    tree.nodes
        .iter()
        .find(|n| n.san.as_deref() == Some(san))
        .unwrap_or_else(|| panic!("no node for {san}"))
}

// --- Tests ----------------------------------------------------------------

#[tokio::test]
async fn offbook_reply_tagged_onbook_reply_recurses() {
    // After 1.e4: c5 is prepared (on-book), e5 is not (off-book).
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.5), report("e5", 0.4)],
    );
    // A neutral search after e4: best c5, second e5 — nothing dangerous.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -20)],
    );

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let c5 = node_by_san(&tree, "c5");
    assert!(c5.tag.is_none(), "a prepared reply is plain, not tagged");

    let e5 = node_by_san(&tree, "e5");
    let e5_tag = e5.tag.as_ref().expect("e5 is off-book");
    assert_eq!(e5_tag.kind, DangerKind::OffBook);
    assert_eq!(e5_tag.role, DangerRole::OffBook);
    // The off-book node is a leaf: the repertoire has nothing beyond it.
    assert!(e5.children.is_empty());
}

#[tokio::test]
async fn db_suffixed_reply_matches_the_spines_unsuffixed_prepared_move() {
    // The DB stores normalized SAN with a trailing +/# (position.rs), while the
    // spine keeps the PGN author's own spelling. An exact-string match would
    // wrongly call the user's own prepared c5 "Off-book" and never walk its
    // subtree (issue #194) — shakmaty's SAN parser doesn't validate the
    // suffix, so `c5+` still applies as the real c5 move.
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5+", 0.5), report("e5", 0.4)],
    );
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -20)],
    );
    // Deep enough to also walk one ply past c5 (Nf3), proving the subtree is
    // actually followed, not just matched.
    let config = SpineConfig {
        max_depth: 3,
        ..SpineConfig::default()
    };

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &config,
        STD,
    )
    .await
    .unwrap();

    let c5 = node_by_san(&tree, "c5+");
    assert!(
        c5.tag.is_none(),
        "the prepared reply must not be Off-book despite the DB's +/# suffix"
    );
    assert!(
        !c5.children.is_empty(),
        "the spine's own follow-up (Nf3) must still be walked, not stranded"
    );
}

#[tokio::test]
async fn prepared_answer_with_no_db_games_still_walked() {
    // The DB has no games at all reaching a reply the repertoire prepares for
    // (a personal spine covering a move nobody in the corpus has played yet).
    // It must still appear in the tree — walked as on-book — instead of
    // silently vanishing because it never made the `kept` list (issue #194).
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -20)],
    );
    let stats = HashMap::new(); // no continuations recorded anywhere

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let c5 = node_by_san(&tree, "c5");
    assert!(
        c5.tag.is_none(),
        "a prepared reply with zero DB games must not be Off-book"
    );
}

#[tokio::test]
async fn weapon_trap_tagged_on_our_move() {
    // After 1.e4, Black's best (c5) keeps Black only −10 (our downside bounded),
    // but the tempting e5 drops Black to −300 (our baited upside): a weapon.
    // Humans actually play e5 30% of the time, clearing the bait-frequency gate
    // (#176) — a real trap, not just an engine-only proxy.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -300)],
    );
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.5), report("e5", 0.3)],
    );

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    // The danger is judged on *our* move (e4), not the opponent position.
    let e4 = node_by_san(&tree, "e4");
    let tag = e4.tag.as_ref().expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(tag.role, DangerRole::Weapon);
    assert_eq!(tag.trap, Some(TrapVerdict::Weapon));
}

#[tokio::test]
async fn weapon_refuted_one_ply_deeper_downgrades_to_caution() {
    // After 1.e4, Black's best (c5) keeps Black only −10 (our downside bounded
    // by the shallow root eval) and the tempting e5 drops Black to −300 (our
    // baited upside): the shallow test alone would call this a Weapon. But once
    // Black actually plays 1...c5, our own follow-up is −350 — well past the
    // refutation floor — so the trap is refuted one ply deeper than the root
    // search looked (issue #175) and must downgrade to Caution, not read as
    // safe-to-play.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -300)],
    );
    an.insert(fen_after(&["e4", "c5"]), vec![line("g1f3", -350)]);
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.5), report("e5", 0.3)],
    );

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let e4 = node_by_san(&tree, "e4");
    let tag = e4.tag.as_ref().expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(
        tag.role,
        DangerRole::Caution,
        "a refuted trap must never read as a recommended Weapon"
    );
    assert_eq!(tag.trap, Some(TrapVerdict::HopeChess));
}

#[tokio::test]
async fn refuted_bait_tagged_caution() {
    // Black's best (c5) refutes us hard (Black +200 ⇒ our −200, below the floor),
    // yet the second line still baits and humans do play it: hope-chess →
    // Caution, never recommended.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", 200), line("e7e5", -200)],
    );
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.5), report("e5", 0.3)],
    );

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let node = node_by_san(&tree, "e4");
    let tag = node.tag.as_ref().expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(tag.role, DangerRole::Caution);
    assert_eq!(tag.trap, Some(TrapVerdict::HopeChess));
}

#[tokio::test]
async fn bait_nobody_plays_is_not_a_weapon() {
    // Same shape as a would-be weapon (downside bounded at −50, tempting reply
    // baits well past the upside floor) but the gap (110cp) stays under the
    // only-move threshold, so nothing else would tag this move — and nobody in
    // the DB has ever played the tempting reply. PV2 is just an engine proxy for
    // "tempting"; without a human ever choosing it, it is not a practical trap
    // (#176), so the verdict is downgraded to Quiet and the move goes untagged.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -50), line("e7e5", -160)],
    );
    let mut stats = HashMap::new();
    // c5 (the actually-played best reply) is on the board; e5 never appears.
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.9)]);

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    assert!(
        node_by_san(&tree, "e4").tag.is_none(),
        "a bait no human plays must not be reported as a trap"
    );
}

#[tokio::test]
async fn rare_bait_clearing_min_frequency_still_counts() {
    // Same eval shape as `bait_nobody_plays_is_not_a_weapon`, but the tempting
    // reply is on record at exactly `min_frequency` (2%, the default floor for
    // "a human plays this at all") — enough to count as a real, if rare, bait.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -50), line("e7e5", -160)],
    );
    let mut stats = HashMap::new();
    stats.insert(
        fen_after(&["e4"]),
        vec![report("c5", 0.9), report("e5", 0.02)],
    );

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let tag = node_by_san(&tree, "e4").tag.as_ref().expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(tag.trap, Some(TrapVerdict::Weapon));
}

#[tokio::test]
async fn single_reply_position_yields_no_trap_verdict() {
    // The engine finds only one reasonable reply (a forced mate search that
    // stops expanding past the mating line, or a literal one-legal-move
    // position) — no second line for the opponent to be tempted by. The
    // asymmetric refutation test needs two candidates to compare, so this must
    // not be misread as an unbaited Quiet trap; it simply carries no trap
    // verdict at all (#176). With no only-move gap or attack signal available
    // either, the move goes untagged rather than defaulting to some verdict.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![Analysis {
            bestmove: "c7c5".to_string(),
            ponder: None,
            score: Some(Score::Mate { value: -3 }), // forced mate against Black
            depth: None,
            pv: vec!["c7c5".to_string()],
        }],
    );
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 1.0)]);

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    assert!(node_by_san(&tree, "e4").tag.is_none());
}

#[tokio::test]
async fn only_move_weapon_when_humans_miss_it() {
    // A wide gap (best c5 +50, second −100 ⇒ gap 150) but no trap bait (our
    // baited upside is only +100 < 150 ⇒ Quiet). Humans play the best move only
    // half the time, so the narrow path is a practical weapon.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", 50), line("e7e5", -100)],
    );
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.5)]);

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    let node = node_by_san(&tree, "e4");
    let tag = node.tag.as_ref().expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::OnlyMove);
    assert_eq!(tag.role, DangerRole::Weapon);
    assert_eq!(tag.miss_rate, Some(0.5));
    assert_eq!(tag.only_move_gap, Some(150));
}

#[tokio::test]
async fn only_move_not_a_weapon_when_humans_find_it() {
    // Same wide gap, but humans play the only move 90% of the time: not a
    // practical weapon, so no tag.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", 50), line("e7e5", -100)],
    );
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.9)]);

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &cfg(),
        STD,
    )
    .await
    .unwrap();

    assert!(node_by_san(&tree, "e4").tag.is_none());
}

#[tokio::test]
async fn black_repertoire_expands_opponent_first_and_never_tags_root() {
    // A Black repertoire: White moves first. We answer 1.e4 but not 1.d4.
    let mut spine = MoveTree::new();
    let e4 = spine.add_move(spine.root, "e4");
    spine.add_move(e4, "c5");

    let mut stats = HashMap::new();
    stats.insert(
        STARTPOS_FEN.to_string(),
        vec![report("e4", 0.6), report("d4", 0.3)],
    );

    let config = SpineConfig {
        our_side: Side::Black,
        max_depth: 1,
        ..SpineConfig::default()
    };

    let tree = walk_danger_spine(
        &FakeAnalyzer(HashMap::new()),
        &FakeStats(stats),
        &spine,
        STARTPOS_FEN,
        &config,
        STD,
    )
    .await
    .unwrap();

    // The move-less root is never tagged, even though it is an opponent position.
    assert!(tree.nodes[tree.root].tag.is_none());
    // The prepared opponent move is on-book (plain); the unprepared one is off-book.
    assert!(node_by_san(&tree, "e4").tag.is_none());
    assert_eq!(
        node_by_san(&tree, "d4").tag.as_ref().unwrap().kind,
        DangerKind::OffBook
    );
}

#[tokio::test]
async fn max_depth_bounds_the_walk() {
    let mut stats = HashMap::new();
    stats.insert(fen_after(&["e4"]), vec![report("c5", 0.5)]);
    let config = SpineConfig {
        max_depth: 1,
        ..SpineConfig::default()
    };

    let tree = walk_danger_spine(
        &FakeAnalyzer(HashMap::new()),
        &FakeStats(stats),
        &white_spine(),
        STARTPOS_FEN,
        &config,
        STD,
    )
    .await
    .unwrap();

    // Root (ply 0) + e4 (ply 1) only; the opponent replies at ply 2 are cut.
    assert_eq!(tree.nodes.len(), 2);
    assert!(tree.nodes.iter().all(|n| n.ply <= 1));
}

#[tokio::test]
async fn attack_pawn_storm_tagged_caution_on_our_move() {
    // A quiet middlegame: both kings castled short, only the f/g/h pawns left.
    // Our spine plays 1.Kf1; the engine's best reply for Black is a g-pawn storm
    // (g5-g4-g3) toward our king — no trap, no narrow path, so the attack signal
    // is what fires, flagging Kf1 as a Caution.
    let start = "6k1/5ppp/8/8/8/8/5PPP/6K1 w - - 0 1";
    let after_kf1 = apply_san(start, "Kf1", STD).unwrap().0;

    let mut spine = MoveTree::new();
    spine.add_move(spine.root, "Kf1");

    // Neutral evals (Black +50 best, 0 second) ⇒ Quiet trap, gap 50 < 120 ⇒ not
    // an only-move. The best line is the storm.
    let storm = Analysis {
        bestmove: "g7g5".to_string(),
        ponder: None,
        score: Some(Score::Cp { value: 50 }),
        depth: None,
        pv: vec![
            "g7g5".to_string(),
            "f1g1".to_string(),
            "g5g4".to_string(),
            "g1f1".to_string(),
            "g4g3".to_string(),
        ],
    };
    let mut an = HashMap::new();
    an.insert(after_kf1.clone(), vec![storm, line("f7f5", 0)]);
    let mut stats = HashMap::new();
    stats.insert(after_kf1, vec![report("g5", 0.5)]);

    let config = SpineConfig {
        max_depth: 2,
        ..SpineConfig::default()
    };

    let tree = walk_danger_spine(
        &FakeAnalyzer(an),
        &FakeStats(stats),
        &spine,
        start,
        &config,
        STD,
    )
    .await
    .unwrap();

    let node = node_by_san(&tree, "Kf1");
    let tag = node.tag.as_ref().expect("Kf1 is tagged");
    assert_eq!(tag.kind, DangerKind::Attack);
    assert_eq!(tag.role, DangerRole::Caution);
    let attack = tag.attack.as_ref().expect("storm recorded");
    assert_eq!(attack.pawn, 'p', "Black's storming pawn");
    assert_eq!(attack.path, vec!["g7", "g5", "g4", "g3"]);
    assert_eq!(attack.advances, 3);
}

#[test]
fn miss_rate_matches_db_suffixed_san_to_a_checking_best_move() {
    // `uci_to_san` never emits a check/mate suffix, but the DB's reported SAN
    // does — an exact-string match would treat every checking best move as
    // "nobody ever plays it" and inflate `miss_rate` to 1.0 (issue #194),
    // manufacturing bogus only-move Weapons.
    let best = line("c7c5", 0);
    let replies = vec![report("c5+", 0.7)];
    let fen = fen_after(&["e4"]);

    let miss = miss_rate(&best, &replies, &fen, STD).expect("best move maps to a SAN");
    assert!(
        (miss - 0.3).abs() < 1e-9,
        "expected miss_rate 0.3 (1 - 0.7 played), got {miss}"
    );
}

#[tokio::test]
async fn bait_frequency_matches_db_suffixed_san_for_a_checking_bait() {
    // Same asymmetry as `miss_rate`, but for the trap test's tempting-reply
    // lookup (#176's bait-frequency gate): a checking bait stored in the DB as
    // `e5+` must still be found by its real frequency, not fall back to 0.0
    // and get downgraded to Quiet (issue #194).
    let fen = fen_after(&["e4"]);
    let lines = vec![line("c7c5", -10), line("e7e5", -300)];
    let replies = vec![report("c5", 0.5), report("e5+", 0.3)];
    let config = SpineConfig::default();

    let verdict = resolve_trap(
        &lines,
        &FakeAnalyzer(HashMap::new()),
        &fen,
        &replies,
        &config,
        STD,
    )
    .await
    .unwrap();

    assert_eq!(
        verdict,
        Some(TrapVerdict::Weapon),
        "the checking bait's real 30% DB frequency clears min_frequency and must not downgrade to Quiet"
    );
}

#[tokio::test]
async fn invalid_start_fen_errors() {
    let err = walk_danger_spine(
        &FakeAnalyzer(HashMap::new()),
        &FakeStats(HashMap::new()),
        &white_spine(),
        "not a fen",
        &cfg(),
        STD,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, SpineError::InvalidFen(_)));
}
