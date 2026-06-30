//! Unit tests for the danger-map route DTOs: the request body defaults and the
//! partial `SpineConfig` override semantics (issue #141). The full happy path
//! needs a real engine + LLM, so it is covered at the service layer with injected
//! fakes (`study_gen::danger_generate` tests) and the mounted-route 503 path lives
//! in `tests/studies.rs`.

use super::*;
use crate::study_gen::spine::Side;

#[test]
fn body_defaults_when_only_required_fields_are_given() {
    let body: DangerMapBody = serde_json::from_value(serde_json::json!({
        "database_id": 7,
        "name": "Najdorf traps",
        "spine_pgn": "1. e4 c5 *",
    }))
    .expect("minimal body deserializes");

    assert_eq!(body.database_id, 7);
    assert!(!body.global);
    assert!(body.start_fen.is_none());
    assert!(body.model.is_none());
    assert!(body.movetime_ms.is_none());
    assert!(body.multipv.is_none());
    // An omitted `spine` falls back to the full defaults.
    assert_eq!(body.spine, SpineConfig::default());
}

#[test]
fn spine_accepts_partial_overrides_keeping_other_defaults() {
    let body: DangerMapBody = serde_json::from_value(serde_json::json!({
        "database_id": 1,
        "name": "Black weapons",
        "spine_pgn": "1. e4 e5 *",
        "spine": {
            "our_side": "Black",
            "max_depth": 12,
            "danger": { "only_move_gap_cp": 200 }
        }
    }))
    .expect("partial spine overrides deserialize");

    let defaults = SpineConfig::default();
    // Overridden fields take the request value...
    assert_eq!(body.spine.our_side, Side::Black);
    assert_eq!(body.spine.max_depth, 12);
    assert_eq!(body.spine.danger.only_move_gap_cp, 200);
    // ...while everything else (including the rest of `danger`) keeps defaults.
    assert_eq!(body.spine.min_frequency, defaults.min_frequency);
    assert_eq!(body.spine.max_replies, defaults.max_replies);
    assert_eq!(
        body.spine.danger.downside_floor_cp,
        defaults.danger.downside_floor_cp
    );
    assert_eq!(body.spine.attack, defaults.attack);
}
