//! Engine facade: one pooled UCI service, two consumption paths (ADR 0014).
//!
//! [`EngineService`] wraps the Epic 5 [`Engine`] process manager in a small,
//! bounded pool and exposes a single one-shot operation,
//! [`EngineService::analyse`], that runs a bounded search to completion and
//! returns a flat [`Analysis`] (eval / pv / bestmove). One pool backs **two
//! facades**:
//!
//! - the **batch pipeline** calls `analyse` directly in-process — the returned
//!   eval/PV is plain Rust data that never enters any LLM context (ADR 0009);
//! - the **MCP endpoint** registers an `engine_analyse` tool that calls the very
//!   same service for interactive analysis.
//!
//! The streaming WebSocket (`server/engine_ws.rs`) keeps its own per-socket
//! engine: it needs incremental `info` updates and a mid-search `stop`, which
//! the one-shot pool deliberately does not model.

use std::collections::BTreeMap;

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

use super::analysis::{AnalysisEvent, AnalysisInfo, Score};
use super::command::Limits;
use super::manager::Engine;
use super::EngineConfig;

/// Search depth applied when a caller passes fully-unbounded limits, so a
/// one-shot `analyse` always terminates instead of searching forever. Public so
/// the MCP analysis tools document — and share — the *one* default depth that
/// every "explain this position/game" path falls back to (issue #125).
pub const DEFAULT_DEPTH: u32 = 20;

/// Hard ceiling on a single one-shot search when no `movetime` bounds it (a
/// depth/nodes search). The backstop against a stuck or pathologically slow
/// engine pinning the shared single-permit pool forever (issue #93).
const SEARCH_CEILING: Duration = Duration::from_secs(60);

/// Grace added on top of an explicit `movetime` budget before the overall
/// deadline fires, covering engine wind-down after it should have stopped.
const MOVETIME_GRACE: Duration = Duration::from_secs(5);

/// The distilled result of a one-shot search: the engine's best move plus the
/// evaluation and principal variation of the primary line. Flat and
/// `Serialize`, so the batch pipeline and the MCP tool share one shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    /// Best move in UCI long-algebraic notation (`e2e4`).
    pub bestmove: String,
    /// Ponder move the engine suggests, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ponder: Option<String>,
    /// Evaluation of the primary line, from the side-to-move's perspective.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<Score>,
    /// Depth (plies) the reported line reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
    /// Principal variation of the primary line (UCI moves).
    pub pv: Vec<String>,
}

impl Analysis {
    /// Assemble the result from the best primary-PV `info` seen and the terminal
    /// `bestmove`. Pure, so the event folding is unit-testable without a process.
    fn from_search(
        primary: Option<AnalysisInfo>,
        best_move: String,
        ponder: Option<String>,
    ) -> Self {
        let primary = primary.unwrap_or_default();
        Self {
            bestmove: best_move,
            ponder,
            score: primary.score,
            depth: primary.depth,
            pv: primary.pv,
        }
    }
}

/// Keep `info` only if it refines the primary (MultiPV 1) line. Engines emit
/// secondary PVs (MultiPV 2, 3, …) and shallow chatter we ignore for the
/// distilled result.
fn fold_primary(prev: Option<AnalysisInfo>, info: AnalysisInfo) -> Option<AnalysisInfo> {
    if info.multipv.unwrap_or(1) == 1 {
        Some(info)
    } else {
        prev
    }
}

/// Clamp user-supplied limits to their safe maxima, then substitute a default
/// depth for fully-unbounded limits so a one-shot search always terminates.
/// Clamping here protects the shared pool from *every* caller, not just the MCP
/// tools (issue #93).
fn bounded(limits: &Limits) -> Limits {
    let limits = limits.clone().clamped();
    if limits.depth.is_none() && limits.movetime_ms.is_none() && limits.nodes.is_none() {
        Limits::depth(DEFAULT_DEPTH)
    } else {
        limits
    }
}

/// The overall wall-clock deadline for one search: a movetime budget plus a
/// grace margin, or the fixed [`SEARCH_CEILING`] for depth/nodes searches.
fn search_deadline(limits: &Limits) -> Duration {
    match limits.movetime_ms {
        Some(ms) => Duration::from_millis(ms) + MOVETIME_GRACE,
        None => SEARCH_CEILING,
    }
}

/// A pooled, shareable handle to one engine configuration. Holds a bounded set
/// of idle [`Engine`] processes and hands them out one search at a time.
pub struct EngineService {
    config: EngineConfig,
    /// Idle, ready-to-reuse engines. Live-process count is bounded by `permits`.
    idle: Mutex<Vec<Engine>>,
    /// Caps how many engine processes run concurrently (the pool size).
    permits: Semaphore,
}

impl EngineService {
    /// Build a service for `config` allowing up to `pool_size` concurrent engine
    /// processes (at least one). Engines are spawned lazily on first use and
    /// then reused across calls.
    pub fn new(config: EngineConfig, pool_size: usize) -> Self {
        Self {
            config,
            idle: Mutex::new(Vec::new()),
            permits: Semaphore::new(pool_size.max(1)),
        }
    }

    /// The engine configuration backing this pool.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Run a bounded search on `fen` to completion and return the distilled
    /// [`Analysis`]. `options` are applied via `setoption` before the search
    /// (e.g. `MultiPV`, `Threads`). Unbounded `limits` fall back to a fixed
    /// depth so the call always returns.
    ///
    /// This is the **direct in-process API** the batch pipeline calls; the MCP
    /// `engine_analyse` tool routes through this exact method.
    pub async fn analyse(
        &self,
        fen: &str,
        limits: &Limits,
        options: &BTreeMap<String, String>,
    ) -> Result<Analysis> {
        // Hold a permit for the whole search: the pool never spawns more than
        // `pool_size` processes, and extra concurrent callers queue here.
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| anyhow!("engine pool is closed"))?;

        let mut engine = self.checkout().await?;
        match run_to_bestmove(&mut engine, fen, &bounded(limits), options).await {
            Ok(analysis) => {
                // Only a healthy, idle engine goes back into the pool.
                self.idle.lock().await.push(engine);
                Ok(analysis)
            }
            Err(e) => {
                // A failed search may leave the engine mid-state; discard it.
                let _ = engine.quit().await;
                Err(e)
            }
        }
    }

    /// Run a bounded search returning up to `multipv` principal variations, each
    /// the deepest `info` seen for that line. Line 0 is the engine's best move;
    /// later lines are the runners-up. Used by the full-game review pass (#119)
    /// to read a position's best move, its closest alternative, and the played
    /// move's rank among them.
    ///
    /// Like [`analyse`](Self::analyse) it holds one pool permit for the whole
    /// search and runs under the same overall deadline.
    pub async fn analyse_multi(
        &self,
        fen: &str,
        limits: &Limits,
        multipv: u16,
    ) -> Result<Vec<Analysis>> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| anyhow!("engine pool is closed"))?;

        let mut options = BTreeMap::new();
        if multipv > 1 {
            options.insert("MultiPV".to_string(), multipv.to_string());
        }

        let mut engine = self.checkout().await?;
        match run_to_lines(&mut engine, fen, &bounded(limits), &options, multipv).await {
            Ok(lines) => {
                self.idle.lock().await.push(engine);
                Ok(lines)
            }
            Err(e) => {
                let _ = engine.quit().await;
                Err(e)
            }
        }
    }

    /// Take an idle engine or spawn a fresh one. A permit is already held, so
    /// the live-process count stays within the pool size.
    async fn checkout(&self) -> Result<Engine> {
        if let Some(engine) = self.idle.lock().await.pop() {
            return Ok(engine);
        }
        Engine::spawn(self.config.clone()).await
    }

    /// Quit every pooled engine. Best-effort; `kill_on_drop` reaps any that
    /// ignore `quit`.
    pub async fn shutdown(&self) {
        let engines: Vec<Engine> = std::mem::take(&mut *self.idle.lock().await);
        for engine in engines {
            let _ = engine.quit().await;
        }
    }
}

/// Configure, search, and fold the event stream down to one [`Analysis`]. On
/// success the engine is left idle (post-`bestmove`) and safe to reuse.
///
/// The whole search runs under an overall [`search_deadline`]: a stuck or
/// pathologically slow engine can no longer hang forever and pin the pool. On
/// timeout the caller discards the (mid-search) engine rather than reusing it.
async fn run_to_bestmove(
    engine: &mut Engine,
    fen: &str,
    limits: &Limits,
    options: &BTreeMap<String, String>,
) -> Result<Analysis> {
    let deadline = search_deadline(limits);
    let search = async {
        for (name, value) in options {
            engine.set_option(name, value).await?;
        }
        if !options.is_empty() {
            engine.wait_ready().await?;
        }
        engine.set_position(fen).await?;
        engine.go(limits).await?;

        let mut primary: Option<AnalysisInfo> = None;
        loop {
            match engine.next_event().await? {
                Some(AnalysisEvent::Info(info)) => primary = fold_primary(primary, info),
                Some(AnalysisEvent::BestMove { best_move, ponder }) => {
                    return Ok(Analysis::from_search(primary, best_move, ponder));
                }
                None => bail!("engine exited before returning a best move"),
            }
        }
    };

    timeout(deadline, search).await.map_err(|_| {
        anyhow!(
            "engine search exceeded the {}s deadline",
            deadline.as_secs()
        )
    })?
}

/// Configure and search, folding the event stream into one [`Analysis`] per
/// MultiPV line (deepest `info` per line). Mirrors [`run_to_bestmove`] but keeps
/// every line rather than only the primary, for the review pass.
async fn run_to_lines(
    engine: &mut Engine,
    fen: &str,
    limits: &Limits,
    options: &BTreeMap<String, String>,
    multipv: u16,
) -> Result<Vec<Analysis>> {
    let deadline = search_deadline(limits);
    let slots = multipv.max(1) as usize;
    let search = async {
        for (name, value) in options {
            engine.set_option(name, value).await?;
        }
        if !options.is_empty() {
            engine.wait_ready().await?;
        }
        engine.set_position(fen).await?;
        engine.go(limits).await?;

        // Deepest meaningful `info` per line; index 0 is MultiPV 1 (the best).
        let mut lines: Vec<Option<AnalysisInfo>> = vec![None; slots];
        let (best_move, ponder) = loop {
            match engine.next_event().await? {
                Some(AnalysisEvent::Info(info)) => {
                    let idx = info.multipv.unwrap_or(1).saturating_sub(1) as usize;
                    if idx < lines.len() {
                        lines[idx] = Some(info);
                    }
                }
                Some(AnalysisEvent::BestMove { best_move, ponder }) => break (best_move, ponder),
                None => bail!("engine exited before returning a best move"),
            }
        };

        let mut out = Vec::with_capacity(slots);
        for (i, slot) in lines.into_iter().enumerate() {
            let Some(info) = slot else { continue };
            // Line 0 carries the terminal `bestmove`/`ponder`; a runner-up's move
            // is the head of its own PV.
            let (mv, ponder) = if i == 0 {
                (best_move.clone(), ponder.clone())
            } else {
                (info.pv.first().cloned().unwrap_or_default(), None)
            };
            out.push(Analysis::from_search(Some(info), mv, ponder));
        }
        // A search that ended without a usable info line still reports its move.
        if out.is_empty() {
            out.push(Analysis::from_search(None, best_move, ponder));
        }
        Ok(out)
    };

    timeout(deadline, search).await.map_err(|_| {
        anyhow!(
            "engine search exceeded the {}s deadline",
            deadline.as_secs()
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(depth: u8, multipv: Option<u16>, cp: i32, pv: &[&str]) -> AnalysisInfo {
        AnalysisInfo {
            depth: Some(depth),
            multipv,
            score: Some(Score::Cp { value: cp }),
            pv: pv.iter().map(|m| m.to_string()).collect(),
            ..AnalysisInfo::default()
        }
    }

    #[test]
    fn fold_keeps_only_the_primary_line() {
        let mut primary = None;
        primary = fold_primary(primary, info(10, Some(1), 30, &["e2e4"]));
        // A secondary PV must not displace the primary line.
        primary = fold_primary(primary, info(10, Some(2), -200, &["a2a3"]));
        // A deeper primary line refines it.
        primary = fold_primary(primary, info(12, Some(1), 35, &["e2e4", "e7e5"]));

        let got = primary.expect("a primary line");
        assert_eq!(got.depth, Some(12));
        assert_eq!(got.score, Some(Score::Cp { value: 35 }));
        assert_eq!(got.pv, vec!["e2e4", "e7e5"]);
    }

    #[test]
    fn fold_treats_missing_multipv_as_primary() {
        let primary = fold_primary(None, info(8, None, 12, &["d2d4"]));
        assert_eq!(primary.expect("primary").depth, Some(8));
    }

    #[test]
    fn analysis_carries_eval_pv_and_bestmove() {
        let primary = Some(info(14, Some(1), 42, &["e2e4", "e7e5"]));
        let a = Analysis::from_search(primary, "e2e4".to_string(), Some("e7e5".to_string()));
        assert_eq!(a.bestmove, "e2e4");
        assert_eq!(a.ponder.as_deref(), Some("e7e5"));
        assert_eq!(a.score, Some(Score::Cp { value: 42 }));
        assert_eq!(a.depth, Some(14));
        assert_eq!(a.pv, vec!["e2e4", "e7e5"]);
    }

    #[test]
    fn analysis_without_info_still_reports_bestmove() {
        let a = Analysis::from_search(None, "e2e4".to_string(), None);
        assert_eq!(a.bestmove, "e2e4");
        assert!(a.score.is_none());
        assert!(a.pv.is_empty());
    }

    #[test]
    fn unbounded_limits_get_a_default_depth() {
        assert_eq!(bounded(&Limits::default()).depth, Some(DEFAULT_DEPTH));
        // Any explicit bound is preserved untouched.
        let movetime = Limits {
            movetime_ms: Some(500),
            ..Limits::default()
        };
        assert_eq!(bounded(&movetime), movetime);
    }

    #[test]
    fn bounded_clamps_oversized_user_limits() {
        let huge = Limits {
            depth: Some(9_999),
            movetime_ms: Some(600_000),
            nodes: None,
        };
        let bounded = bounded(&huge);
        assert_eq!(bounded.depth, Some(super::super::command::MAX_DEPTH));
        assert_eq!(
            bounded.movetime_ms,
            Some(super::super::command::MAX_MOVETIME_MS)
        );
    }

    #[test]
    fn deadline_tracks_movetime_then_falls_back_to_the_ceiling() {
        let with_movetime = Limits {
            movetime_ms: Some(5_000),
            ..Limits::default()
        };
        assert_eq!(
            search_deadline(&with_movetime),
            Duration::from_millis(5_000) + MOVETIME_GRACE
        );
        // A depth-only search has no movetime budget ⇒ the fixed ceiling.
        assert_eq!(search_deadline(&Limits::depth(30)), SEARCH_CEILING);
    }

    #[test]
    fn analysis_serialises_to_a_flat_object() {
        let a = Analysis::from_search(
            Some(info(12, Some(1), 18, &["e2e4"])),
            "e2e4".to_string(),
            None,
        );
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["bestmove"], "e2e4");
        assert_eq!(json["score"]["type"], "cp");
        assert_eq!(json["score"]["value"], 18);
        assert_eq!(json["depth"], 12);
        assert_eq!(json["pv"][0], "e2e4");
        // No ponder ⇒ the field is omitted, not null.
        assert!(json.get("ponder").is_none());
    }
}
