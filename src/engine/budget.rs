//! Whole-job engine budget (ADR-0054). A long job (study/game analysis, danger
//! map, opening tree, shape regeneration) runs inside [`JobBudget::run`]; every
//! [`EngineService`](super::EngineService) call made from that task is bounded by
//! the job's remaining time. Once it is spent, engine calls return *empty*
//! results instead of searching, and the job is flagged truncated — so callers
//! return what they finished rather than an error or an unbounded run.
//!
//! Task-local rather than a parameter: the engine is reached through a dozen
//! adapters and traits (`Evaluator`, `MultiAnalyzer`, …), and threading a budget
//! through each would touch every one of them for a single cross-cutting rule.
//! Calls outside a job (one-shot MCP `engine_analyse`, tests) are unaffected.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Wall-clock budget for one long engine job.
pub const JOB_BUDGET: Duration = Duration::from_secs(5 * 60);

tokio::task_local! {
    static BUDGET: JobBudget;
}

#[derive(Clone)]
pub struct JobBudget {
    deadline: Instant,
    truncated: Arc<AtomicBool>,
}

impl JobBudget {
    pub fn new(limit: Duration) -> Self {
        Self {
            deadline: Instant::now() + limit,
            truncated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The standard [`JOB_BUDGET`].
    pub fn standard() -> Self {
        Self::new(JOB_BUDGET)
    }

    /// Run `job` under this budget. Returns its output and whether any engine
    /// work was skipped because the budget ran out.
    pub async fn run<F: Future>(self, job: F) -> (F::Output, bool) {
        let truncated = self.truncated.clone();
        let out = BUDGET.scope(self, job).await;
        (out, truncated.load(Ordering::Relaxed))
    }
}

/// Run `job` as a long engine job: under a fresh [`JOB_BUDGET`], or — when
/// already inside a job — as part of that one, so nesting never extends it.
pub async fn job<F: Future>(job: F) -> (F::Output, bool) {
    if remaining().is_some() {
        let out = job.await;
        (out, truncated())
    } else {
        JobBudget::standard().run(job).await
    }
}

/// Time left in the enclosing job, or `None` outside any job.
pub(crate) fn remaining() -> Option<Duration> {
    BUDGET
        .try_with(|b| b.deadline.saturating_duration_since(Instant::now()))
        .ok()
}

pub(crate) fn mark_truncated() {
    let _ = BUDGET.try_with(|b| b.truncated.store(true, Ordering::Relaxed));
}

/// Whether the enclosing job has already skipped engine work. Loops use it to
/// stop instead of writing the empty fallback results into their output.
pub fn truncated() -> bool {
    BUDGET
        .try_with(|b| b.truncated.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// Run one engine call under the enclosing job's remaining time. Outside a job
/// it just runs `work`. Inside, an exhausted budget — or one that expires while
/// `work` is still queued or searching (the future is dropped, so its engine
/// process is killed and its pool permit released) — yields `fallback` and marks
/// the job truncated.
pub(crate) async fn bounded<T>(
    fallback: T,
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match remaining() {
        None => work.await,
        Some(left) if left.is_zero() => {
            mark_truncated();
            Ok(fallback)
        }
        Some(left) => match tokio::time::timeout(left, work).await {
            Ok(result) => result,
            Err(_) => {
                mark_truncated();
                Ok(fallback)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn outside_a_job_work_just_runs() {
        let out = bounded(0, async { Ok(7) }).await.unwrap();
        assert_eq!(out, 7);
        assert!(!truncated());
        assert!(remaining().is_none());
    }

    #[tokio::test]
    async fn nested_job_shares_the_outer_budget() {
        let ((_, inner_truncated), outer_truncated) = JobBudget::new(Duration::ZERO)
            .run(job(async { bounded(0, async { Ok(1) }).await }))
            .await;
        assert!(inner_truncated && outer_truncated);
    }

    #[tokio::test]
    async fn work_within_budget_is_not_truncated() {
        let (out, truncated) = JobBudget::new(Duration::from_secs(5))
            .run(async { bounded(0, async { Ok(7) }).await.unwrap() })
            .await;
        assert_eq!((out, truncated), (7, false));
    }

    #[tokio::test]
    async fn exhausted_budget_skips_work_and_flags_truncation() {
        let (out, flagged) = JobBudget::new(Duration::ZERO)
            .run(async {
                let v = bounded(0, async { panic!("must not run") }).await.unwrap();
                (v, truncated())
            })
            .await;
        assert_eq!(out, (0, true));
        assert!(flagged);
    }

    #[tokio::test]
    async fn budget_expiring_mid_call_returns_the_fallback() {
        let (out, flagged) = JobBudget::new(Duration::from_millis(50))
            .run(bounded(0, async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(7)
            }))
            .await;
        assert_eq!(out.unwrap(), 0);
        assert!(flagged);
    }
}
