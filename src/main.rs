#![deny(unsafe_code)]

mod dispatcher;
mod handlers;
mod miniapp;
mod response;
mod state;

use std::collections::HashSet;
use std::env;
use std::num::NonZeroI64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use grammers_client::sender::{ConnectionParams, UpdatesConfiguration, UpdatesOverflowPolicy};
use grammers_client::session::storages::SqliteSession;
use grammers_client::{Client, SenderPool};

use handlers::{BotIdentity, Ctx, RuntimeConfig};
use state::{ImageFilterWrite, Settings};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct UpdateTelemetry {
    received: AtomicU64,
    completed: AtomicU64,
    active: AtomicU64,
    queued: AtomicU64,
    failed: AtomicU64,
    latency: [AtomicU64; 32],
    queue_wait: [AtomicU64; 32],
}

impl UpdateTelemetry {
    fn complete(&self, completion: dispatcher::Completion) -> bool {
        match completion.result {
            Ok(()) => {
                self.completed.fetch_add(1, Ordering::Relaxed);
                for (histogram, value) in [
                    (&self.latency, completion.latency),
                    (&self.queue_wait, completion.queue_wait),
                ] {
                    let micros = value.as_micros().min(u32::MAX as u128) as u32;
                    let bucket = (u32::BITS - micros.max(1).leading_zeros()).min(31) as usize;
                    histogram[bucket].fetch_add(1, Ordering::Relaxed);
                }
                true
            }
            Err(error) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                log::error!("update handler failed: {error}");
                false
            }
        }
    }

    fn percentiles(histogram: &[AtomicU64; 32]) -> [u64; 3] {
        let buckets: [u64; 32] =
            std::array::from_fn(|index| histogram[index].swap(0, Ordering::Relaxed));
        let count: u64 = buckets.iter().sum();
        [50, 95, 99].map(|percent| {
            if count == 0 {
                return 0;
            }
            let target = (count * percent).div_ceil(100);
            let mut cumulative = 0;
            for (index, bucket) in buckets.iter().enumerate() {
                cumulative += bucket;
                if cumulative >= target {
                    return if index == 31 { u64::MAX } else { 1u64 << index };
                }
            }
            u32::MAX as u64
        })
    }
}

#[derive(Debug)]
enum UpdateWorkerError {
    Initialize(Box<dyn std::error::Error + Send + Sync>),
    Receive(grammers_client::InvocationError),
    Synchronize(Box<dyn std::error::Error + Send + Sync>),
    ReceiveAndSynchronize {
        receive: grammers_client::InvocationError,
        synchronize: Box<dyn std::error::Error + Send + Sync>,
    },
    HandlerPanicked {
        count: usize,
    },
    DrainAborted {
        unfinished: usize,
    },
}

struct UpdateWorkerOutcome {
    updates: grammers_client::client::UpdateStream,
    receive_error: Option<grammers_client::InvocationError>,
}

impl UpdateWorkerOutcome {
    async fn synchronize(mut self) -> std::result::Result<(), UpdateWorkerError> {
        let synchronize_error = self
            .updates
            .sync_update_state()
            .await
            .err()
            .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
        match (self.receive_error, synchronize_error) {
            (None, None) => Ok(()),
            (Some(receive), None) => Err(UpdateWorkerError::Receive(receive)),
            (None, Some(synchronize)) => Err(UpdateWorkerError::Synchronize(synchronize)),
            (Some(receive), Some(synchronize)) => Err(UpdateWorkerError::ReceiveAndSynchronize {
                receive,
                synchronize,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateShutdown {
    Running,
    Drain,
    Abort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CheckpointDecision {
    Commit,
    ResumeWithoutCommit,
}

struct UpdateCheckpointCommand {
    ready: tokio::sync::oneshot::Sender<()>,
    decision: tokio::sync::oneshot::Receiver<CheckpointDecision>,
    completed: tokio::sync::oneshot::Sender<Result>,
}

struct PendingUpdateCheckpoint {
    label: &'static str,
    ready: tokio::sync::oneshot::Receiver<()>,
    decision: tokio::sync::oneshot::Sender<CheckpointDecision>,
    completed: tokio::sync::oneshot::Receiver<Result>,
}

#[derive(Debug)]
enum PeriodicCheckpointError {
    WorkerCommand {
        label: &'static str,
    },
    WorkerStopped {
        label: &'static str,
        phase: &'static str,
    },
    OwnedTaskFailed {
        count: usize,
    },
    OwnedTaskTimeout,
    CursorCommitTimeout,
    CursorCommitFailed(String),
    PersistenceBarrierPoisoned,
    BackgroundTaskFailed(String),
    DatabaseOwnershipLost,
}

impl std::fmt::Display for PeriodicCheckpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerCommand { label } => {
                write!(
                    formatter,
                    "{label} update worker rejected its bounded checkpoint command"
                )
            }
            Self::WorkerStopped { label, phase } => {
                write!(
                    formatter,
                    "{label} update worker stopped during checkpoint {phase}"
                )
            }
            Self::OwnedTaskFailed { count } => write!(
                formatter,
                "{count} owned continuation(s) failed during the checkpoint barrier"
            ),
            Self::OwnedTaskTimeout => formatter.write_str(
                "owned continuations exceeded the periodic checkpoint budget and were cancelled",
            ),
            Self::CursorCommitTimeout => formatter.write_str(
                "cursor storage exceeded the periodic checkpoint budget after commit authorization",
            ),
            Self::CursorCommitFailed(error) => write!(
                formatter,
                "cursor storage failed after commit authorization; workers were stopped: {error}"
            ),
            Self::PersistenceBarrierPoisoned => formatter.write_str(
                "correctness-critical persistence failed; update intake and cursor persistence are fail-closed",
            ),
            Self::BackgroundTaskFailed(error) => {
                write!(
                    formatter,
                    "background task failed before cursor commit: {error}"
                )
            }
            Self::DatabaseOwnershipLost => formatter.write_str(
                "database ownership was lost before cursor commit; checkpoint is fail-closed",
            ),
        }
    }
}

impl std::error::Error for PeriodicCheckpointError {}

enum PeriodicCheckpointOutcome {
    Committed,
    Deferred { reason: String },
}

fn finish_periodic_cursor_commit(
    bot_result: Result,
    cleaner_result: Result,
) -> std::result::Result<PeriodicCheckpointOutcome, PeriodicCheckpointError> {
    let mut failures = Vec::new();
    if let Err(error) = bot_result {
        failures.push(format!("bot cursor: {error}"));
    }
    if let Err(error) = cleaner_result {
        failures.push(format!("cleaner cursor: {error}"));
    }
    if failures.is_empty() {
        Ok(PeriodicCheckpointOutcome::Committed)
    } else {
        Err(PeriodicCheckpointError::CursorCommitFailed(
            failures.join("; "),
        ))
    }
}

impl std::fmt::Display for UpdateWorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialize(error) => {
                write!(formatter, "update stream initialization failed: {error}")
            }
            Self::Receive(error) => write!(formatter, "update stream failed: {error}"),
            Self::Synchronize(error) => {
                write!(formatter, "update-state synchronization failed: {error}")
            }
            Self::ReceiveAndSynchronize {
                receive,
                synchronize,
            } => write!(
                formatter,
                "update stream failed ({receive}) and its state could not be synchronized ({synchronize})"
            ),
            Self::DrainAborted { unfinished } => write!(
                formatter,
                "shutdown aborted and joined {unfinished} unfinished admitted update(s); update state was not advanced"
            ),
            Self::HandlerPanicked { count } => write!(
                formatter,
                "{count} admitted update handler(s) panicked; update state was not advanced (subsequent protocol replay remains subject to application freshness/idempotency policy)"
            ),
        }
    }
}

impl std::error::Error for UpdateWorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Initialize(error) | Self::Synchronize(error) => Some(error.as_ref()),
            Self::Receive(error) | Self::ReceiveAndSynchronize { receive: error, .. } => {
                Some(error)
            }
            Self::DrainAborted { .. } => None,
            Self::HandlerPanicked { .. } => None,
        }
    }
}

type UpdateWorkerJoinResult = std::result::Result<
    std::result::Result<UpdateWorkerOutcome, UpdateWorkerError>,
    tokio::task::JoinError,
>;

fn retain_update_worker_outcome(
    label: &str,
    result: Option<UpdateWorkerJoinResult>,
    update_error: &mut Option<Box<dyn std::error::Error + Send + Sync>>,
) -> Option<UpdateWorkerOutcome> {
    match result {
        Some(Ok(Ok(outcome))) => Some(outcome),
        Some(Ok(Err(error))) => {
            log::error!("{label} update worker failed: {error}");
            if update_error.is_none() {
                *update_error = Some(Box::new(error));
            }
            None
        }
        Some(Err(error)) if error.is_cancelled() => {
            log::error!("shutdown: {label} update worker was aborted without a cursor commit");
            if update_error.is_none() {
                *update_error = Some(Box::new(error));
            }
            None
        }
        Some(Err(error)) => {
            log::error!("{label} update worker failed: {error}");
            if update_error.is_none() {
                *update_error = Some(Box::new(error));
            }
            None
        }
        None => {
            if update_error.is_none() {
                *update_error = Some(
                    format!("shutdown: {label} update worker outcome was not observed").into(),
                );
            }
            None
        }
    }
}

enum CheckpointPause {
    Resume,
    Drain,
}

async fn pause_at_checkpoint(
    updates: &mut grammers_client::client::UpdateStream,
    mut command: UpdateCheckpointCommand,
    stopping: &mut tokio::sync::watch::Receiver<UpdateShutdown>,
) -> std::result::Result<CheckpointPause, UpdateWorkerError> {
    if command.ready.send(()).is_err() {
        return Ok(CheckpointPause::Resume);
    }

    let decision = loop {
        match *stopping.borrow() {
            UpdateShutdown::Running => {}
            UpdateShutdown::Drain => return Ok(CheckpointPause::Drain),
            UpdateShutdown::Abort => {
                return Err(UpdateWorkerError::DrainAborted { unfinished: 0 });
            }
        }
        tokio::select! {
            biased;
            changed = stopping.changed() => {
                if changed.is_err() {
                    return Err(UpdateWorkerError::DrainAborted { unfinished: 0 });
                }
            }
            decision = &mut command.decision => {
                break decision.unwrap_or(CheckpointDecision::ResumeWithoutCommit);
            }
        }
    };

    if decision == CheckpointDecision::Commit {
        let result = updates
            .sync_update_state()
            .await
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
        if command.completed.send(result).is_err() {
            log::warn!("update checkpoint completed after its coordinator was cancelled");
        }
    }

    match *stopping.borrow() {
        UpdateShutdown::Running => Ok(CheckpointPause::Resume),
        UpdateShutdown::Drain => Ok(CheckpointPause::Drain),
        UpdateShutdown::Abort => Err(UpdateWorkerError::DrainAborted { unfinished: 0 }),
    }
}

fn request_update_checkpoint(
    label: &'static str,
    commands: &tokio::sync::mpsc::Sender<UpdateCheckpointCommand>,
) -> std::result::Result<PendingUpdateCheckpoint, PeriodicCheckpointError> {
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    let (decision, decision_rx) = tokio::sync::oneshot::channel();
    let (completed_tx, completed) = tokio::sync::oneshot::channel();
    commands
        .try_send(UpdateCheckpointCommand {
            ready: ready_tx,
            decision: decision_rx,
            completed: completed_tx,
        })
        .map_err(|_| PeriodicCheckpointError::WorkerCommand { label })?;
    Ok(PendingUpdateCheckpoint {
        label,
        ready,
        decision,
        completed,
    })
}

fn reap_completed_background_tasks(
    background_tasks: &mut tokio::task::JoinSet<()>,
) -> std::result::Result<(), PeriodicCheckpointError> {
    while let Some(completed) = background_tasks.try_join_next() {
        if let Err(error) = completed {
            return Err(PeriodicCheckpointError::BackgroundTaskFailed(
                error.to_string(),
            ));
        }
    }
    Ok(())
}

fn close_cursor_gate_for_ownership_loss(shutdown_failure: &mut Option<String>) {
    shutdown_failure.get_or_insert_with(|| {
        "database ownership was lost; volatile state and Telegram cursors were not persisted"
            .to_owned()
    });
}

fn cursor_commit_allowed(shutdown_failure: &Option<String>, owned_graceful: bool) -> bool {
    shutdown_failure.is_none() && owned_graceful
}

async fn periodic_update_checkpoint(
    ctx: &Arc<Ctx>,
    bot_commands: &tokio::sync::mpsc::Sender<UpdateCheckpointCommand>,
    cleaner_commands: &tokio::sync::mpsc::Sender<UpdateCheckpointCommand>,
    background_tasks: &mut tokio::task::JoinSet<()>,
    deadline: tokio::time::Instant,
) -> std::result::Result<PeriodicCheckpointOutcome, PeriodicCheckpointError> {
    let bot = request_update_checkpoint("bot", bot_commands)?;
    let cleaner = match request_update_checkpoint("cleaner", cleaner_commands) {
        Ok(checkpoint) => checkpoint,
        Err(error) => {
            drop(bot.decision);
            return Err(error);
        }
    };
    let PendingUpdateCheckpoint {
        label: bot_label,
        ready: bot_ready,
        decision: bot_decision,
        completed: bot_completed,
    } = bot;
    let PendingUpdateCheckpoint {
        label: cleaner_label,
        ready: cleaner_ready,
        decision: cleaner_decision,
        completed: cleaner_completed,
    } = cleaner;

    let (bot_ready, cleaner_ready) =
        tokio::time::timeout_at(deadline, async { tokio::join!(bot_ready, cleaner_ready) })
            .await
            .map_err(|_| PeriodicCheckpointError::WorkerStopped {
                label: "bot or cleaner",
                phase: "barrier deadline",
            })?;
    if bot_ready.is_err() {
        return Err(PeriodicCheckpointError::WorkerStopped {
            label: bot_label,
            phase: "barrier",
        });
    }
    if cleaner_ready.is_err() {
        return Err(PeriodicCheckpointError::WorkerStopped {
            label: cleaner_label,
            phase: "barrier",
        });
    }

    let _background_epoch = match tokio::time::timeout_at(deadline, ctx.checkpoint_epoch()).await {
        Ok(epoch) => epoch,
        Err(_) => {
            bot_decision
                .send(CheckpointDecision::ResumeWithoutCommit)
                .map_err(|_| PeriodicCheckpointError::WorkerStopped {
                    label: bot_label,
                    phase: "background-barrier resume",
                })?;
            cleaner_decision
                .send(CheckpointDecision::ResumeWithoutCommit)
                .map_err(|_| PeriodicCheckpointError::WorkerStopped {
                    label: cleaner_label,
                    phase: "background-barrier resume",
                })?;
            return Ok(PeriodicCheckpointOutcome::Deferred {
                reason:
                    "a background iteration exceeded the checkpoint budget; no cursor was advanced"
                        .to_owned(),
            });
        }
    };

    if ctx.background_task_panicked() {
        return Err(PeriodicCheckpointError::BackgroundTaskFailed(
            "a background iteration panicked before its JoinError became observable".to_owned(),
        ));
    }
    reap_completed_background_tasks(background_tasks)?;

    let mut owned = ctx.begin_owned_checkpoint();
    let owned_failures = match tokio::time::timeout_at(deadline, owned.join(ctx)).await {
        Ok(failures) => failures,
        Err(_) => {
            owned.retain_for_shutdown(ctx);
            return Err(PeriodicCheckpointError::OwnedTaskTimeout);
        }
    };
    if owned_failures != 0 {
        return Err(PeriodicCheckpointError::OwnedTaskFailed {
            count: owned_failures,
        });
    }

    let flushed = tokio::time::timeout_at(deadline, async {
        let media_flushed = handlers::tempmedia::flush_pending(ctx).await;
        let statistics_flushed = handlers::stats::flush(ctx).await;
        let logs_flushed = handlers::log::flush_all(ctx).await;
        let samples_flushed = ctx.flush_samples().await;
        media_flushed && statistics_flushed && logs_flushed && samples_flushed
    })
    .await;

    if ctx.cursor_barrier_poisoned() {
        return Err(PeriodicCheckpointError::PersistenceBarrierPoisoned);
    }
    let persistence_failure = match flushed {
        Ok(true) => None,
        Ok(false) => Some(
            "volatile persistence did not complete; the previous cursor remains active"
                .to_owned(),
        ),
        Err(_) => Some(
            "volatile persistence exceeded the checkpoint budget; cancellation-safe batches were retained and the previous cursor remains active"
                .to_owned(),
        ),
    };
    if let Some(reason) = persistence_failure {
        bot_decision
            .send(CheckpointDecision::ResumeWithoutCommit)
            .map_err(|_| PeriodicCheckpointError::WorkerStopped {
                label: bot_label,
                phase: "resume",
            })?;
        cleaner_decision
            .send(CheckpointDecision::ResumeWithoutCommit)
            .map_err(|_| PeriodicCheckpointError::WorkerStopped {
                label: cleaner_label,
                phase: "resume",
            })?;
        return Ok(PeriodicCheckpointOutcome::Deferred { reason });
    }

    if !ctx.settings.liveness().load(Ordering::Acquire) {
        return Err(PeriodicCheckpointError::DatabaseOwnershipLost);
    }

    bot_decision.send(CheckpointDecision::Commit).map_err(|_| {
        PeriodicCheckpointError::WorkerStopped {
            label: bot_label,
            phase: "commit authorization",
        }
    })?;
    cleaner_decision
        .send(CheckpointDecision::Commit)
        .map_err(|_| PeriodicCheckpointError::WorkerStopped {
            label: cleaner_label,
            phase: "commit authorization",
        })?;

    let (bot_result, cleaner_result) = tokio::time::timeout_at(deadline, async {
        tokio::join!(bot_completed, cleaner_completed)
    })
    .await
    .map_err(|_| PeriodicCheckpointError::CursorCommitTimeout)?;
    let bot_result = bot_result.map_err(|_| PeriodicCheckpointError::WorkerStopped {
        label: bot_label,
        phase: "cursor commit",
    })?;
    let cleaner_result = cleaner_result.map_err(|_| PeriodicCheckpointError::WorkerStopped {
        label: cleaner_label,
        phase: "cursor commit",
    })?;
    finish_periodic_cursor_commit(bot_result, cleaner_result)
}

async fn run_bot_updates(
    ctx: Arc<Ctx>,
    client: Client,
    updates: tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>,
    concurrency: usize,
    telemetry: Arc<UpdateTelemetry>,
    mut stopping: tokio::sync::watch::Receiver<UpdateShutdown>,
    mut checkpoint_commands: tokio::sync::mpsc::Receiver<UpdateCheckpointCommand>,
) -> std::result::Result<UpdateWorkerOutcome, UpdateWorkerError> {
    let mut updates = client
        .stream_updates(
            updates,
            UpdatesConfiguration {
                catch_up: true,
                drop_idle_channels: true,
            },
        )
        .await
        .map_err(UpdateWorkerError::Initialize)?;
    let mut tasks = dispatcher::Dispatcher::new(
        concurrency,
        handlers::PER_CHAT_UPDATES,
        UPDATES_CHANNEL_CAPACITY.get(),
    );
    'updates: loop {
        let mut handler_panics = 0usize;
        let mut checkpoint = None;
        let mut receive_error = loop {
            while let Some(finished) = tasks.try_join_next() {
                handler_panics += usize::from(!telemetry.complete(finished));
            }
            tasks.start(|update| {
                let ctx = Arc::clone(&ctx);
                async move {
                    handlers::dispatch(&ctx, update).await;
                }
            });
            telemetry
                .active
                .store(tasks.active() as u64, Ordering::Relaxed);
            telemetry
                .queued
                .store(tasks.pending() as u64, Ordering::Relaxed);
            let update = tokio::select! {
                _ = stopping.changed() => break None,
                command = checkpoint_commands.recv() => {
                    checkpoint = command;
                    break None;
                }
                finished = tasks.join_next(), if tasks.active() > 0 => {
                    if let Some(finished) = finished {
                        handler_panics += usize::from(!telemetry.complete(finished));
                    }
                    continue;
                }
                update = updates.next(), if tasks.has_room() => update,
            };
            match update {
                Ok(update) => {
                    telemetry.received.fetch_add(1, Ordering::Relaxed);
                    tasks.push(handlers::dispatch_key(&update), update);
                }
                Err(error) => break Some(error),
            }
        };

        let mut cursor_error = if matches!(
            receive_error.as_ref(),
            Some(grammers_client::InvocationError::Session(_))
        ) {
            receive_error.take()
        } else {
            None
        };

        while cursor_error.is_none() {
            while let Some(finished) = tasks.try_join_next() {
                handler_panics += usize::from(!telemetry.complete(finished));
            }
            tasks.start(|update| {
                let ctx = Arc::clone(&ctx);
                async move {
                    handlers::dispatch(&ctx, update).await;
                }
            });
            if *stopping.borrow() == UpdateShutdown::Abort {
                let unfinished = tasks.shutdown().await;
                return Err(UpdateWorkerError::DrainAborted { unfinished });
            }
            if !tasks.has_room() {
                tokio::select! {
                    changed = stopping.changed() => {
                        if changed.is_err() || *stopping.borrow() == UpdateShutdown::Abort {
                            let unfinished = tasks.shutdown().await;
                            return Err(UpdateWorkerError::DrainAborted { unfinished });
                        }
                    }
                    finished = tasks.join_next() => {
                        if let Some(finished) = finished {
                            handler_panics += usize::from(!telemetry.complete(finished));
                        }
                    }
                }
                continue;
            }
            let buffered = tokio::select! {
                changed = stopping.changed() => {
                    if changed.is_err() || *stopping.borrow() == UpdateShutdown::Abort {
                        let unfinished = tasks.shutdown().await;
                        return Err(UpdateWorkerError::DrainAborted { unfinished });
                    }
                    continue;
                }
                update = updates.next_buffered() => update,
            };
            let update = match buffered {
                Ok(Some(update)) => update,
                Ok(None) => break,
                Err(error) => {
                    cursor_error = Some(error);
                    break;
                }
            };
            telemetry.received.fetch_add(1, Ordering::Relaxed);
            tasks.push(handlers::dispatch_key(&update), update);
        }

        while !tasks.is_empty() {
            if *stopping.borrow() == UpdateShutdown::Abort {
                let unfinished = tasks.shutdown().await;
                return Err(UpdateWorkerError::DrainAborted { unfinished });
            }
            tasks.start(|update| {
                let ctx = Arc::clone(&ctx);
                async move {
                    handlers::dispatch(&ctx, update).await;
                }
            });
            tokio::select! {
                changed = stopping.changed() => {
                    if changed.is_err() || *stopping.borrow() == UpdateShutdown::Abort {
                        let unfinished = tasks.shutdown().await;
                        return Err(UpdateWorkerError::DrainAborted { unfinished });
                    }
                }
                finished = tasks.join_next() => {
                    if let Some(finished) = finished {
                        handler_panics += usize::from(!telemetry.complete(finished));
                    }
                }
            }
        }
        if handler_panics != 0 {
            return Err(UpdateWorkerError::HandlerPanicked {
                count: handler_panics,
            });
        }
        if let Some(error) = cursor_error {
            return Err(UpdateWorkerError::Receive(error));
        }
        if let Some(command) = checkpoint {
            match pause_at_checkpoint(&mut updates, command, &mut stopping).await? {
                CheckpointPause::Resume => continue 'updates,
                CheckpointPause::Drain => {}
            }
        }
        return Ok(UpdateWorkerOutcome {
            updates,
            receive_error,
        });
    }
}

async fn wait_optional_task<T>(
    task: &mut Option<tokio::task::JoinHandle<T>>,
) -> Option<std::result::Result<T, tokio::task::JoinError>> {
    match task {
        Some(task) => Some(task.await),
        None => std::future::pending().await,
    }
}

const SESSION_FILE: &str = "groupbot.session";

const USER_SESSION_FILE: &str = "cleaner.session";
const SETTINGS_FILE: &str = "groupbot.data";

const DEFAULT_UPDATE_CONCURRENCY: usize = 512;

const DEFAULT_MAX_SHARD_CHATS: usize = 50_000;

const DEFAULT_CLEANER_CONCURRENCY: usize = 128;

const NIGHT_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

const LOG_FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

const TEMP_MEDIA_SWEEP: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEFERRED_SWEEP_SECS: u32 = 5;
const DEFERRED_SWEEP: std::time::Duration =
    std::time::Duration::from_secs(DEFERRED_SWEEP_SECS as u64);

const STATS_FLUSH: std::time::Duration = std::time::Duration::from_secs(60);
const SAMPLE_FLUSH: std::time::Duration = std::time::Duration::from_secs(60);
const UPDATE_CHECKPOINT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const UPDATE_CHECKPOINT_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

const SHUTDOWN_DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(75);
const SHUTDOWN_TOTAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(105);
const SHUTDOWN_ABORT_BUDGET: std::time::Duration = std::time::Duration::from_secs(115);

const CHAT_SWEEP: std::time::Duration = std::time::Duration::from_secs(600);

const CHAT_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);

const UPDATES_CHANNEL_CAPACITY: std::num::NonZeroUsize = std::num::NonZeroUsize::new(4096).unwrap();

fn configured_cap(name: &str, default: usize, minimum: usize, maximum: usize) -> Result<usize> {
    let value = match env::var(name) {
        Ok(value) => value.parse::<usize>().map_err(|_| {
            format!("{name} must be an integer in {minimum}..={maximum}, got {value:?}")
        })?,
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} is not valid Unicode").into());
        }
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{name} must be in {minimum}..={maximum}, got {value}").into());
    }
    Ok(value)
}

fn configured_log_level(value: Option<&str>) -> Result<log::LevelFilter> {
    match value {
        None | Some("warn") => Ok(log::LevelFilter::Warn),
        Some("debug") => Ok(log::LevelFilter::Debug),
        Some("trace") => Ok(log::LevelFilter::Trace),
        Some("info") => Ok(log::LevelFilter::Info),
        Some("error") => Ok(log::LevelFilter::Error),
        Some(value) => Err(format!(
            "LOG must be one of error, warn, info, debug, or trace; got {value:?}"
        )
        .into()),
    }
}

fn optional_nonempty_env(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            validate_nonempty_env_value(name, &value)?;
            Ok(Some(value))
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid Unicode").into()),
    }
}

fn validate_nonempty_env_value(name: &str, value: &str) -> Result {
    if value.trim().is_empty() {
        return Err(format!("{name} is present but empty").into());
    }
    if value != value.trim() {
        return Err(
            format!("{name} has leading or trailing whitespace; remove it explicitly").into(),
        );
    }
    Ok(())
}

fn required_nonempty_env(name: &str) -> Result<String> {
    optional_nonempty_env(name)?.ok_or_else(|| format!("{name} is required").into())
}

fn parse_api_id(value: &str) -> Result<i32> {
    let id = value
        .parse::<i32>()
        .map_err(|_| format!("TG_ID must be a positive 32-bit Telegram API id, got {value:?}"))?;
    if id <= 0 {
        return Err(format!("TG_ID must be positive, got {id}").into());
    }
    Ok(id)
}

fn optional_user_id(name: &str) -> Result<Option<i64>> {
    let Some(value) = optional_nonempty_env(name)? else {
        return Ok(None);
    };
    let id = value
        .parse::<i64>()
        .map_err(|_| format!("{name} must be a Telegram user id, got {value:?}"))?;
    if grammers_session::types::PeerId::user(id).is_none() {
        return Err(format!("{name} is outside Telegram's user-id range: {id}").into());
    }
    Ok(Some(id))
}

fn load_owned_chats(path: &str, max_chats: usize) -> Result<Arc<HashSet<i64>>> {
    let contents = std::fs::read_to_string(path)?;
    let mut chats = HashSet::new();
    for (line_number, line) in contents.lines().enumerate() {
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        if value.split_whitespace().count() != 1 {
            return Err(format!(
                "{path}:{}: expected one chat id per line; filter shard_route TSV output first",
                line_number + 1
            )
            .into());
        }
        let chat: i64 = value
            .parse()
            .map_err(|_| format!("{path}:{}: invalid BIGINT chat id", line_number + 1))?;
        if chat == 0 {
            return Err(format!("{path}:{}: chat id 0 is global state", line_number + 1).into());
        }
        if !chats.insert(chat) {
            return Err(format!("{path}:{}: duplicate chat id {chat}", line_number + 1).into());
        }
        if chats.len() > max_chats {
            return Err(format!("{path}: more than MAX_SHARD_CHATS={max_chats} chat ids").into());
        }
    }
    if chats.is_empty() {
        return Err(format!("{path}: route file is empty").into());
    }
    Ok(Arc::new(chats))
}

async fn shutdown_signal() -> std::io::Result<tokio::time::Instant> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
    }
    Ok(tokio::time::Instant::now())
}

async fn stop_shutdown_watcher(
    watcher: &mut tokio::task::JoinHandle<std::io::Result<tokio::time::Instant>>,
) -> Result<Option<tokio::time::Instant>> {
    if !watcher.is_finished() {
        watcher.abort();
    }
    match watcher.await {
        Ok(Ok(requested_at)) => Ok(Some(requested_at)),
        Ok(Err(error)) => Err(Box::new(error)),
        Err(error) if error.is_cancelled() => Ok(None),
        Err(error) => Err(Box::new(error)),
    }
}

async fn capacity_probe() -> Result {
    let database_url = required_nonempty_env("DATABASE_URL")?;
    let started = std::time::Instant::now();
    let settings = Settings::connect(&database_url).await?;
    let load_ms = started.elapsed().as_millis();
    let chats = settings.chats();
    let passes = configured_cap("CAPACITY_PROBE_PASSES", 3, 1, 100)?;
    let scan_started = std::time::Instant::now();
    let mut checked = 0usize;
    let mut present = 0usize;
    for _ in 0..passes {
        for chat in chats.iter().copied() {
            checked += 1;
            present += settings.with_chat(chat, |values| {
                ["owner", "flood", "night", "report_at", "auto_purge_at"]
                    .into_iter()
                    .filter(|key| values.value(key).is_some() || values.is_locked(key))
                    .count()
            });
        }
    }
    println!(
        "capacity-probe: chats={} settings_rows={} settings_bytes={} passes={} load_ms={} scan_ms={} checked={} present={}",
        settings.chat_count(),
        settings.setting_count(),
        settings.setting_bytes(),
        passes,
        load_ms,
        scan_started.elapsed().as_millis(),
        checked,
        present,
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result {
    let log_value = match env::var("LOG") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => return Err("LOG is not valid Unicode".into()),
    };
    simple_logger::SimpleLogger::new()
        .with_level(configured_log_level(log_value.as_deref())?)
        .init()?;
    if env::args().skip(1).any(|arg| arg == "--capacity-probe") {
        return capacity_probe().await;
    }
    handlers::nsfw::configure_from_environment()
        .map_err(|error| format!("model configuration: {error}"))?;
    if env::args().skip(1).any(|arg| arg == "--model-probe") {
        let classifier = handlers::nsfw::capacity_probe();
        let ocr = handlers::ocr::capacity_probe();
        let vision = handlers::vision::present();
        println!(
            "model-probe: nsfw_pools={} ocr_pools={} vision_pool={}",
            classifier,
            ocr,
            usize::from(vision),
        );
        return Ok(());
    }

    let update_concurrency =
        configured_cap("UPDATE_CONCURRENCY", DEFAULT_UPDATE_CONCURRENCY, 32, 4096)?;
    let cleaner_concurrency =
        configured_cap("CLEANER_CONCURRENCY", DEFAULT_CLEANER_CONCURRENCY, 16, 1024)?;
    log::info!(
        "runtime admission: updates={update_concurrency}, cleaner={cleaner_concurrency}, update_queue={}",
        UPDATES_CHANNEL_CAPACITY
    );

    let api_id = parse_api_id(&required_nonempty_env("TG_ID")?)?;
    let api_hash = required_nonempty_env("TG_HASH")?;
    let token = required_nonempty_env("TG_BOT_TOKEN")?;
    let sudo_id = optional_user_id("SUDO_ID")?;

    let database_url = required_nonempty_env("DATABASE_URL")?;
    let miniapp_config = miniapp::config(&token).map_err(|error| format!("miniapp: {error}"))?;
    let miniapp_link = miniapp_config
        .as_ref()
        .map(|config| config.link().to_owned());
    let runtime_config = RuntimeConfig::from_environment(sudo_id, api_hash.clone(), miniapp_link)
        .map_err(|error| format!("runtime configuration: {error}"))?;
    runtime_config
        .validate()
        .await
        .map_err(|error| format!("runtime configuration: {error}"))?;
    let max_shard_chats =
        configured_cap("MAX_SHARD_CHATS", DEFAULT_MAX_SHARD_CHATS, 1_000, 500_000)?;
    let settings = Arc::new(
        Settings::connect_with_chat_limit_and_process_lock(&database_url, Some(max_shard_chats))
            .await?,
    );
    let mut owner_monitor = tokio::spawn(Arc::clone(&settings).monitor_ownership());
    settings
        .recover_stats(|chat, previous_total, total, awarded| {
            if settings.is_locked(chat, handlers::stats::RANKS) {
                handlers::stats::rank_award_milestone(previous_total, total, awarded)
            } else {
                None
            }
        })
        .await?;

    match settings.import_file(SETTINGS_FILE).await? {
        0 => {}
        n => println!("imported {n} settings from {SETTINGS_FILE}"),
    }
    let normalized = settings
        .try_normalize_fixed_image_filters(handlers::imgfilter::FIXED_MODEL_CUT)
        .await?;
    if normalized > 0 {
        println!(
            "image filters: normalized {normalized} fixed cut(s) to {:.3}",
            handlers::imgfilter::FIXED_MODEL_CUT
        );
    }
    let space_flag = format!("vision:{}", handlers::vision::CHECKPOINT);
    let migrating = !settings.is_locked(0, &space_flag);
    if migrating {
        println!(
            "vision: the database's vectors are not from {}; migrating",
            handlers::vision::CHECKPOINT
        );
        let examples = settings.try_example_image_filter_keys().await?;
        for (chat, name) in &examples {
            settings
                .try_set(
                    *chat,
                    &format!("{}{name}", handlers::imgfilter::PREFIX),
                    false,
                )
                .await?;
            settings.try_delete_image_filter(*chat, name).await?;
            println!(
                "vision: retired example filter «{name}» in {chat}; its pictures are from the old space"
            );
        }
        settings.try_clear_samples().await?;
    }
    let fixed_filters = if migrating {
        settings.try_phrase_image_filter_keys().await?
    } else {
        settings.try_fixed_image_filter_keys().await?
    };
    let mut refreshed = 0usize;
    for (chat, name) in fixed_filters {
        let row = settings
            .try_image_filter(chat, &name)
            .await?
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "vision migration row disappeared for {chat}/{name}"
                ))
            })?;
        let phrase = name.clone();
        let vector = tokio::task::spawn_blocking(move || handlers::imgtext::embed(&phrase))
            .await
            .map_err(|error| {
                std::io::Error::other(format!("vision embedding worker failed: {error}"))
            })?
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "vision embedding unavailable while migrating {chat}/{name}"
                ))
            })?;
        let (bytes, scale) = handlers::imgfilter::quantize(&vector);
        let saved = settings
            .try_save_image_filter(
                chat,
                ImageFilterWrite {
                    name: &name,
                    vector: &bytes,
                    scale,
                    cut: row.cut,
                    rate: row.rate,
                    live: row.live,
                    samples: row.samples,
                    calibrated: row.calibrated,
                },
            )
            .await?;
        if !saved {
            return Err(std::io::Error::other(format!(
                "vision migration could not retain {chat}/{name}: image-filter capacity reached"
            ))
            .into());
        }
        refreshed += 1;
    }
    if refreshed > 0 {
        println!("image filters: refreshed {refreshed} phrase vector(s)");
    }
    if migrating {
        settings.try_set(0, &space_flag, true).await?;
        println!(
            "vision: database vectors are now from {}",
            handlers::vision::CHECKPOINT
        );
    }
    let shard_chats = settings.chat_count();
    if shard_chats > max_shard_chats {
        return Err(format!(
            "shard has {shard_chats} configured chats, above MAX_SHARD_CHATS={max_shard_chats}; split the database across fleet shards"
        )
        .into());
    }
    let shard_name = optional_nonempty_env("SHARD_NAME")?;
    let allowed_chats = optional_nonempty_env("SHARD_CHAT_IDS_FILE")?
        .map(|path| load_owned_chats(&path, max_shard_chats))
        .transpose()?;
    if shard_name.is_some() && allowed_chats.is_none() {
        return Err(
            "SHARD_NAME is set, but SHARD_CHAT_IDS_FILE is missing; a templated fleet shard must have an ownership file"
                .into(),
        );
    }
    if let Some(allowed) = &allowed_chats
        && let Some(chat) = settings
            .chats()
            .into_iter()
            .find(|chat| !allowed.contains(chat))
    {
        return Err(format!(
            "database contains chat {chat}, but SHARD_CHAT_IDS_FILE does not assign it to this shard"
        )
        .into());
    }

    let session = Arc::new(SqliteSession::open(SESSION_FILE).await?);
    let params = ConnectionParams {
        updates_channel_capacity: UPDATES_CHANNEL_CAPACITY,
        updates_overflow_policy: UpdatesOverflowPolicy::RecoverGap,
        request_gate: Some(settings.liveness()),
        ..Default::default()
    };
    let SenderPool {
        runner,
        updates,
        handle,
    } = SenderPool::with_configuration(Arc::clone(&session), api_id, params);
    let client = Client::new(handle.clone());
    let mut pool_task = tokio::spawn(runner.run());

    let me = if client.is_authorized().await? {
        client
            .get_me()
            .await
            .map_err(|error| format!("could not read the bot's own identity: {error}"))?
    } else {
        let me = client.bot_sign_in(&token, &api_hash).await?;
        println!("signed in");
        me
    };
    if let Some(config) = miniapp_config.as_ref() {
        config
            .validate_bot_username(me.username())
            .map_err(|error| format!("miniapp: {error}"))?;
    }
    let Some(me_id) = me.id().bare_id().and_then(NonZeroI64::new) else {
        return Err("Telegram returned an invalid zero bot identity".into());
    };
    let bot_identity = BotIdentity::new(me_id, me.username().map(str::to_owned));
    let ctx = Arc::new(Ctx::new_with_allowed_chats(
        client.clone(),
        Arc::clone(&settings),
        grammers_session::storages::erase(Arc::clone(&session)),
        bot_identity,
        runtime_config,
        max_shard_chats,
        allowed_chats,
    ));
    handlers::premium::initialize(&client)
        .await
        .map_err(|error| format!("premium emoji: {error}"))?;
    let mut background_tasks = tokio::task::JoinSet::new();
    let mut miniapp_stop = None;
    let mut miniapp_task = None;
    if let Some(config) = miniapp_config {
        let bound = miniapp::bind(config).await?;
        let miniapp_ctx = Arc::clone(&ctx);
        let (stop, stopping) = tokio::sync::watch::channel(false);
        miniapp_stop = Some(stop);
        miniapp_task = Some(tokio::spawn(miniapp::serve(miniapp_ctx, bound, stopping)));
    }

    let update_telemetry = Arc::new(UpdateTelemetry::default());
    let (bot_stop, bot_stopping) = tokio::sync::watch::channel(UpdateShutdown::Running);
    let (bot_checkpoint, bot_checkpoints) = tokio::sync::mpsc::channel(1);
    let mut bot_worker = tokio::spawn(run_bot_updates(
        Arc::clone(&ctx),
        client.clone(),
        updates,
        update_concurrency,
        Arc::clone(&update_telemetry),
        bot_stopping,
        bot_checkpoints,
    ));
    handlers::stats::recover_awards(&ctx).await;

    let user_session = Arc::new(SqliteSession::open(USER_SESSION_FILE).await?);
    let SenderPool {
        runner: user_runner,
        updates: user_updates,
        handle: user_handle,
    } = SenderPool::with_configuration(
        Arc::clone(&user_session),
        api_id,
        ConnectionParams {
            updates_channel_capacity: UPDATES_CHANNEL_CAPACITY,
            updates_overflow_policy: UpdatesOverflowPolicy::RecoverGap,
            request_gate: Some(settings.liveness()),
            ..Default::default()
        },
    );

    let mut user_task = tokio::spawn(user_runner.run());
    let user_client = Client::new(user_handle.clone());
    let cleaner_telemetry = Arc::new(UpdateTelemetry::default());
    match user_client.is_authorized().await {
        Ok(true) => match user_client.get_me().await {
            Ok(me) => {
                let Some(id) = me.id().bare_id() else {
                    return Err("Telegram returned an invalid zero cleaner identity".into());
                };
                ctx.set_cleaner_id(id);
                println!("cleaner signed in as {}", me.full_name());
            }
            Err(e) => eprintln!("cleaner: signed in but unreachable: {e}"),
        },
        Ok(false) => println!("cleaner not signed in — send «ورود کلینر» to the bot"),
        Err(e) => eprintln!("cleaner: {e}"),
    }
    ctx.set_user_client(user_client.clone());

    let (cleaner_stop, mut cleaner_stopping) = tokio::sync::watch::channel(UpdateShutdown::Running);
    let (cleaner_checkpoint, mut cleaner_checkpoints) = tokio::sync::mpsc::channel(1);
    let cleaner_ctx = Arc::clone(&ctx);
    let cleaner_worker_telemetry = Arc::clone(&cleaner_telemetry);
    let mut cleaner_worker = tokio::spawn(async move {
        let mut updates = user_client
            .stream_updates(
                user_updates,
                UpdatesConfiguration {
                    catch_up: true,
                    drop_idle_channels: true,
                },
            )
            .await
            .map_err(UpdateWorkerError::Initialize)?;
        let mut tasks = dispatcher::Dispatcher::<grammers_client::update::Message>::new(
            cleaner_concurrency,
            handlers::PER_CHAT_UPDATES,
            UPDATES_CHANNEL_CAPACITY.get(),
        );
        'updates: loop {
            let mut handler_panics = 0usize;
            let mut checkpoint = None;
            let mut receive_error = loop {
                while let Some(finished) = tasks.try_join_next() {
                    handler_panics += usize::from(!cleaner_worker_telemetry.complete(finished));
                }
                tasks.start(|message| {
                    let ctx = Arc::clone(&cleaner_ctx);
                    async move {
                        handlers::bots::on_cleaner_message(&ctx, &message).await;
                    }
                });
                cleaner_worker_telemetry
                    .active
                    .store(tasks.active() as u64, Ordering::Relaxed);
                cleaner_worker_telemetry
                    .queued
                    .store(tasks.pending() as u64, Ordering::Relaxed);
                let update = tokio::select! {
                    _ = cleaner_stopping.changed() => break None,
                    command = cleaner_checkpoints.recv() => {
                        checkpoint = command;
                        break None;
                    }
                    finished = tasks.join_next(), if tasks.active() > 0 => {
                        if let Some(finished) = finished {
                            handler_panics +=
                                usize::from(!cleaner_worker_telemetry.complete(finished));
                        }
                        continue;
                    }
                    update = updates.next(), if tasks.has_room() => update,
                };
                match update {
                    Ok(grammers_client::update::Update::NewMessage(message)) => {
                        if !handlers::bots::cleaner_candidate(&cleaner_ctx, &message) {
                            continue;
                        }
                        let key = message.peer_id().bot_api_dialog_id().unwrap_or(0);
                        cleaner_worker_telemetry
                            .received
                            .fetch_add(1, Ordering::Relaxed);
                        tasks.push(key, message);
                    }
                    Ok(_) => {}
                    Err(error) => break Some(error),
                }
            };
            let mut cursor_error = if matches!(
                receive_error.as_ref(),
                Some(grammers_client::InvocationError::Session(_))
            ) {
                receive_error.take()
            } else {
                None
            };

            while cursor_error.is_none() {
                while let Some(finished) = tasks.try_join_next() {
                    handler_panics += usize::from(!cleaner_worker_telemetry.complete(finished));
                }
                tasks.start(|message| {
                    let ctx = Arc::clone(&cleaner_ctx);
                    async move {
                        handlers::bots::on_cleaner_message(&ctx, &message).await;
                    }
                });
                if *cleaner_stopping.borrow() == UpdateShutdown::Abort {
                    let unfinished = tasks.shutdown().await;
                    return Err(UpdateWorkerError::DrainAborted { unfinished });
                }
                if !tasks.has_room() {
                    tokio::select! {
                        changed = cleaner_stopping.changed() => {
                            if changed.is_err()
                                || *cleaner_stopping.borrow() == UpdateShutdown::Abort
                            {
                                let unfinished = tasks.shutdown().await;
                                return Err(UpdateWorkerError::DrainAborted { unfinished });
                            }
                        }
                        finished = tasks.join_next() => {
                            if let Some(finished) = finished {
                                handler_panics += usize::from(
                                    !cleaner_worker_telemetry.complete(finished),
                                );
                            }
                        }
                    }
                    continue;
                }
                let buffered = tokio::select! {
                    changed = cleaner_stopping.changed() => {
                        if changed.is_err()
                            || *cleaner_stopping.borrow() == UpdateShutdown::Abort
                        {
                            let unfinished = tasks.shutdown().await;
                            return Err(UpdateWorkerError::DrainAborted { unfinished });
                        }
                        continue;
                    }
                    update = updates.next_buffered() => update,
                };
                let update = match buffered {
                    Ok(Some(update)) => update,
                    Ok(None) => break,
                    Err(error) => {
                        cursor_error = Some(error);
                        break;
                    }
                };
                if let grammers_client::update::Update::NewMessage(message) = update
                    && handlers::bots::cleaner_candidate(&cleaner_ctx, &message)
                {
                    let key = message.peer_id().bot_api_dialog_id().unwrap_or(0);
                    cleaner_worker_telemetry
                        .received
                        .fetch_add(1, Ordering::Relaxed);
                    tasks.push(key, message);
                }
            }
            while !tasks.is_empty() {
                if *cleaner_stopping.borrow() == UpdateShutdown::Abort {
                    let unfinished = tasks.shutdown().await;
                    return Err(UpdateWorkerError::DrainAborted { unfinished });
                }
                tasks.start(|message| {
                    let ctx = Arc::clone(&cleaner_ctx);
                    async move {
                        handlers::bots::on_cleaner_message(&ctx, &message).await;
                    }
                });
                tokio::select! {
                    changed = cleaner_stopping.changed() => {
                        if changed.is_err()
                            || *cleaner_stopping.borrow() == UpdateShutdown::Abort
                        {
                            let unfinished = tasks.shutdown().await;
                            return Err(UpdateWorkerError::DrainAborted { unfinished });
                        }
                    }
                    finished = tasks.join_next() => {
                        if let Some(finished) = finished {
                            handler_panics +=
                                usize::from(!cleaner_worker_telemetry.complete(finished));
                        }
                    }
                }
            }
            if handler_panics != 0 {
                return Err(UpdateWorkerError::HandlerPanicked {
                    count: handler_panics,
                });
            }
            if let Some(error) = cursor_error {
                return Err(UpdateWorkerError::Receive(error));
            }
            if let Some(command) = checkpoint {
                match pause_at_checkpoint(&mut updates, command, &mut cleaner_stopping).await? {
                    CheckpointPause::Resume => continue 'updates,
                    CheckpointPause::Drain => {}
                }
            }
            return Ok(UpdateWorkerOutcome {
                updates,
                receive_error,
            });
        }
    });

    handlers::join::prime(&ctx).await?;
    handlers::tempmedia::restore(&ctx).await?;

    let (background_stop, background_stopping) = tokio::sync::watch::channel(false);
    background_tasks.spawn(handlers::cases::run_retention(
        Arc::clone(&ctx),
        background_stopping.clone(),
    ));
    let autoconfig_ctx = Arc::clone(&ctx);
    background_tasks.spawn(async move {
        let _epoch = autoconfig_ctx.background_epoch().await;
        handlers::autoconfig::recover_startup(Arc::clone(&autoconfig_ctx)).await;
    });

    let pending_warn_ctx = Arc::clone(&ctx);
    let mut pending_warn_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = pending_warn_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = pending_warn_ctx.background_epoch().await;
                    handlers::warns::recover_pending(&pending_warn_ctx).await;
                },
            }
        }
    });

    let pending_strict_ctx = Arc::clone(&ctx);
    let mut pending_strict_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = pending_strict_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = pending_strict_ctx.background_epoch().await;
                    handlers::strict::recover_pending(&pending_strict_ctx).await;
                },
            }
        }
    });

    let captcha_ctx = Arc::clone(&ctx);
    let mut captcha_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(DEFERRED_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = captcha_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = captcha_ctx.background_epoch().await;
                    handlers::captcha::sweep(&captcha_ctx).await;
                },
            }
        }
    });

    let cleaner_recommend_ctx = Arc::clone(&ctx);
    let mut cleaner_recommend_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cleaner_recommend_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = cleaner_recommend_ctx.background_epoch().await;
                    handlers::cleaner_setup::run_recommendations(&cleaner_recommend_ctx).await;
                },
            }
        }
    });

    let rights_ctx = Arc::clone(&ctx);
    let mut rights_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = rights_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = rights_ctx.background_epoch().await;
                    handlers::rights::run_due(&rights_ctx).await;
                },
            }
        }
    });

    let report_ctx = Arc::clone(&ctx);
    let mut report_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = report_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = report_ctx.background_epoch().await;
                    handlers::stats::run_daily(&report_ctx).await;
                },
            }
        }
    });

    let purge_ctx = Arc::clone(&ctx);
    let mut purge_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = purge_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = purge_ctx.background_epoch().await;
                    handlers::purge::run_auto(&purge_ctx).await;
                },
            }
        }
    });

    let media_ctx = Arc::clone(&ctx);
    let mut media_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(TEMP_MEDIA_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = media_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = media_ctx.background_epoch().await;
                    handlers::tempmedia::sweep(&media_ctx).await;
                },
            }
        }
    });

    let deferred_ctx = Arc::clone(&ctx);
    let mut deferred_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(DEFERRED_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = deferred_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = deferred_ctx.background_epoch().await;
                    handlers::tempmedia::sweep_deferred(&deferred_ctx).await;
                },
            }
        }
    });

    let log_ctx = Arc::clone(&ctx);
    let mut log_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(LOG_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = log_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = log_ctx.background_epoch().await;
                    if !handlers::log::flush(&log_ctx).await {
                        log::warn!("log: delivery batch retained for retry");
                    }
                }
            }
        }
    });

    let stats_ctx = Arc::clone(&ctx);
    let mut stats_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut flushes: u32 = 1;
        let mut prune_due = false;
        let mut tick = tokio::time::interval(STATS_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = stats_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = stats_ctx.background_epoch().await;
                    handlers::stats::flush(&stats_ctx).await;

                    if flushes.is_multiple_of(1440) {
                        prune_due = true;
                    }
                    if prune_due {
                        match handlers::stats::prune(&stats_ctx).await {
                            Ok(_) => prune_due = false,
                            Err(error) => {
                                log::warn!("counter prune failed; retrying next stats tick: {error}");
                            }
                        }
                    }
                    flushes += 1;
                }
            }
        }
    });

    let sweep_ctx = Arc::clone(&ctx);
    let mut sweep_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(CHAT_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = sweep_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = sweep_ctx.background_epoch().await;
                    match sweep_ctx.evict_idle(CHAT_IDLE) {
                        0 => {}
                        dropped => log::debug!("forgot the state of {dropped} quiet chats"),
                    }
                }
            }
        }
    });

    ctx.load_samples().await?;

    let capacity_ctx = Arc::clone(&ctx);
    let cleaner_capacity = Arc::clone(&cleaner_telemetry);
    let capacity_telemetry = Arc::clone(&update_telemetry);
    let mut capacity_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = capacity_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = capacity_ctx.background_epoch().await;
                    let snapshot = capacity_ctx.capacity_snapshot();
                    let updates_received = capacity_telemetry.received.swap(0, Ordering::Relaxed);
                    let updates_completed = capacity_telemetry.completed.swap(0, Ordering::Relaxed);
                    let latency = UpdateTelemetry::percentiles(&capacity_telemetry.latency);
                    let queue_wait = UpdateTelemetry::percentiles(&capacity_telemetry.queue_wait);
                    log::info!(
                "dispatch: queued={} cleaner_queued={} failures={} transport_dropped_batches_total={} stale_updates_total={} latency_us_upper_p50_p95_p99={latency:?} queue_wait_us_upper_p50_p95_p99={queue_wait:?}",
                capacity_telemetry.queued.load(Ordering::Relaxed),
                cleaner_capacity.queued.load(Ordering::Relaxed),
                capacity_telemetry.failed.swap(0, Ordering::Relaxed),
                grammers_client::sender::dropped_update_batches(),
                handlers::STALE_UPDATES.load(Ordering::Relaxed)
                    );
                    let (db_connections, db_idle) = capacity_ctx.settings.pool_stats();
                    let (counter_rows, note_rows) = match capacity_ctx.settings.durable_counts().await {
                        Ok((counter_rows, note_rows)) => {
                            (counter_rows.to_string(), note_rows.to_string())
                        }
                        Err(error) => {
                            log::warn!("capacity: durable row telemetry unavailable: {error}");
                            ("unknown".to_owned(), "unknown".to_owned())
                        }
                    };
                    log::info!(
                "capacity: runtime_chats={} settings_chats={}/{} settings_rows={} settings_bytes={} counter_rows={} note_rows={} user_chats={} dirty={}/{}/{} pending_writes={} pending_drops={} deferred={} pending_admins={} deleted={} verdicts={} voice={}/{} outbound={}/{} critical_waiting={} update_active={}/{} updates_received={} updates_completed={} cleaner_active={}/{} db_connections={} db_idle={}",
                snapshot.runtime_chats,
                snapshot.settings_chats,
                max_shard_chats,
                snapshot.settings_rows,
                snapshot.settings_bytes,
                counter_rows,
                note_rows,
                snapshot.user_chats,
                snapshot.dirty_logs,
                snapshot.dirty_media,
                snapshot.dirty_stats,
                snapshot.pending_writes,
                snapshot.pending_drops,
                snapshot.deferred,
                snapshot.pending_admins,
                snapshot.deleted,
                snapshot.verdicts,
                snapshot.voice_verdicts,
                snapshot.filtered_voices,
                snapshot.outbound_active,
                snapshot.outbound_waiting,
                snapshot.outbound_critical_waiting,
                capacity_telemetry.active.load(Ordering::Relaxed),
                update_concurrency,
                updates_received,
                updates_completed,
                cleaner_capacity.active.load(Ordering::Relaxed),
                cleaner_concurrency,
                db_connections,
                        db_idle,
                    );
                }
            }
        }
    });

    let samples_ctx = Arc::clone(&ctx);
    let mut samples_stopping = background_stopping.clone();
    background_tasks.spawn(async move {
        let mut tick = tokio::time::interval(SAMPLE_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = samples_stopping.changed() => break,
                _ = tick.tick() => {
                    let _epoch = samples_ctx.background_epoch().await;
                    if !samples_ctx.flush_samples().await {
                        log::warn!("calibration: sample persistence retained for retry");
                    }
                }
            }
        }
    });

    let badges_ctx = Arc::clone(&ctx);
    background_tasks.spawn(async move {
        let _epoch = badges_ctx.background_epoch().await;
        handlers::stats::sweep_badges(&badges_ctx).await;
    });

    println!("running");
    let mut shutdown = tokio::spawn(shutdown_signal());
    let mut shutdown_requested_at = None;
    let mut checkpoint_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + UPDATE_CHECKPOINT,
        UPDATE_CHECKPOINT,
    );
    checkpoint_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut bot_worker_result = None;
    let mut cleaner_worker_result = None;
    let mut miniapp_result = None;
    let mut background_error = None;
    let mut shutdown_failure = None;
    loop {
        tokio::select! {
            result = &mut shutdown => {
                shutdown_requested_at = Some(result??);
                break;
            },
            _ = checkpoint_tick.tick() => {
                let deadline = tokio::time::Instant::now() + UPDATE_CHECKPOINT_BUDGET;
                match periodic_update_checkpoint(
                    &ctx,
                    &bot_checkpoint,
                    &cleaner_checkpoint,
                    &mut background_tasks,
                    deadline,
                ).await {
                    Ok(PeriodicCheckpointOutcome::Committed) => {
                        log::info!("Telegram update checkpoint committed");
                    }
                    Ok(PeriodicCheckpointOutcome::Deferred { reason }) => {
                        log::warn!("Telegram update checkpoint deferred: {reason}");
                    }
                    Err(error) => {
                        log::error!("Telegram update checkpoint failed: {error}");
                        shutdown_failure = Some(error.to_string());
                        break;
                    }
                }
            }
            _ = settings.ownership_lost() => {
                close_cursor_gate_for_ownership_loss(&mut shutdown_failure);
                break;
            },
            _ = ctx.persistence_barrier_failed() => {
                shutdown_failure = Some(
                    "correctness-critical persistence failed; update intake stopped and Telegram cursors were not persisted"
                        .to_owned(),
                );
                break;
            },
            finished = &mut bot_worker => {
                bot_worker_result = Some(finished);
                break;
            }
            finished = &mut cleaner_worker => {
                cleaner_worker_result = Some(finished);
                break;
            }
            finished = background_tasks.join_next(), if !background_tasks.is_empty() => {
                if let Some(Err(error)) = finished {
                    log::error!("owned background task failed: {error}");
                    background_error = Some(error.to_string());
                    break;
                }
            }
            finished = wait_optional_task(&mut miniapp_task) => {
                let finished = finished.expect("a missing Mini App task never completes");
                let detail = match &finished {
                    Ok(Ok(())) => "miniapp server stopped unexpectedly".to_owned(),
                    Ok(Err(error)) => format!("miniapp server exited: {error}"),
                    Err(error) => format!("miniapp server task failed: {error}"),
                };
                log::error!("{detail}");
                miniapp_result = Some(finished);
                background_error = Some(detail);
                break;
            }
        }
    }

    if shutdown_requested_at.is_none() {
        shutdown_requested_at = stop_shutdown_watcher(&mut shutdown).await?;
    }
    let shutdown_started = shutdown_requested_at.unwrap_or_else(tokio::time::Instant::now);
    let drain_deadline = shutdown_started + SHUTDOWN_DRAIN_BUDGET;
    let shutdown_deadline = shutdown_started + SHUTDOWN_TOTAL_BUDGET;
    let abort_deadline = shutdown_started + SHUTDOWN_ABORT_BUDGET;
    let mut owned_graceful = false;

    if bot_stop.send(UpdateShutdown::Drain).is_err() && bot_worker_result.is_none() {
        log::warn!("shutdown: bot update worker had already stopped");
    }
    if cleaner_stop.send(UpdateShutdown::Drain).is_err() && cleaner_worker_result.is_none() {
        log::warn!("shutdown: cleaner worker had already stopped");
    }
    if let Some(stop) = &miniapp_stop
        && stop.send(true).is_err()
        && miniapp_result.is_none()
    {
        log::warn!("shutdown: Mini App server had already stopped");
    }
    if background_stop.send(true).is_err() && !background_tasks.is_empty() {
        log::warn!("shutdown: background stop signal had no receivers");
    }

    let background_graceful = tokio::time::timeout_at(drain_deadline, async {
        while let Some(finished) = background_tasks.join_next().await {
            if let Err(error) = finished {
                log::error!("owned background task failed during shutdown: {error}");
                background_error.get_or_insert_with(|| error.to_string());
            }
        }
    })
    .await
    .is_ok();
    if !background_graceful {
        if tokio::time::timeout_at(shutdown_deadline, background_tasks.shutdown())
            .await
            .is_err()
        {
            let _ = tokio::time::timeout_at(abort_deadline, background_tasks.shutdown()).await;
        }
        shutdown_failure = Some(
            "background work did not reach a cancellation-safe boundary before the shutdown deadline; Telegram cursors were not persisted"
                .to_owned(),
        );
    }
    if let Some(error) = &background_error {
        shutdown_failure.get_or_insert_with(|| {
            format!(
                "background work failed ({error}); volatile state and Telegram cursors were not persisted"
            )
        });
    }

    let sources_graceful = shutdown_failure.is_none()
        && tokio::time::timeout_at(drain_deadline, async {
            if bot_worker_result.is_none() {
                bot_worker_result = Some((&mut bot_worker).await);
            }
            if cleaner_worker_result.is_none() {
                cleaner_worker_result = Some((&mut cleaner_worker).await);
            }
            if miniapp_result.is_none()
                && let Some(task) = miniapp_task.as_mut()
            {
                miniapp_result = Some(task.await);
            }
        })
        .await
        .is_ok();

    let mut sources_stopped = sources_graceful;
    if shutdown_failure.is_none() && !sources_graceful {
        log::warn!(
            "shutdown: graceful request drain budget exhausted; cancelling admitted updates"
        );
        if bot_worker_result.is_none() {
            let _ = bot_stop.send(UpdateShutdown::Abort);
        }
        if cleaner_worker_result.is_none() {
            let _ = cleaner_stop.send(UpdateShutdown::Abort);
        }
        sources_stopped = tokio::time::timeout_at(shutdown_deadline, async {
            if bot_worker_result.is_none() {
                bot_worker_result = Some((&mut bot_worker).await);
            }
            if cleaner_worker_result.is_none() {
                cleaner_worker_result = Some((&mut cleaner_worker).await);
            }
            if miniapp_result.is_none()
                && let Some(task) = miniapp_task.as_mut()
            {
                miniapp_result = Some(task.await);
            }
        })
        .await
        .is_ok();
        if !sources_stopped {
            shutdown_failure =
                Some("request sources did not stop before the hard shutdown deadline".to_owned());
        }
    }

    if sources_stopped {
        let mut owned = ctx.begin_owned_drain();
        owned_graceful = if sources_graceful {
            matches!(
                tokio::time::timeout_at(drain_deadline, owned.join()).await,
                Ok(0)
            )
        } else {
            false
        };
        if !owned_graceful {
            shutdown_failure.get_or_insert_with(|| {
                "owned handler drain timed out or observed a task failure; volatile state and Telegram cursors were not persisted (protocol replay remains subject to application freshness/idempotency policy)"
                    .to_owned()
            });
            if tokio::time::timeout_at(shutdown_deadline, owned.abort())
                .await
                .is_err()
            {
                let _ = tokio::time::timeout_at(abort_deadline, owned.abort()).await;
                shutdown_failure = Some(
                    "owned handler tasks did not stop before the hard shutdown deadline; Telegram cursors were not persisted"
                        .to_owned(),
                );
            }
        }
    }

    if !sources_stopped {
        if bot_worker_result.is_none() {
            bot_worker.abort();
        }
        if cleaner_worker_result.is_none() {
            cleaner_worker.abort();
        }
        if miniapp_result.is_none()
            && let Some(task) = miniapp_task.as_mut()
        {
            task.abort();
        }
        let _ = tokio::time::timeout_at(abort_deadline, async {
            if bot_worker_result.is_none() {
                bot_worker_result = Some((&mut bot_worker).await);
            }
            if cleaner_worker_result.is_none() {
                cleaner_worker_result = Some((&mut cleaner_worker).await);
            }
            if miniapp_result.is_none()
                && let Some(task) = miniapp_task.as_mut()
            {
                miniapp_result = Some(task.await);
            }
        })
        .await;
        let mut owned = ctx.begin_owned_drain();
        let _ = tokio::time::timeout_at(abort_deadline, owned.abort()).await;
    }

    let mut update_error: Option<Box<dyn std::error::Error + Send + Sync>> = None;
    let bot_update_outcome =
        retain_update_worker_outcome("bot", bot_worker_result, &mut update_error);
    let cleaner_update_outcome =
        retain_update_worker_outcome("cleaner", cleaner_worker_result, &mut update_error);

    if shutdown_failure.is_none() && ctx.cursor_barrier_poisoned() {
        shutdown_failure = Some(
            "a bounded commit-critical queue overflowed; Telegram cursors were not persisted"
                .to_owned(),
        );
    }

    if shutdown_failure.is_none() {
        match tokio::time::timeout_at(shutdown_deadline, async {
            ctx.shutdown_voice().await;
            let media_flushed = handlers::tempmedia::flush_pending(&ctx).await;
            let statistics_flushed = handlers::stats::flush(&ctx).await;
            let logs_flushed = handlers::log::flush_all(&ctx).await;
            let samples_flushed = ctx.flush_samples().await;
            media_flushed && statistics_flushed && logs_flushed && samples_flushed
        })
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                shutdown_failure = Some(
                    "final persistence retained volatile statistics, log, calibration, or temp-media work; Telegram cursors were not persisted (protocol replay remains subject to application freshness/idempotency policy)"
                        .to_owned(),
                );
            }
            Err(_) => {
                shutdown_failure = Some(
                    "final persistence exceeded the hard shutdown deadline; durable outboxes remain recoverable, but unstaged counter deltas, in-memory logs, or latest calibration samples may be lost"
                        .to_owned(),
                );
            }
        }
    } else {
        let _ = tokio::time::timeout_at(abort_deadline, ctx.shutdown_voice()).await;
    }

    if cursor_commit_allowed(&shutdown_failure, owned_graceful) {
        let synchronize = async {
            tokio::join!(
                async {
                    match bot_update_outcome {
                        Some(outcome) => Some(outcome.synchronize().await),
                        None => None,
                    }
                },
                async {
                    match cleaner_update_outcome {
                        Some(outcome) => Some(outcome.synchronize().await),
                        None => None,
                    }
                }
            )
        };
        match tokio::time::timeout_at(shutdown_deadline, synchronize).await {
            Ok((bot, cleaner)) => {
                for (label, result) in [("bot", bot), ("cleaner", cleaner)] {
                    if let Some(Err(error)) = result {
                        log::error!("{label} update worker could not commit its cursor: {error}");
                        if update_error.is_none() {
                            update_error = Some(Box::new(error));
                        }
                    }
                }
            }
            Err(_) => {
                shutdown_failure = Some(
                    "Telegram cursor persistence exceeded the hard shutdown deadline; the last contiguous protocol checkpoint remains active (re-delivery is subject to application freshness/idempotency policy)"
                        .to_owned(),
                );
            }
        }
    } else {
        drop(bot_update_outcome);
        drop(cleaner_update_outcome);
    }

    if miniapp_stop.is_some() {
        match miniapp_result {
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(error))) => {
                log::error!("Mini App server failed during shutdown: {error}");
                if background_error.is_none() {
                    background_error = Some(format!("miniapp server exited: {error}"));
                }
            }
            Some(Err(error)) if error.is_cancelled() && shutdown_failure.is_some() => {}
            Some(Err(error)) => {
                log::error!("Mini App server task failed: {error}");
                if background_error.is_none() {
                    background_error = Some(format!("miniapp server task failed: {error}"));
                }
            }
            None => {
                shutdown_failure.get_or_insert_with(|| {
                    "Mini App server outcome was not observed during shutdown".to_owned()
                });
            }
        }
    }
    handle.quit();
    user_handle.quit();
    owner_monitor.abort();
    match tokio::time::timeout_at(shutdown_deadline, &mut owner_monitor).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error.is_cancelled() => {}
        Ok(Err(error)) => log::error!("shutdown: ownership monitor failed: {error}"),
        Err(_) => {
            owner_monitor.abort();
            let _ = tokio::time::timeout_at(abort_deadline, &mut owner_monitor).await;
            shutdown_failure.get_or_insert_with(|| {
                "ownership monitor exceeded the hard shutdown deadline".to_owned()
            });
        }
    }
    match tokio::time::timeout_at(shutdown_deadline, async {
        tokio::join!(&mut pool_task, &mut user_task)
    })
    .await
    {
        Ok((bot, user)) => {
            if let Err(error) = bot {
                log::error!("shutdown: bot transport task failed: {error}");
            }
            if let Err(error) = user {
                log::error!("shutdown: cleaner transport task failed: {error}");
            }
        }
        Err(_) => {
            pool_task.abort();
            user_task.abort();
            let _ = tokio::time::timeout_at(abort_deadline, async {
                tokio::join!(&mut pool_task, &mut user_task)
            })
            .await;
            shutdown_failure.get_or_insert_with(|| {
                "Telegram transport tasks exceeded the hard shutdown deadline".to_owned()
            });
        }
    }
    if let Some(error) = shutdown_failure {
        return Err(error.into());
    }
    if let Some(error) = update_error {
        return Err(error);
    }
    if let Some(error) = background_error {
        return Err(format!("background task failed: {error}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CheckpointDecision, PeriodicCheckpointError, close_cursor_gate_for_ownership_loss,
        configured_log_level, cursor_commit_allowed, finish_periodic_cursor_commit,
        load_owned_chats, optional_user_id, parse_api_id, reap_completed_background_tasks,
        request_update_checkpoint, stop_shutdown_watcher, validate_nonempty_env_value,
    };
    use std::fs;

    #[test]
    fn route_file_is_strict_and_bounded() {
        let path = std::env::temp_dir().join(format!(
            "groupbot-route-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "# shard\n-100\n-200\n").unwrap();
        let loaded = load_owned_chats(path.to_str().unwrap(), 2).unwrap();
        assert!(loaded.contains(&-100));
        assert!(loaded.contains(&-200));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn route_file_rejects_tsv_and_duplicates() {
        let path = std::env::temp_dir().join(format!(
            "groupbot-route-invalid-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "-100\talpha\n-100\n").unwrap();
        assert!(load_owned_chats(path.to_str().unwrap(), 10).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn log_level_rejects_typos_instead_of_silently_using_warn() {
        assert_eq!(configured_log_level(None).unwrap(), log::LevelFilter::Warn);
        assert_eq!(
            configured_log_level(Some("error")).unwrap(),
            log::LevelFilter::Error
        );
        assert!(configured_log_level(Some("verbose")).is_err());
        assert!(configured_log_level(Some("INFO")).is_err());
    }

    #[test]
    fn telegram_user_id_range_is_not_approximated_by_an_i64_parse() {
        assert!(grammers_session::types::PeerId::user(1).is_some());
        assert!(grammers_session::types::PeerId::user(0).is_none());
        assert!(grammers_session::types::PeerId::user(i64::MAX).is_none());
        let _: fn(&str) -> super::Result<Option<i64>> = optional_user_id;
    }

    #[test]
    fn telegram_api_id_is_positive_and_bounded() {
        assert_eq!(parse_api_id("1").unwrap(), 1);
        assert_eq!(parse_api_id(&i32::MAX.to_string()).unwrap(), i32::MAX);
        assert!(parse_api_id("0").is_err());
        assert!(parse_api_id("-1").is_err());
        assert!(parse_api_id("2147483648").is_err());
        assert!(parse_api_id("").is_err());
    }

    #[test]
    fn required_environment_values_reject_invisible_padding() {
        assert!(validate_nonempty_env_value("TOKEN", "abc").is_ok());
        assert!(validate_nonempty_env_value("TOKEN", "").is_err());
        assert!(validate_nonempty_env_value("TOKEN", "   ").is_err());
        assert!(validate_nonempty_env_value("TOKEN", " abc").is_err());
        assert!(validate_nonempty_env_value("TOKEN", "abc\n").is_err());
    }

    #[tokio::test]
    async fn checkpoint_command_queue_is_bounded_and_cancellation_cannot_commit() {
        let (commands, mut worker) = tokio::sync::mpsc::channel(1);
        let pending = request_update_checkpoint("test", &commands).unwrap();
        assert!(matches!(
            request_update_checkpoint("test", &commands),
            Err(PeriodicCheckpointError::WorkerCommand { label: "test" })
        ));

        let command = worker.recv().await.unwrap();
        command.ready.send(()).unwrap();
        pending.ready.await.unwrap();
        drop(pending.decision);
        assert_eq!(
            command
                .decision
                .await
                .unwrap_or(CheckpointDecision::ResumeWithoutCommit),
            CheckpointDecision::ResumeWithoutCommit
        );
    }

    #[tokio::test]
    async fn checkpoint_error_path_preserves_an_already_observed_shutdown_time() {
        let requested_at = tokio::time::Instant::now() - std::time::Duration::from_secs(20);
        let mut watcher = tokio::spawn(async move { Ok(requested_at) });
        tokio::task::yield_now().await;

        assert_eq!(
            stop_shutdown_watcher(&mut watcher).await.unwrap(),
            Some(requested_at)
        );
    }

    #[test]
    fn ownership_loss_closes_the_final_cursor_gate_before_shutdown_work() {
        let mut failure = None;
        close_cursor_gate_for_ownership_loss(&mut failure);
        assert!(!cursor_commit_allowed(&failure, true));
        assert!(failure.unwrap().contains("ownership"));
    }

    #[test]
    fn post_authorization_cursor_failure_is_fatal_not_deferred() {
        let failure: super::Result = Err(std::io::Error::other("injected session failure").into());
        let error = match finish_periodic_cursor_commit(Ok(()), failure) {
            Err(error) => error,
            Ok(_) => panic!("post-authorization storage failure resumed the workers"),
        };
        assert!(matches!(
            error,
            PeriodicCheckpointError::CursorCommitFailed(message)
                if message.contains("cleaner cursor") && message.contains("injected session failure")
        ));
    }

    #[tokio::test]
    async fn checkpoint_rejects_a_background_panic_before_commit() {
        let mut background = tokio::task::JoinSet::new();
        background.spawn(async { panic!("injected background failure") });
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Err(error) = reap_completed_background_tasks(&mut background) {
                    break error;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the injected panic must become observable");
        assert!(matches!(
            result,
            PeriodicCheckpointError::BackgroundTaskFailed(_)
        ));
    }
}
