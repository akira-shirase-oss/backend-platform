//! The worker: a fixed number of reconcile tasks, each claiming one operation at a time.
//! Every task is tracked; cancellation stops claiming, and an operation in progress runs to
//! the end of its round (bounded by attempts × deadlines) or is abandoned at the caller's
//! shutdown deadline — safe, because its lease expires and another worker resumes it.

use std::sync::Arc;
use std::time::Duration;

use platform_adapters::Provider;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::log::{LogError, OperationLog};
use crate::reconcile::{Hooks, ReconcileConfig, RunError, next_entropy, run_claimed};

/// How the worker runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerConfig {
    /// This worker's id (the lease owner).
    pub id: Uuid,
    /// Concurrent reconcile tasks.
    pub concurrency: usize,
    /// Longest idle poll interval (the interval doubles from 5 ms while idle).
    pub poll_idle_max: Duration,
    /// Deadlines and budgets.
    pub reconcile: ReconcileConfig,
}

/// Runs the worker until `cancel` fires. Returns once every task has stopped.
pub async fn run<L, P, H>(
    log: Arc<L>,
    provider: Arc<P>,
    hooks: Arc<H>,
    config: WorkerConfig,
    cancel: CancellationToken,
) where
    L: OperationLog + 'static,
    P: Provider + 'static,
    H: Hooks + 'static,
{
    let tracker = TaskTracker::new();
    for task in 0..config.concurrency {
        let (log, provider, hooks, cancel) =
            (log.clone(), provider.clone(), hooks.clone(), cancel.clone());
        tracker.spawn(async move {
            task_loop(&*log, &*provider, &*hooks, &config, &cancel, task).await;
        });
    }
    tracker.close();
    tracker.wait().await;
}

async fn task_loop<L: OperationLog, P: Provider, H: Hooks>(
    log: &L,
    provider: &P,
    hooks: &H,
    config: &WorkerConfig,
    cancel: &CancellationToken,
    task: usize,
) {
    const IDLE_MIN: Duration = Duration::from_millis(5);
    let mut entropy = config.id.as_u64_pair().0 ^ (task as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut idle = IDLE_MIN;
    while !cancel.is_cancelled() {
        match log.claim(config.id, config.reconcile.lease).await {
            Ok(Some(claimed)) => {
                idle = IDLE_MIN;
                let operation = claimed.lease.operation;
                match run_claimed(
                    log,
                    provider,
                    hooks,
                    &config.reconcile,
                    claimed,
                    &mut entropy,
                )
                .await
                {
                    Ok(outcome) => tracing::debug!(%operation, ?outcome, "round finished"),
                    Err(RunError::Halted(_)) => {
                        tracing::warn!(%operation, "halted at a fault point");
                        return;
                    }
                    Err(RunError::Log(LogError::Fenced)) => {
                        tracing::warn!(%operation, "lease lost; another worker owns the operation");
                    }
                    Err(error) => {
                        // The lease expires and the operation resumes from its recorded steps.
                        tracing::error!(%operation, error = platform_db::chain(&error), "reconcile stopped");
                        pause(cancel, Duration::from_millis(200)).await;
                    }
                }
            }
            Ok(None) => {
                let jitter = Duration::from_micros(next_entropy(&mut entropy) % 2_000);
                pause(cancel, idle + jitter).await;
                idle = (idle * 2).min(config.poll_idle_max);
            }
            Err(error) => {
                tracing::warn!(
                    error = platform_db::chain(&error),
                    "claim failed; backing off"
                );
                pause(cancel, Duration::from_millis(250)).await;
            }
        }
    }
}

async fn pause(cancel: &CancellationToken, duration: Duration) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = tokio::time::sleep(duration) => {}
    }
}
