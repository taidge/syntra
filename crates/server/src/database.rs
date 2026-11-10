//! Short query leases backed by Horme and Diesel-async's SQLite adapter.
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
use diesel::connection::{
    Instrumentation, InstrumentationEvent, TransactionManager, TransactionManagerStatus,
};
use diesel::result::{Error, QueryResult};
use diesel::{Connection, SqliteConnection};
use diesel_async::pooled_connection::deadpool::{Object, Pool};
use diesel_async::pooled_connection::{
    AsyncDieselConnectionManager, ManagerConfig, RecyclingMethod,
};
use diesel_async::sync_connection_wrapper::{SpawnBlocking, SyncConnectionWrapper};
use futures_util::future::BoxFuture;
use horme::GetConnection;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

use crate::middleware::request_observability::{
    WorkerPhase, current_database_observation, current_database_operation,
    observe_database_operation, observe_statement, with_database_observation,
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
pub struct SqliteRuntime {
    tail: Option<oneshot::Receiver<()>>,
    operation_timeout: Duration,
}
impl SqliteRuntime {
    pub(crate) fn new(operation_timeout: Duration) -> Self {
        Self {
            tail: None,
            operation_timeout,
        }
    }
}

#[derive(Default)]
struct SqliteObservation {
    query_started: Option<Instant>,
}

impl Instrumentation for SqliteObservation {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        observe_statement(|observation, operation| match event {
            InstrumentationEvent::StartQuery { .. } => {
                self.query_started = Some(Instant::now());
                observation
                    .statements_started
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(operation) = operation {
                    operation.statements_started.fetch_add(1, Ordering::Relaxed);
                }
            }
            InstrumentationEvent::FinishQuery { error, .. } => {
                if let Some(started) = self.query_started.take() {
                    observation
                        .sql_run_us
                        .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                    observation
                        .statements_completed
                        .fetch_add(1, Ordering::Relaxed);
                    if let Some(operation) = operation {
                        operation
                            .statements_completed
                            .fetch_add(1, Ordering::Release);
                    }
                }
                if let Some(error) = error {
                    observation.record_pressure(&AppError::Database(error.to_string()));
                }
            }
            _ => {}
        });
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
        let worker = tokio::spawn(async move {
            if let Some(predecessor) = predecessor {
                if let Some(progress) = &worker_operation {
                    progress.set_phase(WorkerPhase::WaitingPredecessor);
                }
                let _ = predecessor.await;
            }
            if let Some(progress) = &worker_operation {
                progress.set_phase(WorkerPhase::BlockingQueue);
            }
            let result = tokio::task::spawn_blocking(move || {
                if let Some(progress) = &worker_operation {
                    progress.set_phase(WorkerPhase::Running);
                }
                let result =
                    with_database_observation(worker_observation, worker_operation.clone(), task);
                if let Some(progress) = &worker_operation {
                    progress.set_phase(WorkerPhase::Returning);
                }
                result
            })
            .await;
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
            result
                .map_err(|error| -> BoxError { Box::new(error) })?
                .map_err(|error| -> BoxError { Box::new(error) })
        })
    }
}

struct Inner {
    provider: horme::Db<Pool<DbConnection>>,
    admission: Arc<Semaphore>,
    checkout_timeout: Duration,
    operation_timeout: Duration,
    admitted: AtomicUsize,
    checkouts: AtomicU64,
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
    connection: Option<Object<DbConnection>>,
    admission: Option<Admission>,
}
impl Deref for DatabaseLease {
    type Target = DbConnection;
    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect("live database lease")
    }
}
impl DerefMut for DatabaseLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect("live database lease")
    }
}
impl Drop for DatabaseLease {
    fn drop(&mut self) {
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        let admission = self.admission.take();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            drop(Object::take(connection));
            drop(admission);
            return;
        };
        // This barrier is queued after every prior operation on this wrapper.
        // Its closure retains admission even if the cleanup future times out.
        let cleanup = connection.spawn_blocking(move |connection| {
            let _admission = admission;
            let status = <SqliteConnection as Connection>::TransactionManager::transaction_manager_status_mut(connection);
            if matches!(status, TransactionManagerStatus::Valid(valid) if valid.transaction_depth().is_none()) {
                Ok(())
            } else {
                // Closing an unfinished SQLite transaction rolls it back.
                Err(Error::RollbackTransaction)
            }
        });
        runtime.spawn(async move {
            if cleanup.await.is_err() {
                drop(Object::take(connection));
            } else {
                drop(connection);
            }
        });
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
        let mut manager_config = ManagerConfig::default();
        manager_config.recycling_method = RecyclingMethod::Fast;
        #[cfg(test)]
        let setup_activity = Arc::new(SetupActivity::default());
        #[cfg(test)]
        let manager_activity = setup_activity.clone();
        manager_config.custom_setup = Box::new(move |url| {
            let url = url.to_owned();
            let busy_setting = busy_setting.clone();
            let progress = STARTUP_CHECKOUT_PROGRESS.try_with(Arc::clone).ok();
            #[cfg(test)]
            let setup_activity = manager_activity.clone();
            Box::pin(async move {
                let advance = move |stage| {
                    if let Some(progress) = &progress {
                        progress.advance(stage);
                    }
                };
                advance("blocking worker queue");
                let connection = tokio::task::spawn_blocking(move || {
                    use diesel::connection::SimpleConnection;
                    #[cfg(test)]
                    let _worker = {
                        let active = setup_activity.active.fetch_add(1, Ordering::SeqCst) + 1;
                        setup_activity.peak.fetch_max(active, Ordering::SeqCst);
                        setup_activity.started.fetch_add(1, Ordering::SeqCst);
                        SetupWorker(setup_activity)
                    };
                    advance("SQLite open");
                    let mut connection = SqliteConnection::establish(&url)?;
                    for (stage, sql) in [
                        ("SQLite busy timeout", busy_setting.as_str()),
                        ("SQLite WAL setup", "PRAGMA journal_mode=WAL;"),
                        ("SQLite synchronous setup", "PRAGMA synchronous=NORMAL;"),
                    ] {
                        advance(stage);
                        connection.batch_execute(sql).map_err(|error| {
                            diesel::ConnectionError::BadConnection(error.to_string())
                        })?;
                    }
                    advance("pool checkout completion");
                    connection.set_instrumentation(SqliteObservation::default());
                    Ok::<_, diesel::ConnectionError>(connection)
                })
                .await
                .map_err(|error| diesel::ConnectionError::BadConnection(error.to_string()))??;
                Ok(SyncConnectionWrapper::with_runtime(
                    connection,
                    SqliteRuntime::new(operation_timeout),
                ))
            })
        });
        let manager =
            AsyncDieselConnectionManager::<DbConnection>::new_with_config(url, manager_config);
        let pool = Pool::builder(manager)
            .max_size(capacity)
            .runtime(deadpool::Runtime::Tokio1)
            .build()
            .map_err(|error| failure(error.to_string()))?;
        Ok(Self(Arc::new(Inner {
            provider: horme::Db::new(pool),
            admission: Arc::new(Semaphore::new(admitted)),
            checkout_timeout,
            operation_timeout,
            admitted: AtomicUsize::new(0),
            checkouts: AtomicU64::new(0),
            #[cfg(test)]
            setup_activity,
        })))
    }

    /// Compatibility with old call sites: this only clones a provider, never a lease.
    pub fn get(&self) -> QueryResult<Self> {
        Ok(self.clone())
    }

    pub fn state(&self) -> DatabaseStatus {
        let status = self.0.provider.pool().status();
        DatabaseStatus {
            connections: status.size as u32,
            idle_connections: status.available as u32,
            admitted_operations: self.0.admitted.load(Ordering::Relaxed),
            checkouts: self.0.checkouts.load(Ordering::Relaxed),
        }
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
        observe_database_operation(|| {
            connection.spawn_blocking(move |connection| Ok(callback(connection)))
        })
        .await
        .map_err(E::from)?
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
        let permit = self
            .0
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| horme::Error::from(failure("database admission capacity exceeded")))?;
        self.0.admitted.fetch_add(1, Ordering::Relaxed);
        let admission = Admission {
            _permit: permit,
            inner: self.0.clone(),
        };
        let inner = self.0.clone();
        // Cancellation only detaches this owner. In particular, an in-progress
        // SQLite setup must retain both deadpool's slot and our admission until
        // its blocking worker finishes. A late lease follows the usual cleanup.
        let checkout = tokio::spawn(async move {
            let mut provider = inner.provider.clone();
            let acquisition = provider.get_mut_conn();
            let connection = match startup {
                Some(progress) => STARTUP_CHECKOUT_PROGRESS.scope(progress, acquisition).await,
                None => acquisition.await,
            }?;
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
        tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap();
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
            while database.0.provider.pool().status().waiting != 2 {
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
