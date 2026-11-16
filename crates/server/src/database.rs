//! Short query leases with per-connection SQLite workers and bounded admission.
//!
//! Cloning/getting a Database never checks out a connection. Only a SQL future
//! or an explicit transaction acquires one. No raw pool escapes this module.
use std::error::Error as StdError;
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::Context;
use contrix_core::{config::DatabaseConfig, error::AppError};
use deadpool::managed::{Object, Pool};
use diesel::connection::{
    Instrumentation, InstrumentationEvent, TransactionManager, TransactionManagerStatus,
};
use diesel::result::{Error, QueryResult};
use diesel::{Connection, SqliteConnection};
use diesel_async::pooled_connection::{
    PoolError as ManagerError, PoolableConnection, RecyclingMethod,
};
use diesel_async::sync_connection_wrapper::{SpawnBlocking, SyncConnectionWrapper};
use futures_util::future::BoxFuture;
use horme::GetConnection;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

mod memory;
pub(crate) mod telemetry;
mod worker;

use worker::WorkerHandle;

use crate::middleware::request_observability::{
    WorkerPhase, current_database_observation, current_database_operation,
    observe_database_operation, observe_statement, record_sqlite_transaction,
    with_database_observation,
};
use telemetry::{
    ConnectionTransactionObservation, DatabaseTelemetry, GcObservation, GcWorkerObservation,
    TransactionPhase,
};

pub type DbConnection = SyncConnectionWrapper<SqliteConnection, SqliteRuntime>;

type BoxError = Box<dyn StdError + Send + Sync>;

fn failure(message: impl Into<String>) -> Error {
    Error::QueryBuilderError(Box::new(std::io::Error::other(message.into())))
}

pub fn query_error(error: horme::Error) -> Error {
    match error {
        horme::Error::Query(error) => error,
        horme::Error::Acquire(error) => Error::QueryBuilderError(error),
    }
}

/// Unlike the adapter's default runtime, cancellation never block_in_place's
/// the HTTP task. Work is serialized in declaration order, including cleanup.
/// EXPERIMENT: establish/query/rollback use one worker. Physical connection
/// Drop keeps the prior caller-thread behavior; no full Drop affinity is claimed.
/// Cancellation/admission anchors remain in the server's ManagedSqlite pool.
pub struct SqliteRuntime {
    tail: Option<oneshot::Receiver<()>>,
    operation_timeout: Duration,
    worker: Result<WorkerHandle, Arc<str>>,
}
impl SqliteRuntime {
    pub(crate) fn new(operation_timeout: Duration) -> Self {
        Self {
            tail: None,
            operation_timeout,
            worker: WorkerHandle::start().map_err(|error| Arc::from(error.to_string())),
        }
    }

    fn with_worker(operation_timeout: Duration, worker: WorkerHandle) -> Self {
        Self {
            tail: None,
            operation_timeout,
            worker: Ok(worker),
        }
    }
}

struct SqliteObservation {
    query_started: Option<Instant>,
    transaction: Arc<Mutex<ConnectionTransactionObservation>>,
    transaction_control: Option<TransactionPhase>,
    telemetry: Arc<DatabaseTelemetry>,
}

impl Instrumentation for SqliteObservation {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        match event {
            InstrumentationEvent::BeginTransaction { depth, .. } if depth.get() == 1 => {
                self.transaction.lock().unwrap().begin();
                self.transaction_control = Some(TransactionPhase::BeginWait);
                self.publish_transaction();
            }
            InstrumentationEvent::CommitTransaction { depth, .. } if depth.get() == 1 => {
                self.transaction
                    .lock()
                    .unwrap()
                    .change_phase(TransactionPhase::CommitWait);
                self.transaction_control = Some(TransactionPhase::CommitWait);
                self.publish_transaction();
            }
            InstrumentationEvent::RollbackTransaction { depth, .. } if depth.get() == 1 => {
                self.transaction
                    .lock()
                    .unwrap()
                    .change_phase(TransactionPhase::RollbackWait);
                self.transaction_control = Some(TransactionPhase::RollbackWait);
                self.publish_transaction();
            }
            InstrumentationEvent::StartQuery { .. } => {
                self.query_started = Some(Instant::now());
                observe_statement(|observation, operation| {
                    observation
                        .statements_started
                        .fetch_add(1, Ordering::Relaxed);
                    if let Some(operation) = operation {
                        operation.statements_started.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
            InstrumentationEvent::FinishQuery { error, .. } => {
                let elapsed = self.query_started.take().map(|started| started.elapsed());
                let app_error = error.map(|error| AppError::Database(error.to_string()));
                let error_cause = app_error.as_ref().map(telemetry::database_cause);
                observe_statement(|observation, operation| {
                    if let Some(elapsed) = elapsed {
                        observation
                            .sql_run_us
                            .fetch_add(elapsed.as_micros() as u64, Ordering::Relaxed);
                        observation
                            .statements_completed
                            .fetch_add(1, Ordering::Relaxed);
                        if let Some(operation) = operation {
                            operation
                                .statements_completed
                                .fetch_add(1, Ordering::Release);
                        }
                    }
                    if let Some(error) = &app_error {
                        observation.record_pressure(error);
                    }
                });
                let control = self.transaction_control.take();
                if control.is_none() && error.is_none() {
                    return;
                }
                let mut transaction = self.transaction.lock().unwrap();
                let mut completed_snapshot = None;
                if control == Some(TransactionPhase::BeginWait) && error.is_none() {
                    transaction.change_phase(TransactionPhase::Active);
                } else if control == Some(TransactionPhase::BeginWait)
                    || matches!(
                        control,
                        Some(TransactionPhase::CommitWait | TransactionPhase::RollbackWait)
                    ) && error.is_none()
                {
                    if let Some(snapshot) = transaction.finish() {
                        completed_snapshot = Some(snapshot);
                        self.telemetry.report_transaction_end(
                            &mut transaction,
                            snapshot,
                            error_cause,
                        );
                    }
                } else if error.is_some()
                    && let Some(snapshot) = transaction.snapshot()
                {
                    completed_snapshot = Some(snapshot);
                    self.telemetry
                        .report_transaction_end(&mut transaction, snapshot, error_cause);
                }
                let snapshot = completed_snapshot.or_else(|| transaction.snapshot());
                drop(transaction);
                if let Some(snapshot) = snapshot {
                    record_sqlite_transaction(snapshot);
                }
            }
            _ => {}
        }
    }
}

impl SqliteObservation {
    fn publish_transaction(&self) {
        let snapshot = self.transaction.lock().unwrap().snapshot();
        if let Some(snapshot) = snapshot {
            record_sqlite_transaction(snapshot);
        }
    }
}

fn sample_requested_sqlite_memory(connection: &mut SqliteConnection) {
    let Some(observation) = connection
        .instrumentation()
        .downcast_ref::<SqliteObservation>()
    else {
        return;
    };
    let epoch = observation.telemetry.memory_epoch();
    if epoch == 0 {
        return;
    }
    // This is only the existing numeric observation, never an opaque connection,
    // Inner, Admission or WorkerControl owner. Release its lock before FFI.
    let slot = observation.transaction.clone();
    if slot
        .lock()
        .unwrap()
        .memory
        .is_some_and(|sample| sample.epoch == epoch)
    {
        return;
    }
    let sample = memory::sample(connection, epoch);
    let mut observation = slot.lock().unwrap();
    observation.memory = Some(sample);
    observation.memory_pending = true;
}

impl Drop for SqliteObservation {
    fn drop(&mut self) {
        // Closing a connection ends its transaction; a monitor must not retain
        // a stale active-writer description after the physical owner retires.
        let _ = self.transaction.lock().unwrap().finish();
        // Instrumentation replacement/disposal is not proof of physical close.
        self.telemetry
            .memory_observation_disposed(&self.transaction);
    }
}

impl SpawnBlocking for SqliteRuntime {
    fn get_runtime() -> Self {
        Self::new(Duration::from_secs(15))
    }

    fn spawn_blocking<'a, R>(
        &mut self,
        task: impl FnOnce() -> R + Send + 'static,
    ) -> BoxFuture<'a, Result<R, BoxError>>
    where
        R: Send + 'static,
    {
        let predecessor = self.tail.take();
        let (finished, next) = oneshot::channel();
        self.tail = Some(next);
        let observation = current_database_observation();
        let worker_observation = observation.clone();
        let operation = current_database_operation();
        let worker_operation = operation.clone();
        // Registration occurs before predecessor wait. Runtime cancellation
        // destroys the unqueued callback before its outstanding guard retires.
        let prepared = self
            .worker
            .as_ref()
            .map(|handle| {
                handle.prepare(move || {
                    if let Some(progress) = &worker_operation {
                        progress.set_phase(WorkerPhase::Running);
                    }
                    let result = with_database_observation(
                        worker_observation,
                        worker_operation.clone(),
                        task,
                    );
                    if let Some(progress) = &worker_operation {
                        progress.set_phase(WorkerPhase::Returning);
                    }
                    result
                })
            })
            .map_err(Arc::clone);
        let waiting_operation = operation.clone();
        let worker = tokio::spawn(async move {
            if let Some(predecessor) = predecessor {
                if let Some(progress) = &waiting_operation {
                    progress.set_phase(WorkerPhase::WaitingPredecessor);
                }
                let _ = predecessor.await;
            }
            if let Some(progress) = &waiting_operation {
                progress.set_phase(WorkerPhase::BlockingQueue);
            }
            let result = match prepared {
                Ok(prepared) => prepared.run().await,
                Err(error) => Err(Box::new(std::io::Error::other(error.to_string())) as BoxError),
            };
            let _ = finished.send(());
            result
        });
        let timeout = self.operation_timeout;
        Box::pin(async move {
            let result = tokio::time::timeout(timeout, worker)
                .await
                .map_err(|_| -> BoxError {
                    const TIMEOUT: &str = "database operation timed out; cleanup remains admitted";
                    if let Some(observation) = observation {
                        observation.record_pressure(&AppError::Database(TIMEOUT.into()));
                        if let Some(operation) = operation {
                            observation.record_operation_timeout(&operation);
                        }
                    }
                    Box::new(std::io::Error::other(TIMEOUT))
                })?;
            result.map_err(|error| -> BoxError { Box::new(error) })?
        })
    }
}

struct Inner {
    pool: Pool<SqliteManager>,
    admission: Arc<Semaphore>,
    checkout_timeout: Duration,
    operation_timeout: Duration,
    admitted: AtomicUsize,
    checkouts: AtomicU64,
    telemetry: Arc<DatabaseTelemetry>,
    #[cfg(test)]
    setup_activity: Arc<SetupActivity>,
}

struct StartupCheckoutProgress(Mutex<(&'static str, Instant)>);

impl StartupCheckoutProgress {
    fn new() -> Self {
        Self(Mutex::new(("pool acquisition", Instant::now())))
    }

    fn advance(&self, stage: &'static str) {
        let (previous, started_at) = {
            let mut progress = self.0.lock().unwrap();
            std::mem::replace(&mut *progress, (stage, Instant::now()))
        };
        tracing::debug!(
            stage = previous,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            "Startup database connection stage completed"
        );
    }

    fn failure_context(&self) -> String {
        let (stage, started_at) = *self.0.lock().unwrap();
        format!(
            "Startup database checkout failed during {stage} ({} ms in stage)",
            started_at.elapsed().as_millis()
        )
    }
}

tokio::task_local! {
    static STARTUP_CHECKOUT_PROGRESS: Arc<StartupCheckoutProgress>;
    static CHECKOUT_ADMISSION: Arc<Admission>;
}

#[cfg(test)]
#[derive(Default)]
struct SetupActivity {
    started: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
}

#[cfg(test)]
struct SetupWorker(Arc<SetupActivity>);

#[cfg(test)]
impl Drop for SetupWorker {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Cloneable query provider. It contains no checked-out connection.
#[derive(Clone)]
pub struct Database(Arc<Inner>);

#[derive(Debug, Clone, Copy)]
pub struct DatabaseStatus {
    pub connections: u32,
    pub idle_connections: u32,
    pub admitted_operations: usize,
    pub checkouts: u64,
}

struct Admission {
    _permit: OwnedSemaphorePermit,
    inner: Arc<Inner>,
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.inner.admitted.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct DatabaseLease {
    connection: Option<Object<SqliteManager>>,
    admission: Option<Arc<Admission>>,
}
impl Deref for DatabaseLease {
    type Target = DbConnection;
    fn deref(&self) -> &Self::Target {
        self.connection
            .as_ref()
            .expect("live database lease")
            .connection
            .as_ref()
            .expect("live physical connection")
    }
}
impl DerefMut for DatabaseLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
            .as_mut()
            .expect("live database lease")
            .connection
            .as_mut()
            .expect("live physical connection")
    }
}
/// Defaults to disposal. Only a completed, valid cleanup barrier can return
/// a connection to the idle pool, including when its task is canceled at shutdown.
struct CleanupOwner {
    connection: Option<Object<SqliteManager>>,
}
impl CleanupOwner {
    fn return_idle(mut self) {
        let mut connection = self.connection.take().expect("one cleanup owner");
        // Publish the physical pool slot before releasing its admission. A new
        // caller must not consume that admission while the slot is still ours.
        let admission = connection.admission_anchor.take();
        drop(connection);
        drop(admission);
    }
}
impl Drop for CleanupOwner {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(Object::take(connection));
        }
    }
}
impl DatabaseLease {
    fn take_cleanup(&mut self) -> Option<impl std::future::Future<Output = ()> + Send + use<>> {
        let mut connection = self.connection.take()?;
        // This anchor remains inside the physical owner on timeout/cancellation.
        // In particular, an aborted unqueued cleanup cannot free admission while
        // its predecessor is still running on the native connection worker.
        connection.admission_anchor = self.admission.take();
        let mut owner = CleanupOwner {
            connection: Some(connection),
        };
        // SqliteRuntime starts its async coordinator during spawn_blocking.
        // Without a runtime, drop the disposal owner before creating that job.
        if tokio::runtime::Handle::try_current().is_err() {
            return None;
        }
        let cleanup = owner.connection.as_mut().expect("one cleanup owner")
            .connection.as_mut().expect("live physical connection")
            .spawn_blocking(move |connection| {
                let status = <SqliteConnection as Connection>::TransactionManager::transaction_manager_status_mut(connection);
                if matches!(status, TransactionManagerStatus::Valid(valid) if valid.transaction_depth().is_none()) {
                    sample_requested_sqlite_memory(connection);
                    Ok(())
                } else {
                    Err(Error::RollbackTransaction)
                }
            });
        Some(async move {
            if cleanup.await.is_ok() {
                owner.return_idle();
            }
        })
    }

    async fn finish(mut self) {
        if let Some(cleanup) = self.take_cleanup() {
            // Cancellation drops CleanupOwner and disposes the connection; its
            // physical owner's admission still outlives any native work.
            cleanup.await;
        }
    }
}

impl Drop for DatabaseLease {
    fn drop(&mut self) {
        let Some(cleanup) = self.take_cleanup() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(cleanup);
    }
}

/// Query-worker causal experiment: keep prior synchronous wrapper Drop when
/// idle, while deferring its admission anchor until real queued/running work ends.
struct ManagedSqlite {
    connection: Option<DbConnection>,
    worker: WorkerHandle,
    admission_anchor: Option<Arc<Admission>>,
}
struct RetireAdmission {
    worker: WorkerHandle,
    admission: Option<Arc<Admission>>,
}
impl Drop for RetireAdmission {
    fn drop(&mut self) {
        self.worker.release_after_jobs(self.admission.take());
    }
}
impl Drop for ManagedSqlite {
    fn drop(&mut self) {
        // This guard also posts the anchor on ordinary Rust unwind. Normal
        // wrapper/idle-C close completes before its caller returns, as before.
        let _admission = RetireAdmission {
            worker: self.worker.clone(),
            admission: self.admission_anchor.take(),
        };
        if let Some(connection) = self.connection.take() {
            drop(connection);
        }
    }
}

struct SqliteManager {
    url: String,
    busy_setting: String,
    operation_timeout: Duration,
    telemetry: Arc<DatabaseTelemetry>,
    #[cfg(test)]
    setup_activity: Arc<SetupActivity>,
}
impl deadpool::managed::Manager for SqliteManager {
    type Type = ManagedSqlite;
    type Error = ManagerError;

    async fn create(&self) -> Result<ManagedSqlite, Self::Error> {
        let worker = WorkerHandle::start().map_err(|error| {
            ManagerError::ConnectionError(diesel::ConnectionError::BadConnection(error.to_string()))
        })?;
        let runtime = SqliteRuntime::with_worker(self.operation_timeout, worker.clone());
        let admission = CHECKOUT_ADMISSION.try_with(Arc::clone).map_err(|_| {
            ManagerError::ConnectionError(diesel::ConnectionError::BadConnection(
                "Missing database admission owner".into(),
            ))
        })?;
        let progress = STARTUP_CHECKOUT_PROGRESS.try_with(Arc::clone).ok();
        let url = self.url.clone();
        let busy_setting = self.busy_setting.clone();
        let telemetry = self.telemetry.clone();
        let connection_worker = worker.clone();
        #[cfg(test)]
        let setup_activity = self.setup_activity.clone();
        if let Some(progress) = &progress {
            progress.advance("blocking worker queue");
        }
        let prepared = worker.prepare(move || {
            use diesel::connection::SimpleConnection;
            #[cfg(test)]
            let _activity = {
                let active = setup_activity.active.fetch_add(1, Ordering::SeqCst) + 1;
                setup_activity.peak.fetch_max(active, Ordering::SeqCst);
                setup_activity.started.fetch_add(1, Ordering::SeqCst);
                SetupWorker(setup_activity)
            };
            let advance = |stage| {
                if let Some(progress) = &progress {
                    progress.advance(stage);
                }
            };
            advance("SQLite open");
            let mut connection = SqliteConnection::establish(&url)?;
            for (stage, sql) in [
                ("SQLite busy timeout", busy_setting.as_str()),
                ("SQLite WAL setup", "PRAGMA journal_mode=WAL;"),
                ("SQLite synchronous setup", "PRAGMA synchronous=NORMAL;"),
            ] {
                advance(stage);
                connection
                    .batch_execute(sql)
                    .map_err(|error| diesel::ConnectionError::BadConnection(error.to_string()))?;
            }
            advance("pool checkout completion");
            connection.set_instrumentation(SqliteObservation {
                query_started: None,
                transaction: telemetry.connection(),
                transaction_control: None,
                telemetry,
            });
            // Build the owned wrapper ON the native worker, before handing it to
            // a cancelable async oneshot/pool creation future.
            Ok::<_, diesel::ConnectionError>(ManagedSqlite {
                connection: Some(SyncConnectionWrapper::with_runtime(connection, runtime)),
                worker: connection_worker,
                admission_anchor: Some(admission),
            })
        });
        prepared
            .run()
            .await
            .map_err(|error| {
                ManagerError::ConnectionError(diesel::ConnectionError::BadConnection(
                    error.to_string(),
                ))
            })?
            .map_err(ManagerError::ConnectionError)
    }

    async fn recycle(
        &self,
        connection: &mut ManagedSqlite,
        _: &deadpool::managed::Metrics,
    ) -> deadpool::managed::RecycleResult<Self::Error> {
        let connection = connection
            .connection
            .as_mut()
            .expect("live pooled SQLite connection");
        if std::thread::panicking() || connection.is_broken() {
            return Err(deadpool::managed::RecycleError::Message(
                "Broken connection".into(),
            ));
        }
        connection
            .ping(&RecyclingMethod::Fast)
            .await
            .map_err(ManagerError::QueryError)?;
        Ok(())
    }
}

impl Database {
    pub fn connect(config: &DatabaseConfig) -> QueryResult<Self> {
        let path = crate::services::database_backup::sqlite_url_to_path(&config.url)
            .map_err(|error| failure(error.to_string()))?;
        Self::with_timeouts(
            &path.to_string_lossy(),
            config.max_connections,
            Duration::from_millis(config.checkout_timeout_ms),
            Duration::from_millis(config.operation_timeout_ms),
            Duration::from_millis(config.busy_timeout_ms),
        )
    }

    pub fn with_timeouts(
        url: &str,
        max_connections: u32,
        checkout_timeout: Duration,
        operation_timeout: Duration,
        busy_timeout: Duration,
    ) -> QueryResult<Self> {
        if max_connections == 0
            || checkout_timeout.is_zero()
            || operation_timeout.is_zero()
            || busy_timeout.is_zero()
        {
            return Err(failure(
                "database connection count and timeouts must be positive",
            ));
        }
        let capacity = usize::try_from(max_connections)
            .map_err(|_| failure("database connection count too large"))?;
        let admitted = capacity
            .checked_mul(2)
            .filter(|size| *size <= Semaphore::MAX_PERMITS)
            .ok_or_else(|| failure("database admission capacity too large"))?;
        let busy_ms = busy_timeout.as_millis().min(i32::MAX as u128);
        let busy_setting = format!("PRAGMA busy_timeout={busy_ms};");
        let telemetry = DatabaseTelemetry::new(capacity);
        #[cfg(test)]
        let setup_activity = Arc::new(SetupActivity::default());
        let manager = SqliteManager {
            url: url.to_owned(),
            busy_setting,
            operation_timeout,
            telemetry: telemetry.clone(),
            #[cfg(test)]
            setup_activity: setup_activity.clone(),
        };
        let pool = Pool::builder(manager)
            .max_size(capacity)
            .runtime(deadpool::Runtime::Tokio1)
            .build()
            .map_err(|error| failure(error.to_string()))?;
        Ok(Self(Arc::new(Inner {
            pool,
            admission: Arc::new(Semaphore::new(admitted)),
            checkout_timeout,
            operation_timeout,
            admitted: AtomicUsize::new(0),
            checkouts: AtomicU64::new(0),
            telemetry,
            #[cfg(test)]
            setup_activity,
        })))
    }

    /// Compatibility with old call sites: this only clones a provider, never a lease.
    pub fn get(&self) -> QueryResult<Self> {
        Ok(self.clone())
    }

    pub fn state(&self) -> DatabaseStatus {
        let status = self.0.pool.status();
        DatabaseStatus {
            connections: status.size as u32,
            idle_connections: status.available as u32,
            admitted_operations: self.0.admitted.load(Ordering::Relaxed),
            checkouts: self.0.checkouts.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn observe_gc(&self) -> (GcWorkerObservation, GcObservation) {
        self.0.telemetry.start_gc()
    }

    #[cfg(test)]
    pub(crate) fn telemetry(&self) -> Arc<DatabaseTelemetry> {
        self.0.telemetry.clone()
    }

    /// Run legacy pure synchronous SQL helpers on the adapter's blocking worker.
    /// This closure cannot borrow its connection across an HTTP/storage await.
    pub async fn run<R, E, F>(&self, callback: F) -> Result<R, E>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<R, E> + Send + 'static,
        R: Send + 'static,
        E: From<Error> + Send + 'static,
    {
        let mut provider = self.clone();
        let mut connection = provider
            .get_mut_conn()
            .await
            .map_err(query_error)
            .map_err(E::from)?;
        let result = observe_database_operation(|| {
            connection.spawn_blocking(move |connection| Ok(callback(connection)))
        })
        .await;
        match result {
            Ok(result) => {
                // The callback has finished. Do not accumulate its cleanup as
                // hidden admission pressure across sequential short queries.
                // Cleanup failure retires the slot without changing a possibly
                // committed business result into a retryable error.
                connection.finish().await;
                result
            }
            // An overdue/running callback retains the existing Drop fallback;
            // never extend an operation deadline by awaiting a second barrier.
            Err(error) => Err(E::from(error)),
        }
    }

    /// Open the first pooled connection and run migrations before starting
    /// traffic. The short request lease deadline also covers queueing, so it
    /// can mask the SQLite busy timeout during cold connection setup.
    pub async fn run_startup_migrations<F>(&self, callback: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut SqliteConnection) -> anyhow::Result<()> + Send + 'static,
    {
        let progress = Arc::new(StartupCheckoutProgress::new());
        let timeout = self.0.checkout_timeout.max(self.0.operation_timeout);
        let started_at = Instant::now();
        let mut connection = self
            .checkout(timeout, Some(progress.clone()))
            .await
            .map_err(query_error)
            .with_context(|| progress.failure_context())?;
        tracing::info!(
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            "Startup database connection ready"
        );
        connection
            .spawn_blocking(move |connection| Ok(callback(connection)))
            .await
            .context("Startup database migration worker failed")?
            .context("Startup database migrations failed")
    }

    async fn checkout(
        &self,
        timeout: Duration,
        startup: Option<Arc<StartupCheckoutProgress>>,
    ) -> horme::Result<DatabaseLease> {
        self.0.telemetry.ensure_monitor();
        let permit = self
            .0
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| horme::Error::from(failure("database admission capacity exceeded")))?;
        self.0.admitted.fetch_add(1, Ordering::Relaxed);
        let admission = Arc::new(Admission {
            _permit: permit,
            inner: self.0.clone(),
        });
        let inner = self.0.clone();
        // Cancellation only detaches this owner. In particular, an in-progress
        // SQLite setup must retain both deadpool's slot and our admission until
        // its blocking worker finishes. A late lease follows the usual cleanup.
        let checkout = tokio::spawn(async move {
            let acquisition = CHECKOUT_ADMISSION.scope(admission.clone(), inner.pool.get());
            let mut connection = match startup {
                Some(progress) => STARTUP_CHECKOUT_PROGRESS.scope(progress, acquisition).await,
                None => acquisition.await,
            }
            .map_err(|error| horme::Error::Acquire(Box::new(error)))?;
            // No await separates transfer from the setup/retirement anchor to
            // DatabaseLease. An aborted ready output is itself safely disposed.
            connection.admission_anchor = None;
            inner.checkouts.fetch_add(1, Ordering::Relaxed);
            Ok::<_, horme::Error>(DatabaseLease {
                connection: Some(connection),
                admission: Some(admission),
            })
        });
        tokio::time::timeout(timeout, checkout)
            .await
            .map_err(|_| horme::Error::from(failure("database checkout timed out")))?
            .map_err(|error| {
                horme::Error::from(failure(format!("database checkout task failed: {error}")))
            })?
    }
}

impl GetConnection for Database {
    type Connection = DbConnection;
    type Lease<'a> = DatabaseLease;

    async fn get_mut_conn(&mut self) -> horme::Result<Self::Lease<'_>> {
        let Some(observation) = current_database_observation() else {
            return self.checkout(self.0.checkout_timeout, None).await;
        };
        let started = Instant::now();
        observation
            .checkout_attempts
            .fetch_add(1, Ordering::Relaxed);
        let result = self.checkout(self.0.checkout_timeout, None).await;
        observation
            .checkout_wait_us
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        match &result {
            Ok(_) => {
                observation
                    .checkouts_acquired
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                observation.record_pressure(&AppError::Database(error.to_string()));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horme::prelude::*;

    fn single_slot(operation_ms: u64) -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let database = Database::with_timeouts(
            &path.to_string_lossy(),
            1,
            Duration::from_secs(1),
            Duration::from_millis(operation_ms),
            Duration::from_millis(30),
        )
        .unwrap();
        (directory, database)
    }

    async fn settled(database: &Database) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while {
                let status = database.state();
                status.admitted_operations != 0 || status.connections != status.idle_connections
            } {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("all blocking cleanup must retire");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_leases_do_not_survive_external_waits_on_clone_handles() {
        let (_directory, database) = single_slot(500);
        let mut first = database.get().unwrap();
        let mut second = database.clone();
        assert_eq!(
            database.state().checkouts,
            0,
            "constructing handles performs no checkout"
        );
        let value: i32 = diesel::select(7.into_sql::<diesel::sql_types::Integer>())
            .get_result(&mut first)
            .await
            .unwrap();
        assert_eq!(value, 7);
        // The first handle stays alive across an unrelated asynchronous wait.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let other: i32 = diesel::select(9.into_sql::<diesel::sql_types::Integer>())
            .get_result(&mut second)
            .await
            .unwrap();
        assert_eq!(other, 9);
        settled(&database).await;
        assert_eq!(database.state().checkouts, 2);
        assert_eq!(database.state().idle_connections, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completed_runs_return_the_slot_before_exposing_business_results() {
        use diesel::connection::SimpleConnection;

        let (_directory, database) = single_slot(5_000);
        database
            .run(|connection| {
                connection.batch_execute(
                    "CREATE TEMP TABLE completed_run(value INTEGER); INSERT INTO completed_run VALUES(7)",
                )
            })
            .await
            .unwrap();
        // No yield, wait-for-idle or retry may hide a leftover cleanup lease.
        assert_eq!(database.state().admitted_operations, 0);
        assert_eq!(database.state().idle_connections, 1);
        let result = database
            .run(|_| Err::<(), _>(diesel::result::Error::RollbackTransaction))
            .await;
        assert!(matches!(
            result,
            Err(diesel::result::Error::RollbackTransaction)
        ));
        assert_eq!(database.state().admitted_operations, 0);
        assert_eq!(database.state().idle_connections, 1);
        let value = database
            .run(|connection| {
                diesel::RunQueryDsl::get_result::<i32>(
                    diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
                        "(SELECT value FROM completed_run)",
                    )),
                    connection,
                )
            })
            .await
            .unwrap();
        assert_eq!(value, 7);
        assert_eq!(database.state().admitted_operations, 0);
        assert_eq!(database.state().idle_connections, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cleanup_retirement_preserves_an_already_committed_business_result() {
        use diesel::connection::SimpleConnection;

        let (directory, database) = single_slot(5_000);
        let result = database
            .run(|connection| {
                connection.batch_execute(
                    "CREATE TABLE committed_result(value INTEGER); INSERT INTO committed_result VALUES(1)",
                )?;
                <SqliteConnection as Connection>::TransactionManager::begin_transaction(connection)?;
                connection.batch_execute("INSERT INTO committed_result VALUES(2)")?;
                Ok::<_, diesel::result::Error>(42)
            })
            .await
            .unwrap();
        assert_eq!(
            result, 42,
            "cleanup must not invite retrying a committed write"
        );
        settled(&database).await;
        assert_eq!(database.state().connections, 0);
        let mut peer =
            SqliteConnection::establish(directory.path().join("runtime.sqlite").to_str().unwrap())
                .unwrap();
        // A new app checkout must not hide an improperly recycled writer.
        peer.batch_execute("PRAGMA busy_timeout=0; BEGIN IMMEDIATE; ROLLBACK;")
            .unwrap();
        let value = diesel::RunQueryDsl::get_result::<i32>(
            diesel::select(diesel::dsl::sql::<diesel::sql_types::Integer>(
                "(SELECT SUM(value) FROM committed_result)",
            )),
            &mut peer,
        )
        .unwrap();
        assert_eq!(value, 1, "keep autocommit, roll back abandoned transaction");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_blocking_sql_keeps_admission_until_the_worker_really_finishes() {
        let (_directory, database) = single_slot(5_000);
        let (started, running) = oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let (finished, completion) = oneshot::channel();
        let provider = database.clone();
        let operation = tokio::spawn(async move {
            provider
                .run(move |_| {
                    let _ = started.send(());
                    blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                    let _ = finished.send(());
                    Ok::<_, diesel::result::Error>(())
                })
                .await
        });
        if let Err(error) = tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
        {
            panic!(
                "callback did not start: {error}; operation result: {:?}",
                operation.await
            );
        }
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        assert_eq!(
            database.state().admitted_operations,
            1,
            "cancelled caller cannot release running work"
        );
        // This yield must progress while the synchronous callback is gated.
        tokio::task::yield_now().await;
        let mut next = database.clone();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(40),
                diesel::select(1.into_sql::<diesel::sql_types::Integer>())
                    .get_result::<i32>(&mut next)
            )
            .await
            .is_err(),
            "the busy slot cannot be borrowed before cleanup"
        );
        assert_eq!(database.state().checkouts, 1);
        assert_eq!(database.state().admitted_operations, 2);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), completion)
            .await
            .unwrap()
            .unwrap();
        settled(&database).await;
        let value: i32 = diesel::select(1.into_sql::<diesel::sql_types::Integer>())
            .get_result(&mut next)
            .await
            .unwrap();
        assert_eq!(value, 1);
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn operation_deadline_retains_capacity_even_when_cleanup_also_times_out() {
        let (_directory, database) = single_slot(25);
        let (started, running) = oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let (finished, completion) = oneshot::channel();
        let provider = database.clone();
        let operation = tokio::spawn(async move {
            provider
                .run(move |_| {
                    let _ = started.send(());
                    blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                    let _ = finished.send(());
                    Ok::<_, diesel::result::Error>(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap();
        assert!(operation.await.unwrap().is_err());
        // Observe cleanup discarding its slot after its own deadline rather
        // than assuming both deadlines elapsed within a fixed wall-clock sleep.
        tokio::time::timeout(Duration::from_secs(3), async {
            while database.state().connections != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            database.state().admitted_operations,
            1,
            "discarding a timed-out pool slot must not free its running work's admission"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), completion)
            .await
            .unwrap()
            .unwrap();
        settled(&database).await;
        let mut next = database.clone();
        assert_eq!(
            diesel::select(1.into_sql::<diesel::sql_types::Integer>())
                .get_result::<i32>(&mut next)
                .await
                .unwrap(),
            1
        );
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_transaction_is_discarded_and_rolls_back() {
        let (_directory, database) = single_slot(500);
        let mut provider = database.clone();
        diesel::sql_query("CREATE TABLE entries(value INTEGER NOT NULL)")
            .execute(&mut provider)
            .await
            .unwrap();
        settled(&database).await;
        let (inserted, observed) = oneshot::channel();
        let mut transaction =
            Box::pin(horme::transaction(&mut provider, async move |connection| {
                diesel::sql_query("INSERT INTO entries(value) VALUES (7)")
                    .execute(connection)
                    .await?;
                let _ = inserted.send(());
                std::future::pending::<horme::Result<()>>().await
            }));
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::select! {
                result = &mut transaction => panic!("transaction ended before cancellation: {result:?}"),
                result = observed => result.unwrap(),
            }
        }).await.unwrap();
        drop(transaction);
        settled(&database).await;
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            total: i64,
        }
        let count: Count = diesel::sql_query("SELECT COUNT(*) AS total FROM entries")
            .get_result(&mut provider)
            .await
            .unwrap();
        assert_eq!(
            count.total, 0,
            "an abandoned transaction must not be committed/recycled"
        );
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn admission_is_bounded_and_checkout_expires() {
        let (_directory, database) = single_slot(500);
        // Cold WAL setup is covered separately. Use a longer deadline only
        // for prewarming, then measure checkout against an occupied slot.
        let warm = database
            .checkout(Duration::from_secs(5), None)
            .await
            .expect("initialize the single-slot pool before testing admission");
        drop(warm);
        settled(&database).await;
        assert_eq!(database.state().idle_connections, 1);
        let mut provider = database.clone();
        let lease = provider.get_mut_conn().await.unwrap();
        let mut waiter = database.clone();
        let waiting = tokio::spawn(async move { waiter.get_mut_conn().await.map(|_| ()) });
        tokio::time::timeout(Duration::from_secs(1), async {
            while database.state().admitted_operations != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut excess = database.clone();
        let error = excess
            .get_mut_conn()
            .await
            .err()
            .expect("a third operation is rejected immediately");
        assert!(error.to_string().contains("admission capacity"));
        let error = waiting.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("database checkout timed out"));
        drop(lease);
        settled(&database).await;
        assert_eq!(database.state().idle_connections, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_cold_checkouts_keep_setup_workers_and_admission_bounded() {
        use diesel::connection::SimpleConnection;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked-setup.sqlite");
        let url = path.to_string_lossy();
        let mut lock = SqliteConnection::establish(&url).unwrap();
        lock.batch_execute(
            "PRAGMA journal_mode=DELETE; CREATE TABLE entries(value INTEGER); BEGIN EXCLUSIVE;",
        )
        .unwrap();
        let database = Database::with_timeouts(
            &url,
            1,
            Duration::from_millis(200),
            Duration::from_secs(5),
            Duration::from_secs(10),
        )
        .unwrap();

        let mut first = database.clone();
        let timed_out = tokio::spawn(async move { first.get_mut_conn().await.map(|_| ()) });
        // Observe the real blocking setup rather than assuming it started within
        // a short Windows scheduling window. The exclusive lock blocks its WAL PRAGMA.
        tokio::time::timeout(Duration::from_secs(3), async {
            while database.0.setup_activity.started.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("SQLite setup must start");
        let error = timed_out.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("checkout timed out"));
        assert_eq!(database.state().admitted_operations, 1);
        assert_eq!(database.0.setup_activity.active.load(Ordering::SeqCst), 1);

        let mut second = database.clone();
        let cancelled = tokio::spawn(async move { second.get_mut_conn().await.map(|_| ()) });
        tokio::time::timeout(Duration::from_secs(3), async {
            while database.0.pool.status().waiting != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both checkout owners must reach the single-slot pool");
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());

        for _ in 0..16 {
            let mut excess = database.clone();
            let error = excess
                .get_mut_conn()
                .await
                .err()
                .expect("cancelled and timed-out setup still consume admission");
            assert!(error.to_string().contains("admission capacity"));
            assert_eq!(database.state().admitted_operations, 2);
        }
        assert_eq!(database.0.setup_activity.started.load(Ordering::SeqCst), 1);
        assert_eq!(database.0.setup_activity.peak.load(Ordering::SeqCst), 1);
        assert_eq!(database.state().connections, 0);
        assert_eq!(database.state().checkouts, 0);

        lock.batch_execute("ROLLBACK").unwrap();
        settled(&database).await;
        assert_eq!(database.0.setup_activity.active.load(Ordering::SeqCst), 0);
        assert_eq!(database.0.setup_activity.started.load(Ordering::SeqCst), 1);
        assert_eq!(database.state().checkouts, 2);
        assert_eq!(database.state().connections, 1);
        assert_eq!(database.state().idle_connections, 1);
        let mut recovered = database.clone();
        assert_eq!(
            diesel::select(42.into_sql::<diesel::sql_types::Integer>())
                .get_result::<i32>(&mut recovered)
                .await
                .unwrap(),
            42,
            "a late setup and cancelled waiter must both retire cleanly"
        );
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_exposes_sqlite_setup_failure_and_reuses_the_migration_connection() {
        use diesel::connection::SimpleConnection;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("startup.sqlite");
        let url = path.to_string_lossy();
        let mut lock = SqliteConnection::establish(&url).unwrap();
        lock.batch_execute(
            "PRAGMA journal_mode=DELETE; CREATE TABLE existing(value INTEGER); BEGIN EXCLUSIVE;",
        )
        .unwrap();

        // This lock outlasts the short request checkout deadline. Startup
        // must expose the SQLite setup failure before attempting migrations.
        let database = Database::with_timeouts(
            &url,
            1,
            Duration::from_millis(25),
            Duration::from_secs(5),
            Duration::from_millis(250),
        )
        .unwrap();
        let error = database
            .run_startup_migrations(|_| panic!("migrations ran without a ready connection"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SQLite WAL setup"), "{error:#}");
        assert!(format!("{error:#}").contains("database is locked"));
        assert_eq!(database.state().checkouts, 0);

        lock.batch_execute("ROLLBACK").unwrap();
        drop(lock);
        database
            .run_startup_migrations(|connection| {
                connection.batch_execute(
                    "CREATE TEMP TABLE startup_probe(value INTEGER); \
                     INSERT INTO startup_probe VALUES (42);",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        settled(&database).await;
        assert_eq!(database.state().checkouts, 1);

        #[derive(diesel::QueryableByName)]
        struct Probe {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            value: i32,
        }
        let mut provider = database.clone();
        let probe: Probe = diesel::sql_query("SELECT value FROM startup_probe")
            .get_result(&mut provider)
            .await
            .unwrap();
        assert_eq!(probe.value, 42);
        assert_eq!(database.0.setup_activity.started.load(Ordering::SeqCst), 2);
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_distinguishes_migration_errors_from_connection_errors() {
        use diesel::connection::SimpleConnection;

        let (_directory, database) = single_slot(5_000);
        let error = database
            .run_startup_migrations(|connection| {
                connection.batch_execute("INSERT INTO absent_table VALUES (1);")?;
                Ok(())
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Startup database migrations failed");
        assert!(format!("{error:#}").contains("no such table: absent_table"));
        assert_eq!(database.state().checkouts, 1);
        settled(&database).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn connections_use_wal_and_busy_timeout() {
        let (_directory, database) = single_slot(500);
        let mut provider = database.clone();
        #[derive(diesel::QueryableByName)]
        struct JournalMode {
            #[diesel(sql_type = diesel::sql_types::Text)]
            journal_mode: String,
        }
        let enabled: JournalMode = diesel::sql_query("PRAGMA journal_mode")
            .get_result(&mut provider)
            .await
            .unwrap();
        assert_eq!(enabled.journal_mode, "wal");
        #[derive(diesel::QueryableByName)]
        struct BusyTimeout {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            timeout: i32,
        }
        let timeout: BusyTimeout = diesel::sql_query("PRAGMA busy_timeout")
            .get_result(&mut provider)
            .await
            .unwrap();
        assert_eq!(timeout.timeout, 30);
        settled(&database).await;
    }
}

// Bounded diagnostic of physical SQLite connection thread migration.
#[cfg(test)]
mod thread_migration_probe {
    use super::*;
    use diesel::RunQueryDsl;
    use std::collections::HashSet;
    use std::thread::{self, ThreadId};

    #[derive(diesel::QueryableByName)]
    struct ProbeRow {
        #[diesel(sql_type = diesel::sql_types::Text)]
        marker: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        counter: i64,
    }

    async fn step(lease: &mut DatabaseLease) -> (ProbeRow, ThreadId) {
        lease
            .spawn_blocking(|connection| {
                diesel::sql_query("UPDATE temp.worker_probe SET counter=counter+1")
                    .execute(connection)?;
                let row = diesel::sql_query("SELECT marker,counter FROM temp.worker_probe")
                    .get_result::<ProbeRow>(connection)?;
                Ok((row, thread::current().id()))
            })
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "bounded diagnostic; explicit coordinator resource window required"]
    async fn real_database_thread_migration_probe_capacity_one_and_five() {
        const CALLS_PER_CONNECTION: usize = 32;
        const BLOCKED_JOBS: usize = 8;
        for capacity in [1_u32, 5_u32] {
            let directory = tempfile::tempdir().unwrap();
            let database = Database::with_timeouts(
                directory.path().join("probe.sqlite").to_str().unwrap(),
                capacity,
                Duration::from_secs(3),
                Duration::from_secs(15),
                Duration::from_secs(3),
            )
            .unwrap();
            let migrations =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
            database
                .run_startup_migrations(move |connection| {
                    crate::migrations::run_directory(connection, &migrations)
                })
                .await
                .unwrap();
            // Keep each lease throughout the experiment. Its TEMP row is an
            // additional identity check, independent of the thread identifier.
            let mut leases = Vec::new();
            for index in 0..capacity {
                let mut lease = database
                    .checkout(Duration::from_secs(3), None)
                    .await
                    .unwrap();
                let marker = format!("connection-{capacity}-{index}");
                let row_marker = marker.clone();
                lease.spawn_blocking(move |connection| {
                    diesel::sql_query("CREATE TEMP TABLE worker_probe(marker TEXT NOT NULL,counter INTEGER NOT NULL)")
                        .execute(connection)?;
                    diesel::sql_query("INSERT INTO temp.worker_probe(marker,counter) VALUES(?,0)")
                        .bind::<diesel::sql_types::Text,_>(&row_marker).execute(connection)?;
                    Ok(())
                }).await.unwrap();
                leases.push((
                    marker,
                    lease,
                    HashSet::<ThreadId>::new(),
                    Vec::<String>::new(),
                ));
            }
            let dummy_path = directory.path().join("dummy.bin");
            std::fs::write(&dummy_path, [7_u8; 4096]).unwrap();
            let mut expected = vec![0_i64; capacity as usize];
            for _ in 0..CALLS_PER_CONNECTION {
                for (index, (marker, lease, threads, trace)) in leases.iter_mut().enumerate() {
                    // Genuine small Tokio FS work shares the ordinary blocking
                    // pool. No thread/allocator/runtime limit is changed.
                    let data = tokio::fs::read(&dummy_path).await.unwrap();
                    assert_eq!(data.len(), 4096);
                    let (row, tid) = step(lease).await;
                    expected[index] += 1;
                    assert_eq!((&row.marker, row.counter), (&*marker, expected[index]));
                    threads.insert(tid);
                    trace.push(format!("{tid:?}"));
                }
            }
            let previous: Vec<_> = leases
                .iter()
                .map(|(_, _, _, trace)| trace.last().unwrap().clone())
                .collect();
            let mut release = Vec::new();
            let mut blockers = Vec::new();
            let mut occupied = HashSet::new();
            for _ in 0..BLOCKED_JOBS {
                let path = dummy_path.clone();
                let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
                let (ready_sender, ready_receiver) = oneshot::channel();
                release.push(release_sender);
                blockers.push(tokio::task::spawn_blocking(move || {
                    let data = std::fs::read(path).unwrap();
                    assert_eq!(data.len(), 4096);
                    ready_sender.send(thread::current().id()).unwrap();
                    // The owned senders also release on unwind. These jobs do
                    // not hold a DB lease or lock; no timeout is tuned.
                    let _ = release_receiver.recv();
                }));
                occupied.insert(ready_receiver.await.unwrap());
            }
            assert_eq!(occupied.len(), BLOCKED_JOBS);
            let occupied_labels: HashSet<_> =
                occupied.iter().map(|tid| format!("{tid:?}")).collect();
            let mut connections = Vec::new();
            for (index, (marker, lease, threads, trace)) in leases.iter_mut().enumerate() {
                let positive_control_covered = occupied_labels.contains(&previous[index]);
                let (row, tid) = step(lease).await;
                expected[index] += 1;
                assert_eq!((&row.marker, row.counter), (&*marker, expected[index]));
                assert!(
                    !occupied.contains(&tid),
                    "SQL cannot run on a still-blocked worker"
                );
                threads.insert(tid);
                trace.push(format!("{tid:?}"));
                let migration_observed = threads.len() > 1;
                if positive_control_covered {
                    assert_ne!(trace.last().unwrap(), &previous[index]);
                    assert!(migration_observed);
                }
                connections.push(serde_json::json!({
                    "connection": index, "sql_calls": expected[index],
                    "distinct_worker_threads": threads.len(), "trace": trace,
                    "migration_observed": migration_observed,
                    "positive_control": if positive_control_covered {"covered_and_migrated"} else {"inconclusive_previous_worker_not_occupied"}
                }));
            }
            // Release and await every owned dummy job before returning leases.
            drop(release);
            for blocker in blockers {
                blocker.await.unwrap();
            }
            drop(leases);
            tokio::time::timeout(Duration::from_secs(3), async {
                while {
                    let state = database.state();
                    state.admitted_operations != 0 || state.connections != state.idle_connections
                } {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(database.state().connections, capacity);
            assert_eq!(database.state().idle_connections, capacity);
            println!(
                "THREAD_MIGRATION_PROBE {}",
                serde_json::json!({
                    "capacity": capacity, "calls_per_connection": CALLS_PER_CONNECTION + 1,
                    "blocked_jobs": BLOCKED_JOBS, "connections": connections,
                    "sql_and_cleanup_verified": true, "rss_or_d07_causation_proved": false
                })
            );
            drop(database);
            directory.close().unwrap();
        }
    }
}

// Regression for cancellation of an unpolled database cleanup owner.
#[cfg(test)]
mod cleanup_owner_cancellation_regression {
    use super::*;
    use diesel::RunQueryDsl;
    use diesel::connection::SimpleConnection;

    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[test]
    fn shutdown_before_cleanup_owner_is_polled_retires_the_sqlite_writer() {
        retained_writer_is_disposed(false);
    }

    #[test]
    fn dropping_a_lease_without_entering_runtime_retires_the_sqlite_writer() {
        retained_writer_is_disposed(true);
    }

    fn retained_writer_is_disposed(without_runtime: bool) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cleanup-owner.sqlite");
        let url = path.to_str().unwrap().to_owned();
        let database = Database::with_timeouts(
            &url,
            1,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let lease = runtime.block_on(async {
            database
                .run_startup_migrations(|connection| {
                    connection.batch_execute(
                        "CREATE TABLE cleanup_owner_probe(value INTEGER NOT NULL);",
                    )?;
                    Ok(())
                })
                .await
                .unwrap();
            let mut lease = database
                .checkout(Duration::from_secs(3), None)
                .await
                .unwrap();
            lease
                .spawn_blocking(|connection| {
                    // Use Diesel's managed transaction, not raw BEGIN with an
                    // invisible transaction-manager depth. INSERT obtains a
                    // real SQLite writer lock before this worker returns.
                    <SqliteConnection as Connection>::TransactionManager::begin_transaction(
                        connection,
                    )?;
                    diesel::sql_query("INSERT INTO cleanup_owner_probe(value) VALUES(42)")
                        .execute(connection)?;
                    let status = <SqliteConnection as Connection>::TransactionManager::transaction_manager_status_mut(connection);
                    assert!(matches!(status, TransactionManagerStatus::Valid(valid) if valid.transaction_depth().is_some()));
                    Ok(())
                })
                .await
                .unwrap();
            lease
        });

        let mut peer = SqliteConnection::establish(&url).unwrap();
        peer.batch_execute("PRAGMA busy_timeout=0;").unwrap();
        let before = peer
            .batch_execute("BEGIN IMMEDIATE; ROLLBACK;")
            .expect_err("positive control: retained lease must hold the SQLite writer");
        assert!(before.to_string().contains("locked"), "{before}");

        // Entering is not driving a current-thread runtime. drop(lease) queues
        // its real cleanup worker/owner, but neither new async task can run in
        // this scope or after it: there is no subsequent block_on/yield/sleep.
        if without_runtime {
            // No Tokio context: cleanup creation must dispose without spawning.
            drop(lease);
            assert_eq!(database.state().idle_connections, 0);
        } else {
            let _entered = runtime.enter();
            drop(lease);
            assert_eq!(database.state().idle_connections, 0);
        }
        // Current-thread shutdown cancels and drops queued async futures. The
        // pool remains strongly owned by `database`, so default Object::drop
        // would put the unchecked writer into idle instead of closing it.
        drop(runtime);

        // No new application checkout occurs: recycle-on-next-borrow cannot
        // hide a lingering idle writer. This must work without waiting for a
        // later request or a timer, and must not commit the abandoned write.
        peer.batch_execute("BEGIN IMMEDIATE; ROLLBACK;")
            .expect("cancelled cleanup owner must retire its SQLite writer immediately");
        let count: Count = diesel::sql_query("SELECT COUNT(*) AS count FROM cleanup_owner_probe")
            .get_result(&mut peer)
            .unwrap();
        assert_eq!(
            count.count, 0,
            "abandoned write must roll back, never publish"
        );
        let state = database.state();
        assert_eq!(
            state.connections, 0,
            "uncertified connection must leave the pool"
        );
        assert_eq!(state.idle_connections, 0);
        assert_eq!(state.admitted_operations, 0);

        drop(peer);
        drop(database);
        directory.close().unwrap();
    }
}
// Real SQLite and worker lifecycle regressions.
#[cfg(test)]
mod query_worker_regressions {
    use super::*;
    use diesel::RunQueryDsl;
    use diesel::connection::SimpleConnection;
    use std::collections::HashSet;
    use std::thread::{self, ThreadId};

    #[derive(diesel::QueryableByName)]
    struct Marker {
        #[diesel(sql_type = diesel::sql_types::Text)]
        marker: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        value: i64,
    }
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }
    fn database(capacity: u32) -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::with_timeouts(
            directory
                .path()
                .join("query-worker.sqlite")
                .to_str()
                .unwrap(),
            capacity,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        (directory, database)
    }
    async fn migrate(database: &Database) {
        database
            .run_startup_migrations(|connection| {
                crate::migrations::run_directory(
                    connection,
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
                )?;
                connection.batch_execute(
                    "CREATE TABLE native_lifecycle_probe(value INTEGER NOT NULL);",
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }
    async fn settled(database: &Database) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let state = database.state();
                if state.admitted_operations == 0 && state.connections == state.idle_connections {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real lease barriers must finish");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn every_physical_database_connection_keeps_one_query_thread() {
        for capacity in [1_u32, 5_u32] {
            let (directory, database) = database(capacity);
            migrate(&database).await;
            let (stopped_send, stopped_recv) = std::sync::mpsc::channel();
            let mut leases = Vec::new();
            let mut workers = Vec::new();
            for index in 0..capacity {
                let mut lease = database
                    .checkout(Duration::from_secs(3), None)
                    .await
                    .unwrap();
                let worker = lease.connection.as_ref().unwrap().worker.clone();
                worker.test_on_stopped(stopped_send.clone());
                let marker = format!("physical-{capacity}-{index}");
                let stored = marker.clone();
                let initialized: ThreadId = lease.spawn_blocking(move |connection| {
                    connection.batch_execute("CREATE TEMP TABLE fixed_worker_probe(marker TEXT NOT NULL,value INTEGER NOT NULL);")?;
                    diesel::sql_query("INSERT INTO temp.fixed_worker_probe(marker,value) VALUES(?,0)")
                        .bind::<diesel::sql_types::Text,_>(stored).execute(connection)?;
                    Ok(thread::current().id())
                }).await.unwrap();
                leases.push((lease, marker, HashSet::from([initialized])));
                workers.push(worker);
            }
            // Genuine bounded global-pool FS/blocking work may not move a
            // physical SQLite connection away from its dedicated query worker.
            let path = directory.path().join("dummy.bin");
            std::fs::write(&path, [7_u8; 4096]).unwrap();
            let mut releases = Vec::new();
            let mut blockers = Vec::new();
            for _ in 0..8 {
                let path = path.clone();
                let (release_send, release_recv) = std::sync::mpsc::channel::<()>();
                let (started_send, started_recv) = oneshot::channel();
                releases.push(release_send);
                blockers.push(tokio::task::spawn_blocking(move || {
                    assert_eq!(std::fs::read(path).unwrap().len(), 4096);
                    started_send.send(()).unwrap();
                    let _ = release_recv.recv();
                }));
                tokio::time::timeout(Duration::from_secs(3), started_recv)
                    .await
                    .unwrap()
                    .unwrap();
            }
            for value in 1_i64..=32 {
                for (lease, expected, threads) in &mut leases {
                    let (row, tid) = lease
                        .spawn_blocking(|connection| {
                            diesel::sql_query("UPDATE temp.fixed_worker_probe SET value=value+1")
                                .execute(connection)?;
                            let row = diesel::sql_query(
                                "SELECT marker,value FROM temp.fixed_worker_probe",
                            )
                            .get_result::<Marker>(connection)?;
                            Ok((row, thread::current().id()))
                        })
                        .await
                        .unwrap();
                    assert_eq!((&row.marker, row.value), (&*expected, value));
                    threads.insert(tid);
                    assert_eq!(
                        threads.len(),
                        1,
                        "same TEMP owner must keep exactly one query ThreadId"
                    );
                }
            }
            drop(releases);
            for blocker in blockers {
                blocker.await.unwrap();
            }
            drop(leases);
            settled(&database).await;
            drop(database); // Idle C close preserves its original synchronous contract.
            directory.close().unwrap();
            drop(workers);
            for _ in 0..capacity {
                stopped_recv.recv_timeout(Duration::from_secs(3)).unwrap();
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn declared_fifo_survives_a_cancelled_queued_caller() {
        let mut runtime = SqliteRuntime::new(Duration::from_secs(15));
        let worker = runtime.worker.as_ref().unwrap().clone();
        let (stopped_send, stopped_recv) = std::sync::mpsc::channel();
        worker.test_on_stopped(stopped_send);
        let order = Arc::new(Mutex::new(Vec::new()));
        let first_order = order.clone();
        let (started_send, started_recv) = oneshot::channel();
        let (release_send, release_recv) = std::sync::mpsc::channel::<()>();
        let first = runtime.spawn_blocking(move || {
            first_order.lock().unwrap().push(1);
            started_send.send(()).unwrap();
            let _ = release_recv.recv();
        });
        let second_order = order.clone();
        let cancelled = runtime.spawn_blocking(move || second_order.lock().unwrap().push(2));
        let third_order = order.clone();
        let third = runtime.spawn_blocking(move || third_order.lock().unwrap().push(3));
        drop(cancelled); // Admitted detached work is not silently aborted or replayed.
        tokio::time::timeout(Duration::from_secs(3), started_recv)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*order.lock().unwrap(), [1]);
        release_send.send(()).unwrap();
        first.await.unwrap();
        third.await.unwrap();
        assert_eq!(*order.lock().unwrap(), [1, 2, 3]);
        drop(runtime);
        drop(worker);
        stopped_recv.recv_timeout(Duration::from_secs(3)).unwrap();
    }

    #[test]
    fn queued_sql_survives_runtime_teardown_and_holds_admission_until_owner_destruction() {
        let (directory, database) = database(1);
        let url = directory.path().join("query-worker.sqlite");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (dummy_release_send, dummy_release_recv) = std::sync::mpsc::channel::<()>();
        let (sql_release_send, sql_release_recv) = std::sync::mpsc::channel::<()>();
        let (sql_started_send, sql_started_recv) = std::sync::mpsc::channel();
        let (released_send, released_recv) = std::sync::mpsc::channel();
        let (stopped_send, stopped_recv) = std::sync::mpsc::channel();
        let (lease, worker, sql, dummy) = runtime.block_on(async {
            migrate(&database).await;
            let mut lease = database
                .checkout(Duration::from_secs(3), None)
                .await
                .unwrap();
            let worker = lease.connection.as_ref().unwrap().worker.clone();
            worker.test_on_release(released_send);
            worker.test_on_stopped(stopped_send);
            let (dummy_started_send, dummy_started_recv) = oneshot::channel();
            let dummy = worker.prepare(move || {
                dummy_started_send.send(()).unwrap();
                let _ = dummy_release_recv.recv();
            });
            let dummy = tokio::spawn(dummy.run());
            tokio::time::timeout(Duration::from_secs(3), dummy_started_recv)
                .await
                .unwrap()
                .unwrap();
            let (queued_send, queued_recv) = oneshot::channel();
            worker.test_on_queued(queued_send);
            let sql: BoxFuture<'static, QueryResult<()>> =
                lease.spawn_blocking(move |connection| {
                    <SqliteConnection as Connection>::TransactionManager::begin_transaction(
                        connection,
                    )?;
                    diesel::sql_query("INSERT INTO native_lifecycle_probe(value) VALUES(77)")
                        .execute(connection)?;
                    sql_started_send.send(()).unwrap();
                    // Leave the real write transaction open when returning. Once
                    // the last opaque connection Arc is destroyed, it must roll back.
                    let _ = sql_release_recv.recv();
                    Ok(())
                });
            tokio::time::timeout(Duration::from_secs(3), queued_recv)
                .await
                .unwrap()
                .unwrap();
            // The actual mailbox insertion point fired while its native worker
            // is still gated in dummy. This is truly queued, not only registered.
            assert!(matches!(
                sql_started_recv.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            (lease, worker, sql, dummy)
        });
        drop(sql);
        drop(dummy);
        {
            let _entered = runtime.enter();
            drop(lease); // Queues its real cleanup, but this runtime is not driven again.
        }
        drop(runtime);
        let state = database.state();
        assert_eq!((state.connections, state.idle_connections), (0, 0));
        assert_eq!(
            state.admitted_operations, 1,
            "queued real SQL must keep its acquisition"
        );
        assert!(matches!(
            released_recv.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        dummy_release_send.send(()).unwrap();
        sql_started_recv
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(
            database.state().admitted_operations,
            1,
            "running real SQL still owns admission after runtime teardown"
        );
        let mut peer = SqliteConnection::establish(url.to_str().unwrap()).unwrap();
        peer.batch_execute("PRAGMA busy_timeout=0;").unwrap();
        let locked = peer
            .batch_execute("BEGIN IMMEDIATE; ROLLBACK;")
            .expect_err("positive control: the real queued SQL now holds a writer lock");
        assert!(locked.to_string().contains("locked"), "{locked}");
        sql_release_send.send(()).unwrap();
        // Actual release callback completion, after opaque closure destruction;
        // no new runtime/checkout/timer or SQL retry can hide an idle writer.
        released_recv.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(database.state().admitted_operations, 0);
        peer.batch_execute("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        let rows = diesel::sql_query("SELECT COUNT(*) AS count FROM native_lifecycle_probe")
            .get_result::<Count>(&mut peer)
            .unwrap();
        assert_eq!(rows.count, 0, "abandoned write must roll back");
        drop(peer);
        drop(database);
        directory.close().unwrap();
        drop(worker);
        stopped_recv.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(
            released_recv.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }
}
