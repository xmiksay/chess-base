//! Unit tests for the pure danger-map classifier (ADR-0026).

use super::*;
use crate::engine::Score;

fn cp(value: i32) -> Option<Score> {
    Some(Score::Cp { value })
}

// --- trap_verdict -------------------------------------------------------------

#[test]
fn weapon_when_downside_bounded_and_upside_baits() {
    let cfg = DangerConfig::default();
    // Refuted: -50 (above the -80 floor); baited: +300 (well past +150).
    assert_eq!(trap_verdict(-50, 300, &cfg), TrapVerdict::Weapon);
}

#[test]
fn hope_chess_when_baits_but_refutation_loses() {
    let cfg = DangerConfig::default();
    // The user's principle: a move that wins if the opponent errs but is lost to
    // the best reply is NOT a trap — it is a blunder wearing a trap's clothes.
    assert_eq!(trap_verdict(-250, 400, &cfg), TrapVerdict::HopeChess);
}

#[test]
fn quiet_when_no_baiting_upside() {
    let cfg = DangerConfig::default();
    // Sound but dull: even the tempting reply keeps the opponent fine.
    assert_eq!(trap_verdict(-30, 20, &cfg), TrapVerdict::Quiet);
    // No upside outranks downside: not-baited is checked first.
    assert_eq!(trap_verdict(-500, 0, &cfg), TrapVerdict::Quiet);
}

#[test]
fn trap_thresholds_are_inclusive_boundaries() {
    let cfg = DangerConfig::default();
    // Exactly on the floor and exactly on the upside target → weapon.
    assert_eq!(
        trap_verdict(cfg.downside_floor_cp, cfg.baited_upside_cp, &cfg),
        TrapVerdict::Weapon
    );
    // One centipawn under the floor flips weapon → hope-chess.
    assert_eq!(
        trap_verdict(cfg.downside_floor_cp - 1, cfg.baited_upside_cp, &cfg),
        TrapVerdict::HopeChess
    );
    // One centipawn under the upside target → quiet.
    assert_eq!(
        trap_verdict(0, cfg.baited_upside_cp - 1, &cfg),
        TrapVerdict::Quiet
    );
}

#[test]
fn trap_respects_custom_config() {
    // A conservative repertoire: insist on near-equality if refuted.
    let cfg = DangerConfig {
        downside_floor_cp: -20,
        baited_upside_cp: 100,
        only_move_gap_cp: 120,
    };
    assert_eq!(trap_verdict(-50, 300, &cfg), TrapVerdict::HopeChess);
    assert_eq!(trap_verdict(-10, 300, &cfg), TrapVerdict::Weapon);
}

// --- only_move_gap / is_only_move ---------------------------------------------

#[test]
fn gap_is_difference_of_the_two_lines() {
    // Best +60, second -90 → gap 150.
    assert_eq!(only_move_gap(cp(60), cp(-90)), Some(150));
}

#[test]
fn no_second_line_means_no_gap() {
    assert_eq!(only_move_gap(cp(60), None), None);
    // The literal one-legal-move case is the orchestrator's job, not ours.
    assert!(!is_only_move(cp(60), None, &DangerConfig::default()));
}

#[test]
fn only_move_when_gap_clears_threshold() {
    let cfg = DangerConfig::default(); // 120
    assert!(is_only_move(cp(50), cp(-80), &cfg)); // gap 130
    assert!(!is_only_move(cp(50), cp(-50), &cfg)); // gap 100
    // Exactly on the threshold counts.
    assert!(is_only_move(cp(0), cp(-120), &cfg)); // gap 120
}

#[test]
fn mate_dominates_the_gap() {
    // Best is mate, second is a quiet +50: the gap is enormous, so it is the
    // opponent's only move by a mile.
    let cfg = DangerConfig::default();
    let best = Some(Score::Mate { value: 2 });
    assert!(is_only_move(best, cp(50), &cfg));
    assert!(only_move_gap(best, cp(50)).unwrap() > cfg.only_move_gap_cp);
}

#[test]
fn missing_best_score_treated_as_neutral() {
    // score_to_cp(None) == 0, so a missing best vs a losing second still yields a
    // positive, finite gap rather than a panic or overflow.
    assert_eq!(only_move_gap(None, cp(-200)), Some(200));
}
