//! Integration test for the UCI engine manager, gated on a real engine binary.
//!
//! Set `CHESS_BASE_TEST_ENGINE` to an engine path (e.g. `stockfish`) to run it;
//! the test is skipped (passes trivially) when the variable is unset, so CI and
//! contributors without an engine installed aren't blocked.
//!
//!     CHESS_BASE_TEST_ENGINE=$(which stockfish) cargo test --test engine

use std::collections::BTreeMap;

use chess_base::engine::{AnalysisEvent, Engine, EngineConfig, EngineService, Limits};
use chess_base::position::STARTPOS_FEN;

fn engine_path() -> Option<String> {
    match std::env::var("CHESS_BASE_TEST_ENGINE") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => {
            eprintln!("skipping: set CHESS_BASE_TEST_ENGINE to a UCI engine binary to run");
            None
        }
    }
}

#[tokio::test]
async fn analyses_startpos_streaming_increasing_depth_then_bestmove() {
    let Some(path) = engine_path() else { return };

    let mut engine = Engine::spawn(EngineConfig::new("test", path))
        .await
        .expect("engine should spawn and complete the UCI handshake");

    // MultiPV is a normal reconfigure between handshake and search.
    engine.set_option("MultiPV", "2").await.unwrap();
    engine.wait_ready().await.unwrap();

    engine.set_position(STARTPOS_FEN).await.unwrap();
    engine.go(&Limits::depth(12)).await.unwrap();

    let mut max_depth = 0u8;
    let mut info_count = 0usize;
    let mut saw_pv = false;
    let mut best_move = None;

    while let Some(event) = engine.next_event().await.unwrap() {
        match event {
            AnalysisEvent::Info(info) => {
                info_count += 1;
                if let Some(d) = info.depth {
                    // Depth is monotonically non-decreasing as the search refines.
                    assert!(d >= max_depth, "depth went backwards: {d} < {max_depth}");
                    max_depth = d;
                }
                if !info.pv.is_empty() {
                    saw_pv = true;
                }
            }
            AnalysisEvent::BestMove { best_move: bm, .. } => {
                best_move = Some(bm);
                break;
            }
        }
    }

    assert!(info_count > 0, "expected streamed info lines");
    assert!(saw_pv, "expected at least one info line with a PV");
    assert!(max_depth >= 1, "expected the search to report depth");
    let bm = best_move.expect("search must end with a bestmove");
    assert!(!bm.is_empty(), "bestmove should name a move");

    engine.quit().await.unwrap();
}

#[tokio::test]
async fn stop_ends_an_infinite_search() {
    let Some(path) = engine_path() else { return };

    let mut engine = Engine::spawn(EngineConfig::new("test", path))
        .await
        .unwrap();
    engine.set_position(STARTPOS_FEN).await.unwrap();
    engine.go(&Limits::default()).await.unwrap(); // go infinite

    // Let it think briefly, then stop; we must still get a terminal bestmove.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    engine.stop().await.unwrap();

    let mut got_bestmove = false;
    while let Some(event) = engine.next_event().await.unwrap() {
        if let AnalysisEvent::BestMove { .. } = event {
            got_bestmove = true;
            break;
        }
    }
    assert!(got_bestmove, "stop should yield a final bestmove");

    engine.quit().await.unwrap();
}

/// The direct in-process facade the batch pipeline uses: `analyse` returns a
/// flat eval/PV/bestmove with no LLM involvement, and the pool reuses its engine
/// across calls.
#[tokio::test]
async fn service_analyses_a_position_and_reuses_the_pool() {
    let Some(path) = engine_path() else { return };

    let service = EngineService::new(EngineConfig::new("test", path), 1);
    let limits = Limits::depth(10);
    let opts = BTreeMap::new();

    let first = service
        .analyse(STARTPOS_FEN, &limits, &opts)
        .await
        .expect("batch analysis should succeed");
    assert!(!first.bestmove.is_empty(), "a best move is required");
    assert!(first.score.is_some(), "expected an evaluation");
    assert!(first.depth.unwrap_or(0) >= 1, "expected a reported depth");

    // A second call must reuse the pooled engine (no double-spawn) and still work.
    let second = service
        .analyse(STARTPOS_FEN, &limits, &opts)
        .await
        .expect("reused-engine analysis should succeed");
    assert!(!second.bestmove.is_empty());

    service.shutdown().await;
}
