//! Bounded pool-level SQLite and deferred-cleanup progress.
//!
//! Typed call-site intent identifies only observed IMMEDIATE transactions.
//! No SQL formatting, extra SQL, business IDs or allocation payloads are read.
//! SQL callbacks copy into bounded slots; only the pool monitor emits logs.
use super::memory;
use contrix_core::error::AppError;
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{Acquire, Relaxed},
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Intent is supplied by known call sites, never inferred from SQL text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransactionStage {
    Unknown,
    BlobReservation,
    BlobRecovery,
    StorageReconcile,
    UploadExpiration,
    DocumentMetadata,
    GcClaim,
    GcFinalize,
    NativeHistoryRetention,
    HistoryRetention,
    OperationHistoryRetention,
}

impl TransactionStage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::BlobReservation => "blob_reservation",
            Self::BlobRecovery => "blob_recovery",
            Self::StorageReconcile => "storage_reconcile",
            Self::UploadExpiration => "upload_expiration",
            Self::DocumentMetadata => "document_metadata",
            Self::GcClaim => "gc_claim",
            Self::GcFinalize => "gc_finalize",
            Self::NativeHistoryRetention => "native_history_retention",
            Self::HistoryRetention => "history_retention",
            Self::OperationHistoryRetention => "operation_history_retention",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransactionPhase {
    BeginWait,
    Active,
    CommitWait,
    RollbackWait,
}

impl TransactionPhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::BeginWait => "begin_wait",
            Self::Active => "active",
            Self::CommitWait => "commit_wait",
            Self::RollbackWait => "rollback_wait",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TransactionSnapshot {
    pub(crate) stage: TransactionStage,
    pub(crate) phase: TransactionPhase,
    pub(crate) known_immediate: bool,
    pub(crate) finished: bool,
    pub(crate) begin_wait_us: u64,
    began_at: Instant,
    phase_since: Instant,
    elapsed_us: u64,
    phase_elapsed_us: u64,
}

impl TransactionSnapshot {
    pub(crate) fn begin_wait_us(self) -> u64 {
        if self.phase == TransactionPhase::BeginWait && !self.finished {
            self.phase_elapsed_us()
        } else {
            self.begin_wait_us
        }
    }
    pub(crate) fn phase_label(self) -> &'static str {
        match (self.known_immediate, self.phase) {
            (true, TransactionPhase::BeginWait) => "write_begin_wait",
            (true, TransactionPhase::Active) => "active_write",
            (true, TransactionPhase::CommitWait) => "write_commit_wait",
            (true, TransactionPhase::RollbackWait) => "write_rollback_wait",
            (false, TransactionPhase::Active) => "active_unknown",
            _ => self.phase.label(),
        }
    }
    pub(crate) fn elapsed_us(self) -> u64 {
        if self.finished {
            self.elapsed_us
        } else {
            micros(self.began_at.elapsed())
        }
    }

    fn phase_elapsed_us(self) -> u64 {
        if self.finished {
            self.phase_elapsed_us
        } else {
            micros(self.phase_since.elapsed())
        }
    }
}

pub(super) fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

tokio::task_local! {
    static TRANSACTION_INTENT: TransactionStage;
}

/// Exactly the original Diesel boundary; this scope changes observation only.
pub(crate) fn immediate_transaction<R, E>(
    connection: &mut diesel::SqliteConnection,
    stage: TransactionStage,
    callback: impl FnOnce(&mut diesel::SqliteConnection) -> Result<R, E>,
) -> Result<R, E>
where
    E: From<diesel::result::Error>,
{
    TRANSACTION_INTENT.sync_scope(stage, || connection.immediate_transaction(callback))
}

pub(crate) struct ConnectionTransactionObservation {
    connection_sequence: u64,
    current: Option<TransactionSnapshot>,
    reported_phase: bool,
    reported_any: bool,
    pending: Option<TransactionReport>,
    pending_failures: u64,
    pub(super) memory: Option<memory::Sample>,
    pub(super) memory_pending: bool,
    instrumentation_disposed: bool,
}

impl ConnectionTransactionObservation {
    pub(crate) fn begin(&mut self) {
        let now = Instant::now();
        let intent = TRANSACTION_INTENT.try_with(|stage| *stage).ok();
        self.current = Some(TransactionSnapshot {
            stage: intent.unwrap_or(TransactionStage::Unknown),
            phase: TransactionPhase::BeginWait,
            known_immediate: intent.is_some(),
            finished: false,
            begin_wait_us: 0,
            began_at: now,
            phase_since: now,
            elapsed_us: 0,
            phase_elapsed_us: 0,
        });
        self.reported_phase = false;
        self.reported_any = false;
    }

    pub(crate) fn change_phase(&mut self, phase: TransactionPhase) {
        if let Some(current) = &mut self.current {
            if current.phase == TransactionPhase::BeginWait {
                current.begin_wait_us = micros(current.phase_since.elapsed());
            }
            current.phase = phase;
            current.phase_since = Instant::now();
            self.reported_phase = false;
        }
    }

    pub(crate) fn snapshot(&self) -> Option<TransactionSnapshot> {
        self.current
    }

    pub(crate) fn finish(&mut self) -> Option<TransactionSnapshot> {
        self.current.take().map(|mut current| {
            current.finished = true;
            let finished_at = Instant::now();
            current.elapsed_us = micros(finished_at.duration_since(current.began_at));
            current.phase_elapsed_us = micros(finished_at.duration_since(current.phase_since));
            if current.phase == TransactionPhase::BeginWait {
                current.begin_wait_us = current.phase_elapsed_us;
            }
            current
        })
    }
}

#[derive(Clone, Copy)]
struct TransactionReport {
    connection_sequence: u64,
    snapshot: TransactionSnapshot,
    outcome: &'static str,
    elapsed_us: u64,
    phase_elapsed_us: u64,
    coalesced_failures: u64,
    begin_wait_us: u64,
    cause: Option<&'static str>,
}

impl TransactionReport {
    fn new(
        observation: &ConnectionTransactionObservation,
        snapshot: TransactionSnapshot,
        outcome: &'static str,
    ) -> Self {
        Self::new_at(observation, snapshot, outcome, Instant::now())
    }

    fn new_at(
        observation: &ConnectionTransactionObservation,
        snapshot: TransactionSnapshot,
        outcome: &'static str,
        observed_at: Instant,
    ) -> Self {
        let elapsed_us = if snapshot.finished {
            snapshot.elapsed_us
        } else {
            micros(observed_at.duration_since(snapshot.began_at))
        };
        let phase_elapsed_us = if snapshot.finished {
            snapshot.phase_elapsed_us
        } else {
            micros(observed_at.duration_since(snapshot.phase_since))
        };
        let begin_wait_us = if snapshot.phase == TransactionPhase::BeginWait && !snapshot.finished {
            phase_elapsed_us
        } else {
            snapshot.begin_wait_us
        };
        Self {
            connection_sequence: observation.connection_sequence,
            snapshot,
            outcome,
            elapsed_us,
            phase_elapsed_us,
            coalesced_failures: 0,
            begin_wait_us,
            cause: None,
        }
    }

    fn warn(self, dropped_observations: u64) {
        let snapshot = self.snapshot;
        tracing::warn!(
            connection_sequence = self.connection_sequence,
            transaction_stage = snapshot.stage.label(),
            transaction_phase = snapshot.phase_label(),
            known_immediate = snapshot.known_immediate,
            transaction_finished = snapshot.finished,
            transaction_elapsed_us = self.elapsed_us,
            transaction_phase_elapsed_us = self.phase_elapsed_us,
            transaction_begin_wait_us = self.begin_wait_us,
            outcome = self.outcome,
            coalesced_failures = self.coalesced_failures,
            cause = ?self.cause,
            dropped_observations,
            holder_scope = "observed_pool_only; implicit_and_external_writers_unknown",
            "SQLite transaction slow or failed"
        );
    }
}

/// One monitor per pool and a bounded set of physical-connection observations.
/// Retired connection observations survive until their bounded summary is
/// consumed. Entries contain no pool reference, SQL, bound values or user IDs.
pub(crate) struct DatabaseTelemetry {
    connections: Mutex<Vec<Arc<Mutex<ConnectionTransactionObservation>>>>,
    gc: Mutex<Vec<Arc<Mutex<GcProgress>>>>,
    capacity: usize,
    sequence: AtomicU64,
    started: AtomicBool,
    dropped_observations: AtomicU64,
    memory: memory::Diagnostics,
}

/// Values only: a blocked subscriber cannot retain the pool or state slots.
struct TelemetryReports {
    transactions: Vec<TransactionReport>,
    gc: Option<GcReport>,
    gc_reports: u64,
    dropped: u64,
    memory: Option<memory::Report>,
}

impl TelemetryReports {
    fn is_empty(&self) -> bool {
        self.transactions.is_empty() && self.gc.is_none() && self.memory.is_none()
    }
    fn emit(self) {
        for transaction in self.transactions {
            transaction.warn(self.dropped);
        }
        if let Some(gc) = self.gc {
            gc.warn(self.gc_reports, self.dropped);
        }
        if let Some(memory) = self.memory {
            memory.emit();
        }
    }
}

impl DatabaseTelemetry {
    pub(crate) fn new(connection_capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            connections: Mutex::new(Vec::new()),
            gc: Mutex::new(Vec::new()),
            capacity: connection_capacity.saturating_mul(4),
            sequence: AtomicU64::new(0),
            started: AtomicBool::new(false),
            dropped_observations: AtomicU64::new(0),
            memory: memory::Diagnostics::new(memory::enabled_from_environment()),
        })
    }

    pub(super) fn memory_epoch(&self) -> u64 {
        self.memory.epoch()
    }

    pub(super) fn memory_observation_disposed(
        &self,
        observation: &Mutex<ConnectionTransactionObservation>,
    ) {
        if self.memory.epoch() != 0 {
            let mut observation = observation.lock().unwrap();
            observation.instrumentation_disposed = true;
            observation.memory_pending = true;
            self.memory.observation_disposed();
        }
    }

    pub(crate) fn connection(&self) -> Arc<Mutex<ConnectionTransactionObservation>> {
        let observation = Arc::new(Mutex::new(ConnectionTransactionObservation {
            connection_sequence: self.sequence.fetch_add(1, Relaxed),
            current: None,
            reported_phase: false,
            reported_any: false,
            pending: None,
            pending_failures: 0,
            memory: None,
            memory_pending: false,
            instrumentation_disposed: false,
        }));
        let mut entries = self.connections.lock().unwrap();
        entries.retain(|entry| {
            Arc::strong_count(entry) > 1 || {
                let observation = entry.lock().unwrap();
                observation.pending.is_some() || observation.memory_pending
            }
        });
        if entries.len() == self.capacity
            && let Some(index) = entries
                .iter()
                .position(|entry| Arc::strong_count(entry) == 1)
        {
            entries.remove(index);
            self.dropped_observations.fetch_add(1, Relaxed);
        }
        if entries.len() < self.capacity {
            entries.push(observation.clone());
        } else {
            self.dropped_observations.fetch_add(1, Relaxed);
        }
        observation
    }

    pub(crate) fn ensure_monitor(self: &Arc<Self>) {
        if self.started.load(Acquire) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .started
            .compare_exchange(false, true, Acquire, Relaxed)
            .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(self);
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let reports = {
                    let Some(telemetry) = weak.upgrade() else {
                        break;
                    };
                    telemetry.collect_reports()
                };
                if !reports.is_empty() {
                    let dispatcher = dispatcher.clone();
                    // Await the one owned output job. Slow I/O cannot block
                    // Tokio's current-thread executor or build a log queue.
                    let _ = tokio::task::spawn_blocking(move || {
                        tracing::dispatcher::with_default(&dispatcher, || reports.emit());
                    })
                    .await;
                }
            }
        });
    }

    pub(crate) fn report_transaction_end(
        &self,
        observation: &mut ConnectionTransactionObservation,
        snapshot: TransactionSnapshot,
        cause: Option<&'static str>,
    ) {
        // Instrumentation runs inside SQLite's call stack, possibly holding a
        // writer lock. Never emit synchronously here or wait on a log writer.
        if let Some(cause) = cause {
            observation.pending_failures = observation.pending_failures.saturating_add(1);
            let mut report = TransactionReport::new(observation, snapshot, "failed");
            report.cause = Some(cause);
            observation.pending = Some(report);
        } else if snapshot.elapsed_us() >= 1_000_000 && !observation.reported_any {
            let report = TransactionReport::new(observation, snapshot, "finished_slow");
            observation.pending.get_or_insert(report);
        }
    }

    fn collect_reports(&self) -> TelemetryReports {
        let epoch = self.memory.epoch();
        // The current epoch remains requested while copying rows. Untouched idle
        // connections remain stale/unknown; no checkout or extra job samples them.
        let mut memory = (epoch != 0).then(|| memory::Report::new(epoch, epoch, self.capacity));
        let entries = self.connections.lock().unwrap().clone();
        let mut transactions = Vec::new();
        for entry in entries {
            let mut observation = entry.lock().unwrap();
            if let Some(memory) = &mut memory {
                memory.rows.push(memory::Row {
                    sequence: observation.connection_sequence,
                    disposed: observation.instrumentation_disposed,
                    sample: observation.memory,
                });
                observation.memory_pending = false;
            }
            if let Some(mut report) = observation.pending.take() {
                report.coalesced_failures = std::mem::take(&mut observation.pending_failures);
                transactions.push(report);
            }
            if let Some(snapshot) = observation.current
                && snapshot.phase_elapsed_us() >= 1_000_000
                && !observation.reported_phase
            {
                transactions.push(TransactionReport::new(&observation, snapshot, "in_flight"));
                observation.reported_phase = true;
                observation.reported_any = true;
            }
        }
        self.connections.lock().unwrap().retain(|entry| {
            Arc::strong_count(entry) > 1 || {
                let observation = entry.lock().unwrap();
                observation.pending.is_some() || observation.memory_pending
            }
        });
        let entries = self.gc.lock().unwrap().clone();
        let mut gc_report = None;
        let mut gc_reports = 0_u64;
        for entry in entries {
            let mut progress = entry.lock().unwrap();
            let outcome = if progress.retired && !progress.retirement_reported {
                progress.retirement_reported = true;
                if progress.cause.is_some() {
                    Some("failed")
                } else if progress.retired_at.is_some_and(|ended| {
                    ended.duration_since(progress.started) >= Duration::from_secs(1)
                }) && !progress.reported_any
                {
                    Some("retired_slow")
                } else {
                    None
                }
            } else if !progress.retired
                && progress.phase_since.elapsed() >= Duration::from_secs(1)
                && !progress.reported
            {
                progress.reported = true;
                progress.reported_any = true;
                Some("in_flight")
            } else {
                None
            };
            if let Some(outcome) = outcome {
                gc_reports += 1;
                if outcome == "in_flight"
                    || gc_report
                        .as_ref()
                        .is_none_or(|report: &GcReport| report.outcome != "in_flight")
                {
                    gc_report = Some(GcReport {
                        progress: *progress,
                        observed_at: progress.retired_at.unwrap_or_else(Instant::now),
                        outcome,
                    });
                }
            }
        }
        self.gc.lock().unwrap().retain(|entry| {
            Arc::strong_count(entry) > 1 || !entry.lock().unwrap().retirement_reported
        });
        let dropped = self.dropped_observations.load(Relaxed);
        if let Some(memory) = &mut memory {
            memory.registrations = self.sequence.load(Relaxed);
            memory.disposal_events = self.memory.disposal_events();
            // Existing omissions can include transaction and GC observations.
            // They are not an exact count of missing physical connections.
            memory.omitted_observations = dropped;
            memory.next_epoch = self.memory.advance();
        }
        TelemetryReports {
            transactions,
            gc: gc_report,
            gc_reports,
            dropped,
            memory,
        }
    }

    #[cfg(test)]
    fn report_slow(&self) {
        self.collect_reports().emit();
    }

    pub(crate) fn start_gc(self: &Arc<Self>) -> (GcWorkerObservation, GcObservation) {
        self.ensure_monitor();
        let observation = GcObservation(Arc::new(Mutex::new(GcProgress {
            phase: GcPhase::Claim,
            started: Instant::now(),
            phase_since: Instant::now(),
            selected: 0,
            claimed: 0,
            unlink_started: 0,
            unlink_retired: 0,
            unlink_completed: 0,
            unlink_failed: 0,
            unlink_cancelled: 0,
            finalized: 0,
            cause: None,
            retired: false,
            retired_at: None,
            reported: false,
            reported_any: false,
            retirement_reported: false,
        })));
        let mut entries = self.gc.lock().unwrap();
        entries.retain(|entry| {
            if Arc::strong_count(entry) > 1 {
                return true;
            }
            let progress = entry.lock().unwrap();
            !progress.retirement_reported
                && (progress.cause.is_some()
                    || progress.retired_at.is_some_and(|ended| {
                        ended.duration_since(progress.started) >= Duration::from_secs(1)
                    }))
        });
        if entries.len() == self.capacity
            && let Some(index) = entries
                .iter()
                .position(|entry| Arc::strong_count(entry) == 1)
        {
            entries.remove(index);
            self.dropped_observations.fetch_add(1, Relaxed);
        }
        if entries.len() < self.capacity {
            entries.push(observation.0.clone());
        } else {
            self.dropped_observations.fetch_add(1, Relaxed);
        }
        (
            GcWorkerObservation {
                progress: observation.clone(),
            },
            observation,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GcPhase {
    Claim,
    Unlink,
    Finalize,
}

impl GcPhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Unlink => "unlink",
            Self::Finalize => "finalize",
        }
    }
}

#[derive(Clone, Copy)]
struct GcProgress {
    phase: GcPhase,
    started: Instant,
    phase_since: Instant,
    selected: u64,
    claimed: u64,
    unlink_started: u64,
    unlink_retired: u64,
    unlink_completed: u64,
    unlink_failed: u64,
    unlink_cancelled: u64,
    finalized: u64,
    cause: Option<&'static str>,
    retired: bool,
    retired_at: Option<Instant>,
    reported: bool,
    reported_any: bool,
    retirement_reported: bool,
}

struct GcReport {
    progress: GcProgress,
    observed_at: Instant,
    outcome: &'static str,
}

impl GcReport {
    fn warn(self, coalesced_reports: u64, dropped_observations: u64) {
        let progress = self.progress;
        tracing::warn!(
            gc_phase = progress.phase.label(), gc_elapsed_us = micros(self.observed_at.duration_since(progress.started)),
            gc_phase_elapsed_us = micros(self.observed_at.duration_since(progress.phase_since)),
            selected = progress.selected, claimed = progress.claimed,
            unlink_started = progress.unlink_started, unlink_completed = progress.unlink_completed,
            unlink_failed = progress.unlink_failed, unlink_cancelled = progress.unlink_cancelled,
            unlink_in_flight = progress.unlink_started.saturating_sub(progress.unlink_retired),
            finalized = progress.finalized, worker_retired = progress.retired, cause = ?progress.cause,
            outcome = self.outcome, coalesced_reports, dropped_observations,
            "Storage GC slow or failed"
        );
    }
}

#[derive(Clone)]
pub(crate) struct GcObservation(Arc<Mutex<GcProgress>>);

impl GcObservation {
    pub(crate) fn phase(&self, phase: GcPhase) {
        let mut progress = self.0.lock().unwrap();
        progress.phase = phase;
        progress.phase_since = Instant::now();
        progress.reported = false;
    }
    pub(crate) fn selected(&self, count: usize) {
        self.0.lock().unwrap().selected += count as u64;
    }
    pub(crate) fn claimed(&self, count: usize) {
        let mut progress = self.0.lock().unwrap();
        progress.claimed += count as u64;
        if progress.retired {
            progress.retirement_reported = false;
        }
    }
    pub(crate) fn finalized(&self, count: usize) {
        let mut progress = self.0.lock().unwrap();
        progress.finalized += count as u64;
        if progress.retired {
            progress.retirement_reported = false;
        }
    }
    pub(crate) fn unlink(&self) -> GcUnlinkObservation {
        self.0.lock().unwrap().unlink_started += 1;
        GcUnlinkObservation {
            observation: self.clone(),
            finished: false,
        }
    }
    pub(crate) fn failure(&self, error: &AppError) {
        let cause = match error {
            AppError::Database(_) => database_cause(error),
            AppError::Storage(_) => "storage_failure",
            _ => "internal_failure",
        };
        self.0.lock().unwrap().cause.get_or_insert(cause);
    }
}

pub(crate) struct GcUnlinkObservation {
    observation: GcObservation,
    finished: bool,
}

impl GcUnlinkObservation {
    pub(crate) fn finish(mut self, success: bool) {
        let mut progress = self.observation.0.lock().unwrap();
        progress.unlink_completed += 1;
        progress.unlink_retired += 1;
        if !success {
            progress.unlink_failed += 1;
        }
        self.finished = true;
    }
}

impl Drop for GcUnlinkObservation {
    fn drop(&mut self) {
        if !self.finished {
            let mut progress = self.observation.0.lock().unwrap();
            progress.unlink_retired += 1;
            progress.unlink_cancelled += 1;
        }
    }
}

pub(crate) struct GcWorkerObservation {
    progress: GcObservation,
}

impl Drop for GcWorkerObservation {
    fn drop(&mut self) {
        let mut progress = self.progress.0.lock().unwrap();
        progress.retired = true;
        progress.retired_at = Some(Instant::now());
    }
}

pub(crate) fn database_cause(error: &AppError) -> &'static str {
    let AppError::Database(message) = error else {
        return "internal_failure";
    };
    let message = message
        .strip_prefix("connection acquisition failed: ")
        .unwrap_or(message);
    match message {
        "database admission capacity exceeded" => "admission_exhausted",
        "database checkout timed out" => "checkout_timeout",
        "database operation timed out; cleanup remains admitted" => "operation_timeout",
        "collaboration database backpressure deadline exceeded" => "ws_backpressure_deadline",
        _ if error.is_transient_database() => "sqlite_lock_contention",
        _ => "database_failure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use diesel::prelude::*;
    use horme::GetConnection;
    use std::io::{self, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{Notify, Semaphore, mpsc, oneshot};

    #[derive(Clone, Default)]
    struct LogCapture(Arc<Mutex<Vec<u8>>>);

    impl Write for LogCapture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl LogCapture {
        fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
            let writer = self.clone();
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_max_level(tracing::Level::WARN)
                .with_writer(move || writer.clone())
                .finish()
        }
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    async fn wait_for(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("controlled telemetry state must become observable");
    }

    async fn settled(database: &Database) {
        wait_for(|| {
            let state = database.state();
            state.admitted_operations == 0 && state.connections == state.idle_connections
        })
        .await;
    }

    #[test]
    fn memory_diagnostics_disabled_is_silent_and_overflow_never_claims_full_coverage() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let telemetry = DatabaseTelemetry::new(1);
        telemetry.memory.set_enabled_for_test(false);
        let reports = telemetry.collect_reports();
        assert!(reports.memory.is_none());
        reports.emit();
        assert!(logs.text().is_empty());

        telemetry.memory.set_enabled_for_test(true);
        // Live observation slots can exceed the configured pool size during
        // retirement. Hold them all, so registry omission cannot be concealed.
        let slots: Vec<_> = (0..5).map(|_| telemetry.connection()).collect();
        let report = telemetry.collect_reports().memory.unwrap();
        assert_eq!(report.registrations, 5);
        assert_eq!(report.rows.len(), 4);
        assert_eq!(report.omitted_observations, 1);
        assert!(
            report
                .rows
                .iter()
                .all(|row| report.row_state(row) == "never_sampled")
        );
        report.emit();
        assert!(logs.text().contains("complete_pool_census=false"));
        drop(slots);
        // One report can copy the retiring value rows before reclaiming slots.
        let _ = telemetry.collect_reports();
        assert!(telemetry.connections.lock().unwrap().is_empty());
        assert!(telemetry.collect_reports().memory.unwrap().rows.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn memory_diagnostics_preserve_canceled_transaction_retirement_and_mark_stale_slots() {
        use diesel::connection::TransactionManager;

        let directory = tempfile::tempdir().unwrap();
        let database = Database::with_timeouts(
            directory.path().join("memory.sqlite").to_str().unwrap(),
            1,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        let telemetry = database.telemetry();
        telemetry.memory.set_enabled_for_test(true);
        database
            .run_startup_migrations(|connection| {
                use diesel::connection::SimpleConnection;
                connection
                    .batch_execute("CREATE TABLE memory_retirement(id INTEGER PRIMARY KEY)")?;
                Ok(())
            })
            .await
            .unwrap();
        settled(&database).await;
        let first = telemetry.collect_reports().memory.unwrap();
        assert_eq!(first.rows.len(), 1);
        // The real monitor can advance an epoch between cleanup and this read.
        // Freshness is reported, never manufactured by an extra checkout.
        assert!(first.rows[0].sample.unwrap().values[0].bytes.unwrap() > 0);

        let (entered, started) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let caller = tokio::spawn({
            let database = database.clone();
            async move {
                database.run(move |connection| {
                    <diesel::SqliteConnection as diesel::Connection>::TransactionManager::begin_transaction(connection)?;
                    let _ = entered.send(());
                    released.recv().unwrap();
                    Ok::<_, diesel::result::Error>(())
                }).await
            }
        });
        started.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(database.state().admitted_operations > 0);
        let while_running = telemetry.collect_reports().memory.unwrap();
        assert_eq!(while_running.row_state(&while_running.rows[0]), "stale");
        // A numeric report must not hold a lease or keep the opaque worker alive.
        let _ = release.send(());
        wait_for(|| database.state().connections == 0 && database.state().admitted_operations == 0)
            .await;
        wait_for(|| telemetry.memory.disposal_events() == 1).await;
        // This only counted instrumentation disposal, not sqlite3_close success.
        let retired = telemetry.collect_reports().memory.unwrap();
        assert!(retired.rows.iter().all(|row| row.disposed));
        let inner = Arc::downgrade(&database.0);
        drop(database);
        wait_for(|| inner.upgrade().is_none()).await;
        drop((first, while_running, retired));
    }

    #[test]
    fn memory_sample_ages_include_time_waiting_to_emit() {
        let logs = LogCapture::default();
        let mut report = memory::Report::new(1, 2, 4);
        // A connection can finish sampling after global collection has begun.
        let captured = Instant::now();
        report.rows.push(memory::Row {
            sequence: 0,
            disposed: false,
            sample: Some(memory::Sample {
                epoch: 1,
                began: captured,
                ended: captured,
                values: [memory::Value {
                    rc: 0,
                    bytes: Some(1),
                }; 4],
            }),
        });
        std::thread::sleep(Duration::from_millis(20));
        tracing::subscriber::with_default(logs.subscriber(), || report.emit());
        let text = logs.text();
        for (event, field) in [
            ("SQLite memory diagnostic summary", "global_sample_age_us="),
            ("SQLite connection memory diagnostic", "sample_age_us="),
        ] {
            let line = text.lines().find(|line| line.contains(event)).unwrap();
            let value = line
                .split_once(field)
                .unwrap()
                .1
                .split_whitespace()
                .next()
                .unwrap();
            let age: u64 = value
                .trim_start_matches("Some(")
                .trim_end_matches(')')
                .parse()
                .unwrap();
            assert!(
                age >= 20_000,
                "capture-time ages must not hide queued output"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_immediate_waiter_reports_its_live_writer_before_either_returns() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let directory = tempfile::tempdir().unwrap();
        let database = Database::with_timeouts(
            directory.path().join("observer.sqlite").to_str().unwrap(),
            2,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        database
            .run_startup_migrations(|connection| {
                diesel::sql_query("CREATE TABLE private_query_sentinel(value TEXT)")
                    .execute(connection)?;
                Ok(())
            })
            .await
            .unwrap();
        // Both connections must finish real startup setup before the lock
        // experiment, so cold WAL setup cannot masquerade as BEGIN waiting.
        {
            let mut provider = database.clone();
            let first = provider.get_mut_conn().await.unwrap();
            database.run_startup_migrations(|_| Ok(())).await.unwrap();
            drop(first);
        }
        settled(&database).await;
        let (entered, running) = oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let writer_db = database.clone();
        let writer = tokio::spawn(async move {
            writer_db
                .run(move |connection| {
                    immediate_transaction::<_, AppError>(
                        connection,
                        TransactionStage::DocumentMetadata,
                        |connection| {
                            diesel::sql_query("INSERT INTO private_query_sentinel VALUES (?)")
                                .bind::<diesel::sql_types::Text, _>("private_body_sentinel")
                                .execute(connection)?;
                            entered.send(()).unwrap();
                            blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                            Ok(())
                        },
                    )
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap();
        let waiter_db = database.clone();
        let waiter = tokio::spawn(async move {
            waiter_db
                .run(move |connection| {
                    immediate_transaction::<_, AppError>(
                        connection,
                        TransactionStage::BlobReservation,
                        |_| Ok(()),
                    )
                })
                .await
        });
        let telemetry = database.telemetry();
        wait_for(|| {
            let entries = telemetry.connections.lock().unwrap().clone();
            entries.iter().any(|entry| {
                entry.lock().unwrap().current.is_some_and(|value| {
                    value.stage == TransactionStage::BlobReservation
                        && value.phase == TransactionPhase::BeginWait
                })
            })
        })
        .await;
        // Await the real pool monitor while both real SQLite operations remain
        // blocked. The summary must exist before a return/exception/cleanup.
        wait_for(|| {
            let text = logs.text();
            text.contains(
                "transaction_stage=\"document_metadata\" transaction_phase=\"active_write\"",
            ) && text.contains(
                "transaction_stage=\"blob_reservation\" transaction_phase=\"write_begin_wait\"",
            )
        })
        .await;
        assert!(!writer.is_finished() && !waiter.is_finished());
        let delayed_report = telemetry
            .connections
            .lock()
            .unwrap()
            .iter()
            .find_map(|entry| {
                let observation = entry.lock().unwrap();
                observation
                    .snapshot()
                    .filter(|snapshot| {
                        snapshot.stage == TransactionStage::BlobReservation
                            && snapshot.phase == TransactionPhase::BeginWait
                    })
                    .map(|snapshot| {
                        // An injected observation instant makes all duration checks
                        // deterministic without sleeping or changing real locks.
                        let observed_at = snapshot.began_at + Duration::from_secs(7);
                        let report = TransactionReport::new_at(
                            &observation,
                            snapshot,
                            "in_flight",
                            observed_at,
                        );
                        assert_eq!(report.elapsed_us, 7_000_000);
                        assert_eq!(report.phase_elapsed_us, 7_000_000);
                        assert_eq!(report.begin_wait_us, 7_000_000);
                        let mut active = snapshot;
                        active.phase = TransactionPhase::Active;
                        active.phase_since = snapshot.began_at + Duration::from_secs(1);
                        active.begin_wait_us = 29;
                        let active_report = TransactionReport::new_at(
                            &observation,
                            active,
                            "in_flight",
                            observed_at,
                        );
                        assert_eq!(active_report.elapsed_us, 7_000_000);
                        assert_eq!(active_report.phase_elapsed_us, 6_000_000);
                        assert_eq!(active_report.begin_wait_us, 29);
                        let mut finished = snapshot;
                        finished.finished = true;
                        finished.elapsed_us = 23;
                        finished.phase_elapsed_us = 19;
                        finished.begin_wait_us = 17;
                        let completed = TransactionReport::new_at(
                            &observation,
                            finished,
                            "completed",
                            observed_at,
                        );
                        assert_eq!(completed.elapsed_us, 23);
                        assert_eq!(completed.phase_elapsed_us, 19);
                        assert_eq!(completed.begin_wait_us, 17);
                        report
                    })
            })
            .expect("freeze the actual blocked BEGIN before releasing its writer");
        let frozen_elapsed_us = delayed_report.elapsed_us;
        let frozen_phase_us = delayed_report.phase_elapsed_us;
        assert_eq!(
            frozen_elapsed_us, frozen_phase_us,
            "one observation instant for BEGIN elapsed times"
        );
        release.send(()).unwrap();
        writer.await.unwrap().unwrap();
        waiter.await.unwrap().unwrap();
        settled(&database).await;
        // The BEGIN and its transaction have now completed. Emitting the old
        // queued value must not include the time spent waiting to write logs.
        let delayed_logs = LogCapture::default();
        tracing::subscriber::with_default(delayed_logs.subscriber(), || delayed_report.warn(0));
        let delayed = delayed_logs.text();
        assert!(delayed.contains(&format!("transaction_elapsed_us={frozen_elapsed_us}")));
        assert!(delayed.contains(&format!("transaction_phase_elapsed_us={frozen_phase_us}")));
        assert!(delayed.contains(&format!("transaction_begin_wait_us={frozen_phase_us}")));
        let text = logs.text();
        assert!(
            !text.contains("private_query_sentinel") && !text.contains("private_body_sentinel")
        );
        assert!(text.contains("implicit_and_external_writers_unknown"));
        drop(database);
        directory.close().unwrap();
    }

    #[derive(Clone)]
    struct BlockingWriter {
        logs: LogCapture,
        entered: Arc<Notify>,
        release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
        blocked: Arc<AtomicBool>,
        returned: Arc<AtomicBool>,
    }
    impl Write for BlockingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.logs.write(bytes)?;
            if !self.blocked.swap(true, Relaxed) {
                self.entered.notify_one();
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
                self.returned.store(true, Relaxed);
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_blocked_log_writer_cannot_hold_the_sql_observation_mutex_or_expose_error_text() {
        let logs = LogCapture::default();
        let entered = Arc::new(Notify::new());
        let returned = Arc::new(AtomicBool::new(false));
        let (release, blocked) = std::sync::mpsc::channel();
        let writer = BlockingWriter {
            logs: logs.clone(),
            entered: entered.clone(),
            release: Arc::new(Mutex::new(blocked)),
            blocked: Arc::new(AtomicBool::new(false)),
            returned: returned.clone(),
        };
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let database = Database::with_timeouts(
            ":memory:",
            1,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        let telemetry = database.telemetry();
        database.run_startup_migrations(|connection| {
            use diesel::connection::SimpleConnection;
            connection.batch_execute("CREATE TABLE private_values(value TEXT); CREATE TABLE committed(value INTEGER); CREATE TRIGGER reject_private BEFORE INSERT ON private_values BEGIN SELECT RAISE(ABORT,'private_error_token_sentinel'); END;")?;
            Ok(())
        }).await.unwrap();
        database
            .run(|connection| {
                for _ in 0..2 {
                    let error = immediate_transaction::<_, AppError>(
                        connection,
                        TransactionStage::DocumentMetadata,
                        |connection| {
                            diesel::sql_query("INSERT INTO private_values VALUES (?)")
                                .bind::<diesel::sql_types::Text, _>("private_bound_body_sentinel")
                                .execute(connection)?;
                            Ok(())
                        },
                    )
                    .unwrap_err();
                    assert!(error.to_string().contains("private_error_token_sentinel"));
                }
                Ok::<_, AppError>(())
            })
            .await
            .unwrap();
        settled(&database).await;
        // The real ensure_monitor path must emit on its bounded blocking job,
        // not on this current-thread executor or a manually driven test thread.
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        assert!(
            !returned.load(Relaxed),
            "the log sink must still be blocked"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            tokio::time::sleep(Duration::from_millis(10)),
        )
        .await
        .unwrap();
        // This real writer must BEGIN, execute and COMMIT while the monitor's
        // output remains blocked. Releasing log I/O is not a prerequisite.
        tokio::time::timeout(
            Duration::from_secs(3),
            database.run(|connection| {
                immediate_transaction::<_, AppError>(
                    connection,
                    TransactionStage::BlobReservation,
                    |connection| {
                        diesel::sql_query("INSERT INTO committed VALUES (1)")
                            .execute(connection)?;
                        Ok(())
                    },
                )
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            !returned.load(Relaxed),
            "SQL and Tokio timers progressed before log release"
        );
        release.send(()).unwrap();
        wait_for(|| returned.load(Relaxed)).await;
        telemetry.report_slow();
        wait_for(|| {
            logs.text()
                .split("coalesced_failures=")
                .skip(1)
                .map(|part| {
                    part.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                        .parse::<u64>()
                        .unwrap()
                })
                .sum::<u64>()
                == 2
        })
        .await;
        let text = logs.text();
        assert!(text.matches("SQLite transaction slow or failed").count() <= 2);
        for sentinel in [
            "private_values",
            "private_error_token_sentinel",
            "private_bound_body_sentinel",
        ] {
            assert!(!text.contains(sentinel));
        }
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gc_reports_claimed_and_blocked_unlinks_then_exact_failure_and_finalize_counts() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let database = Database::with_timeouts(
            ":memory:",
            1,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        database
            .run_startup_migrations(|connection| {
                crate::migrations::run_directory(
                    connection,
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
                )?;
                Ok(())
            })
            .await
            .unwrap();
        database
            .run(|connection| {
                for index in 0..16 {
                    let path = format!("private_object_path_sentinel-{index}");
                    diesel::sql_query(
                        "INSERT INTO blob_inventory(path,state,first_seen) VALUES (?,'ready',0)",
                    )
                    .bind::<diesel::sql_types::Text, _>(&path)
                    .execute(connection)?;
                    diesel::sql_query(
                        "INSERT INTO storage_gc(path,created_at) VALUES (?,'2000-01-01T00:00:00Z')",
                    )
                    .bind::<diesel::sql_types::Text, _>(path)
                    .execute(connection)?;
                }
                Ok::<_, AppError>(())
            })
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let release = Arc::new(Semaphore::new(0));
        let gate = release.clone();
        let (arrived, mut arrivals) = mpsc::channel(16);
        let server = tokio::spawn(async move {
            let mut responses = tokio::task::JoinSet::new();
            for index in 0..16 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let gate = gate.clone();
                let arrived = arrived.clone();
                responses.spawn(async move {
                    let mut request = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        assert!(request.len() < 16384);
                        socket.read_exact(&mut byte).await.unwrap();
                        request.push(byte[0]);
                    }
                    assert!(request.starts_with(b"DELETE "));
                    arrived.send(()).await.unwrap();
                    gate.acquire().await.unwrap().forget();
                    let response: &[u8] = if index == 0 {
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    } else {
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    socket.write_all(response).await.unwrap();
                });
            }
            while let Some(result) = responses.join_next().await {
                result.unwrap();
            }
        });
        let storage =
            crate::services::StorageBackend::S3(crate::services::s3_storage::S3Storage::new(
                "private_bucket_sentinel",
                "test",
                "private_access_sentinel",
                "private_secret_sentinel",
                &endpoint,
            ));
        let gc_db = database.clone();
        let worker = tokio::spawn(async move {
            crate::services::persistence::drain_storage_gc(&gc_db, &storage).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..8 {
                arrivals.recv().await.unwrap();
            }
        })
        .await
        .unwrap();
        let telemetry = database.telemetry();
        let observation = telemetry.gc.lock().unwrap().last().unwrap().clone();
        {
            let progress = observation.lock().unwrap();
            assert_eq!(progress.phase, GcPhase::Unlink);
            assert_eq!((progress.selected, progress.claimed), (16, 16));
            assert_eq!(
                (
                    progress.unlink_started,
                    progress.unlink_completed,
                    progress.unlink_retired
                ),
                (8, 0, 0)
            );
            assert_eq!(progress.finalized, 0);
        }
        wait_for(|| logs.text().contains("Storage GC slow or failed")).await;
        assert!(logs.text().contains("gc_phase=\"unlink\""));
        assert!(logs.text().contains("unlink_in_flight=8"));
        let before = logs.text().matches("Storage GC slow or failed").count();
        telemetry.report_slow();
        telemetry.report_slow();
        assert_eq!(
            logs.text().matches("Storage GC slow or failed").count(),
            before
        );
        release.add_permits(16);
        assert!(matches!(worker.await.unwrap(), Err(AppError::Storage(_))));
        server.await.unwrap();
        {
            let progress = observation.lock().unwrap();
            assert!(progress.retired);
            assert_eq!(
                (
                    progress.claimed,
                    progress.unlink_started,
                    progress.unlink_completed
                ),
                (16, 16, 16)
            );
            assert_eq!(
                (
                    progress.unlink_failed,
                    progress.unlink_cancelled,
                    progress.finalized
                ),
                (1, 0, 15)
            );
            assert_eq!(progress.unlink_started - progress.unlink_retired, 0);
        }
        telemetry.report_slow();
        let text = logs.text();
        assert!(text.contains("cause=Some(\"storage_failure\")"));
        assert!(text.contains("finalized=15"));
        for sentinel in [
            "private_object_path_sentinel",
            "private_bucket_sentinel",
            "private_access_sentinel",
            "private_secret_sentinel",
        ] {
            assert!(!text.contains(sentinel));
        }
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fast_gc_retirement_does_not_accumulate_observations_or_logs() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let telemetry = DatabaseTelemetry::new(1);
        for _ in 0..1000 {
            let (_owner, _progress) = telemetry.start_gc();
        }
        assert_eq!(telemetry.gc.lock().unwrap().len(), 1);
        assert_eq!(telemetry.dropped_observations.load(Relaxed), 0);
        telemetry.report_slow();
        assert!(telemetry.gc.lock().unwrap().is_empty());
        assert!(logs.text().is_empty());
    }
}
