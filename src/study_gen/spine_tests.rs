//! Unit tests for the danger-map spine walk (#139). The walk is driven by two
//! injected fakes — a multi-PV analyzer and a continuation source keyed by FEN —
//! so no engine or DB is needed. The spine is a hand-built [`MoveTree`].

use super::*;

use std::collections::HashMap;

use async_trait::async_trait;

use crate::engine::{Analysis, Score};
use crate::pgn_tree::MoveTree;
use crate::position::{replay, STARTPOS_FEN};
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
    assert_eq!(e5.tag.unwrap().kind, DangerKind::OffBook);
    assert_eq!(e5.tag.unwrap().role, DangerRole::OffBook);
    // The off-book node is a leaf: the repertoire has nothing beyond it.
    assert!(e5.children.is_empty());
}

#[tokio::test]
async fn weapon_trap_tagged_on_our_move() {
    // After 1.e4, Black's best (c5) keeps Black only −10 (our downside bounded),
    // but the tempting e5 drops Black to −300 (our baited upside): a weapon.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", -10), line("e7e5", -300)],
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

    // The danger is judged on *our* move (e4), not the opponent position.
    let e4 = node_by_san(&tree, "e4");
    let tag = e4.tag.expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(tag.role, DangerRole::Weapon);
    assert_eq!(tag.trap, Some(TrapVerdict::Weapon));
}

#[tokio::test]
async fn refuted_bait_tagged_caution() {
    // Black's best (c5) refutes us hard (Black +200 ⇒ our −200, below the floor),
    // yet the second line still baits: hope-chess → Caution, never recommended.
    let mut an = HashMap::new();
    an.insert(
        fen_after(&["e4"]),
        vec![line("c7c5", 200), line("e7e5", -200)],
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

    let tag = node_by_san(&tree, "e4").tag.expect("e4 is tagged");
    assert_eq!(tag.kind, DangerKind::Trap);
    assert_eq!(tag.role, DangerRole::Caution);
    assert_eq!(tag.trap, Some(TrapVerdict::HopeChess));
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

    let tag = node_by_san(&tree, "e4").tag.expect("e4 is tagged");
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
        node_by_san(&tree, "d4").tag.unwrap().kind,
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
