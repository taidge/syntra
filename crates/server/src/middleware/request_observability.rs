//! Correlate overload responses without logging credentials or user input.

use std::future::Future;
use std::sync::atomic::{
    AtomicU8, AtomicU64,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use contrix_core::error::AppError;
use salvo::prelude::*;

#[derive(Clone, Copy)]
struct UnavailableReason {
    stage: &'static str,
    cause: &'static str,
}

#[derive(Default)]
pub(crate) struct DatabaseObservation {
    pub(crate) checkout_attempts: AtomicU64,
    pub(crate) checkouts_acquired: AtomicU64,
    pub(crate) checkout_wait_us: AtomicU64,
    pub(crate) statements_started: AtomicU64,
    pub(crate) statements_completed: AtomicU64,
    pub(crate) sql_run_us: AtomicU64,
    last_database_error: Mutex<Option<&'static str>>,
    unavailable_reason: Mutex<Option<UnavailableReason>>,
    operation_timeout: Mutex<Option<OperationTimeout>>,
}

struct DatabaseObservationContext {
    observation: Arc<DatabaseObservation>,
    operation: Option<Arc<DatabaseOperationProgress>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum WorkerPhase {
    AsyncQueue,
    WaitingPredecessor,
    BlockingQueue,
    Running,
    Returning,
}

impl WorkerPhase {
    fn label(self) -> &'static str {
        match self {
            Self::AsyncQueue => "async_queue",
            Self::WaitingPredecessor => "waiting_predecessor",
            Self::BlockingQueue => "blocking_queue",
            Self::Running => "running",
            Self::Returning => "returning",
        }
    }
}

pub(crate) struct DatabaseOperationProgress {
    stage: &'static str,
    phase: AtomicU8,
    pub(crate) statements_started: AtomicU64,
    pub(crate) statements_completed: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
struct OperationTimeout {
    stage: &'static str,
    phase: WorkerPhase,
    statements_started: u64,
    statements_completed: u64,
}

struct DatabaseSnapshot {
    checkout_attempts: u64,
    checkouts_acquired: u64,
    checkout_wait_us: u64,
    statements_started: u64,
    statements_completed: u64,
    sql_run_us: u64,
    last_observed_db_pressure: Option<&'static str>,
    unavailable_reason: Option<UnavailableReason>,
    operation: Option<OperationTimeout>,
}

impl DatabaseOperationProgress {
    pub(crate) fn set_phase(&self, phase: WorkerPhase) {
        self.phase.store(phase as u8, Release);
    }

    fn snapshot(&self) -> OperationTimeout {
        let phase = match self.phase.load(Acquire) {
            phase if phase == WorkerPhase::AsyncQueue as u8 => WorkerPhase::AsyncQueue,
            phase if phase == WorkerPhase::WaitingPredecessor as u8 => {
                WorkerPhase::WaitingPredecessor
            }
            phase if phase == WorkerPhase::BlockingQueue as u8 => WorkerPhase::BlockingQueue,
            phase if phase == WorkerPhase::Running as u8 => WorkerPhase::Running,
            _ => WorkerPhase::Returning,
        };
        // Read completed first so a concurrently starting query cannot make
        // the reported completed count exceed the started count.
        let statements_completed = self.statements_completed.load(Acquire);
        OperationTimeout {
            stage: self.stage,
            phase,
            statements_started: self.statements_started.load(Relaxed),
            statements_completed,
        }
    }
}

tokio::task_local! {
    static DATABASE_OBSERVATION: DatabaseObservationContext;
    static DATABASE_STAGE: &'static str;
    static DATABASE_OPERATION: Arc<DatabaseOperationProgress>;
}

pub(crate) fn current_database_observation() -> Option<Arc<DatabaseObservation>> {
    DATABASE_OBSERVATION
        .try_with(|context| context.observation.clone())
        .ok()
}

pub(crate) fn observe_database_operation<T>(work: impl FnOnce() -> T) -> T {
    let Ok(stage) = DATABASE_STAGE.try_with(|&stage| stage) else {
        return work();
    };
    // Only the labeled call's worker receives this state. A lease's later
    // cleanup barrier is independent and cannot overwrite the deadline phase.
    DATABASE_OPERATION.sync_scope(
        Arc::new(DatabaseOperationProgress {
            stage,
            phase: AtomicU8::new(WorkerPhase::AsyncQueue as u8),
            statements_started: AtomicU64::new(0),
            statements_completed: AtomicU64::new(0),
        }),
        work,
    )
}

pub(crate) fn current_database_operation() -> Option<Arc<DatabaseOperationProgress>> {
    DATABASE_OPERATION.try_with(Arc::clone).ok()
}

pub(crate) fn observe_statement(
    record: impl FnOnce(&DatabaseObservation, Option<&DatabaseOperationProgress>),
) {
    let _ = DATABASE_OBSERVATION.try_with(|context| {
        record(&context.observation, context.operation.as_deref());
    });
}

/// Propagate only observation to the SQLite worker. No change to task ownership
/// or cancellation semantics is needed.
pub(crate) fn with_database_observation<T>(
    observation: Option<Arc<DatabaseObservation>>,
    operation: Option<Arc<DatabaseOperationProgress>>,
    work: impl FnOnce() -> T,
) -> T {
    match observation {
        Some(observation) => DATABASE_OBSERVATION.sync_scope(
            DatabaseObservationContext {
                observation,
                operation,
            },
            work,
        ),
        None => work(),
    }
}

/// Capture from the calling task, before the returned future is spawned.
pub(crate) fn with_request_database_observation<F: Future>(
    work: F,
) -> impl Future<Output = F::Output> {
    let observation = current_database_observation();
    async move {
        match observation {
            Some(observation) => scope_database_observation(observation, work).await,
            None => work.await,
        }
    }
}

async fn scope_database_observation<F: Future>(
    observation: Arc<DatabaseObservation>,
    work: F,
) -> F::Output {
    DATABASE_OBSERVATION
        .scope(
            DatabaseObservationContext {
                observation,
                operation: None,
            },
            work,
        )
        .await
}

/// A WebSocket upgrade's request scope does not describe subsequent messages.
/// Each operation owns fresh counters, including its final authorization check.
pub(crate) async fn observe_ws_operation<T>(
    document_id: uuid::Uuid,
    op_client_id: uuid::Uuid,
    op_counter: u64,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    let db = Arc::new(DatabaseObservation::default());
    let started = Instant::now();
    let result =
        scope_database_observation(db.clone(), with_database_stage("ws_operation", work)).await;
    if let Err(error) = &result
        && error.status_code() >= 500
    {
        let DatabaseSnapshot {
            checkout_attempts,
            checkouts_acquired,
            checkout_wait_us,
            statements_started,
            statements_completed,
            sql_run_us,
            last_observed_db_pressure,
            unavailable_reason,
            operation,
        } = db.snapshot();
        let fallback_cause = match error {
            AppError::Database(_) => database_cause(error),
            AppError::Storage(_) => "storage_failure",
            AppError::Internal(_) => "internal_failure",
            _ => "server_failure",
        };
        let reason = unavailable_reason.unwrap_or(UnavailableReason {
            stage: "ws_operation",
            cause: fallback_cause,
        });
        let operation_stage = operation.map(|operation| operation.stage);
        let operation_phase = operation.map(|operation| operation.phase.label());
        let operation_statements_started = operation.map(|operation| operation.statements_started);
        let operation_statements_completed =
            operation.map(|operation| operation.statements_completed);
        let elapsed_us = started.elapsed().as_micros() as u64;
        tracing::warn!(
            %document_id, %op_client_id, op_counter,
            stage = reason.stage, cause = reason.cause, ?last_observed_db_pressure,
            ?operation_stage, ?operation_phase, ?operation_statements_started,
            ?operation_statements_completed, elapsed_us, checkout_attempts,
            checkouts_acquired, checkout_wait_us, statements_started,
            statements_completed, sql_run_us,
            "WebSocket operation failed"
        );
    }
    result
}

/// Mark a stage only when its error is propagated. Nested stages can retain a
/// more specific reason as their error is rendered by an outer handler.
pub(crate) async fn with_database_stage<T>(
    stage: &'static str,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    if DATABASE_OBSERVATION.try_with(|_| ()).is_err() {
        return work.await;
    }
    let result = DATABASE_STAGE.scope(stage, work).await;
    if let Err(error) = &result {
        record_database_response(stage, error);
    }
    result
}

impl DatabaseObservation {
    fn snapshot(&self) -> DatabaseSnapshot {
        // Read completion before started, matching timeout progress snapshots
        // when a late worker is still finishing.
        let statements_completed = self.statements_completed.load(Relaxed);
        DatabaseSnapshot {
            checkout_attempts: self.checkout_attempts.load(Relaxed),
            checkouts_acquired: self.checkouts_acquired.load(Relaxed),
            checkout_wait_us: self.checkout_wait_us.load(Relaxed),
            statements_started: self.statements_started.load(Relaxed),
            statements_completed,
            sql_run_us: self.sql_run_us.load(Relaxed),
            last_observed_db_pressure: *self.last_database_error.lock().unwrap(),
            unavailable_reason: *self.unavailable_reason.lock().unwrap(),
            operation: *self.operation_timeout.lock().unwrap(),
        }
    }

    pub(crate) fn record_pressure(&self, error: &AppError) {
        if error.is_transient_database() {
            *self.last_database_error.lock().unwrap() = Some(database_cause(error));
        }
    }

    pub(crate) fn record_operation_timeout(&self, operation: &DatabaseOperationProgress) {
        *self.operation_timeout.lock().unwrap() = Some(operation.snapshot());
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

pub(crate) fn record_failure(stage: &'static str, cause: &'static str) {
    let _ = DATABASE_OBSERVATION.try_with(|context| {
        *context.observation.unavailable_reason.lock().unwrap() =
            Some(UnavailableReason { stage, cause });
    });
}

pub(crate) fn record_database_response(stage: &'static str, error: &AppError) {
    if error.is_transient_database() {
        let _ = DATABASE_OBSERVATION.try_with(|context| {
            context
                .observation
                .unavailable_reason
                .lock()
                .unwrap()
                .get_or_insert_with(|| UnavailableReason {
                    stage,
                    cause: database_cause(error),
                });
        });
    }
}

/// Install before IP rules and authentication so early rejections get the
/// same generated identifier and timing information as handler responses.
#[handler]
pub async fn observe_http_request(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let request_id = uuid::Uuid::new_v4().to_string();
    let started = Instant::now();
    let db = Arc::new(DatabaseObservation::default());
    DATABASE_OBSERVATION
        .scope(
            DatabaseObservationContext {
                observation: db.clone(),
                operation: None,
            },
            ctrl.call_next(req, depot, res),
        )
        .await;

    res.headers_mut().insert(
        "x-request-id",
        request_id.parse().expect("UUID is a valid header value"),
    );
    let elapsed_us = started.elapsed().as_micros() as u64;
    let DatabaseSnapshot {
        checkout_attempts,
        checkouts_acquired,
        checkout_wait_us,
        statements_started,
        statements_completed,
        sql_run_us,
        last_observed_db_pressure,
        unavailable_reason,
        operation,
    } = db.snapshot();
    let status = res.status_code.unwrap_or(StatusCode::OK);
    let reason = if matches!(
        status,
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
    ) {
        unavailable_reason
    } else {
        None
    };
    if status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::TOO_MANY_REQUESTS && reason.is_some()
    {
        let reason = reason.unwrap_or(UnavailableReason {
            stage: "response",
            cause: "unspecified",
        });
        let operation_stage = operation.map(|operation| operation.stage);
        let operation_phase = operation.map(|operation| operation.phase.label());
        let operation_statements_started = operation.map(|operation| operation.statements_started);
        let operation_statements_completed =
            operation.map(|operation| operation.statements_completed);
        tracing::warn!(
            %request_id, stage = reason.stage, cause = reason.cause, ?last_observed_db_pressure,
            ?operation_stage, ?operation_phase, ?operation_statements_started,
            ?operation_statements_completed,
            elapsed_us, checkout_attempts, checkouts_acquired, checkout_wait_us,
            statements_started, statements_completed, sql_run_us,
            "HTTP {} response", status.as_u16()
        );
    } else {
        tracing::trace!(
            %request_id, elapsed_us, checkout_attempts, checkouts_acquired, checkout_wait_us,
            statements_started, statements_completed, sql_run_us, "HTTP request completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, SqliteRuntime};
    use diesel::RunQueryDsl;
    use diesel_async::sync_connection_wrapper::SpawnBlocking;
    use std::io::{self, Write};
    use std::time::Duration;
    use tokio::sync::oneshot;

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

    async fn settled(database: &Database) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = database.state();
                if status.admitted_operations == 0 && status.connections == status.idle_connections
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned database cleanup must retire");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn websocket_scopes_only_warn_for_server_failures_and_do_not_log_error_text() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let document = uuid::Uuid::new_v4();
        let client = uuid::Uuid::new_v4();
        observe_ws_operation(document, client, 1, async { Ok::<_, AppError>(()) })
            .await
            .unwrap();
        for error in [
            AppError::Conflict("private-document-sentinel".into()),
            AppError::Auth("private-token-sentinel".into()),
            AppError::RateLimit("private-budget-sentinel".into()),
        ] {
            observe_ws_operation(document, client, 2, async { Err::<(), _>(error) })
                .await
                .unwrap_err();
        }
        assert!(logs.text().is_empty());
        observe_ws_operation(document, client, 3, async {
            Err::<(), _>(AppError::Storage(
                "private-token-sentinel/private-document-sentinel".into(),
            ))
        })
        .await
        .unwrap_err();
        let text = logs.text();
        assert_eq!(text.matches("WebSocket operation failed").count(), 1);
        assert!(text.contains("cause=\"storage_failure\""));
        assert!(text.contains("operation_phase=None"));
        assert!(!text.contains("private-token-sentinel"));
        assert!(!text.contains("private-document-sentinel"));
        assert!(!text.contains("private-budget-sentinel"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn websocket_operation_inherits_spawned_sql_stage_and_isolates_late_cleanup() {
        let logs = LogCapture::default();
        let _subscriber = tracing::subscriber::set_default(logs.subscriber());
        let document = uuid::Uuid::new_v4();
        let client = uuid::Uuid::new_v4();
        for (counter, stage) in ["blob_write_finish", "document_metadata_commit"]
            .into_iter()
            .enumerate()
        {
            let database = Database::with_timeouts(
                ":memory:",
                1,
                Duration::from_secs(3),
                Duration::from_millis(500),
                Duration::from_millis(30),
            )
            .unwrap();
            database.run_startup_migrations(|_| Ok(())).await.unwrap();
            settled(&database).await;
            let worker_db = database.clone();
            let (release, blocked) = std::sync::mpsc::channel();
            let (entered, started) = oneshot::channel();
            let (captured, observation) = oneshot::channel();
            let work = observe_ws_operation(document, client, counter as u64, async move {
                let parent = current_database_observation().unwrap();
                assert!(captured.send(parent.clone()).is_ok());
                let spawned = with_request_database_observation(async move {
                    assert!(Arc::ptr_eq(
                        &parent,
                        &current_database_observation().unwrap()
                    ));
                    with_database_stage(
                        stage,
                        worker_db.run(move |connection| {
                            diesel::sql_query("CREATE TABLE probe(value INTEGER)")
                                .execute(connection)?;
                            entered.send(()).unwrap();
                            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                            Ok::<_, AppError>(())
                        }),
                    )
                    .await
                });
                tokio::spawn(spawned).await.unwrap()
            });
            let task = tokio::spawn(work);
            let observation = observation.await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), started)
                .await
                .unwrap()
                .unwrap();
            let error = task.await.unwrap().unwrap_err();
            assert_eq!(database_cause(&error), "operation_timeout");
            let snapshot = observation.snapshot();
            let operation = snapshot.operation.unwrap();
            assert_eq!(operation.stage, stage);
            assert_eq!(operation.phase, WorkerPhase::Running);
            assert_eq!(operation.statements_started, 1);
            assert_eq!(operation.statements_completed, 1);
            assert_eq!(snapshot.unavailable_reason.unwrap().stage, stage);
            assert_eq!(snapshot.checkout_attempts, 1);
            assert_eq!(snapshot.checkouts_acquired, 1);
            assert_eq!(snapshot.statements_started, 1);
            assert_eq!(snapshot.statements_completed, 1);

            // A new operation starts while the old worker still owns cleanup.
            observe_ws_operation(document, client, 100 + counter as u64, async {
                let fresh = current_database_observation().unwrap();
                assert!(!Arc::ptr_eq(&fresh, &observation));
                assert_eq!(fresh.snapshot().checkout_attempts, 0);
                assert!(fresh.snapshot().unavailable_reason.is_none());
                assert!(fresh.snapshot().operation.is_none());
                release.send(()).unwrap();
                settled(&database).await;
                assert_eq!(fresh.snapshot().statements_started, 0);
                assert!(fresh.snapshot().operation.is_none());
                Ok::<_, AppError>(())
            })
            .await
            .unwrap();
            let late = observation.snapshot().operation.unwrap();
            assert_eq!(late.stage, stage);
            assert_eq!(late.phase, WorkerPhase::Running);
        }
        let text = logs.text();
        assert_eq!(text.matches("WebSocket operation failed").count(), 2);
        assert!(text.contains("stage=\"blob_write_finish\""));
        assert!(text.contains("stage=\"document_metadata_commit\""));
        assert!(text.contains("operation_phase=Some(\"running\")"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_deadline_waiting_for_the_predecessor_reports_no_running_sql() {
        let mut runtime = SqliteRuntime::new(Duration::from_millis(250));
        let (release, blocked) = std::sync::mpsc::channel();
        let (entered, started) = oneshot::channel();
        let first = runtime.spawn_blocking(move || {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        tokio::time::timeout(Duration::from_secs(3), started)
            .await
            .unwrap()
            .unwrap();

        let (executed, mut execution) = oneshot::channel();
        let observation = Arc::new(DatabaseObservation::default());
        DATABASE_OBSERVATION
            .scope(
                DatabaseObservationContext {
                    observation: observation.clone(),
                    operation: None,
                },
                async {
                    let error = with_database_stage("document_preflight", async {
                        let queued = observe_database_operation(|| {
                            runtime.spawn_blocking(move || executed.send(()).unwrap())
                        });
                        queued
                            .await
                            .map_err(|error| AppError::Database(error.to_string()))
                    })
                    .await
                    .unwrap_err();
                    assert!(error.is_transient_database());
                    record_database_response("documents", &error);
                },
            )
            .await;
        let detail = observation.operation_timeout.lock().unwrap().unwrap();
        assert_eq!(detail.stage, "document_preflight");
        assert_eq!(detail.phase, WorkerPhase::WaitingPredecessor);
        assert_eq!(detail.statements_started, 0);
        assert_eq!(detail.statements_completed, 0);
        let reason = observation.unavailable_reason.lock().unwrap().unwrap();
        assert_eq!(reason.stage, "document_preflight");
        assert_eq!(reason.cause, "operation_timeout");
        assert_eq!(
            execution.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );

        release.send(()).unwrap();
        first.await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), execution)
            .await
            .unwrap()
            .unwrap();
    }
}
