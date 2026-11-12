//! Query-affinity experiment: one native job slot and one admission-release slot.
//! SQLite closes on the previous owner thread; admission waits for all jobs.
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

type BoxError = Box<dyn Error + Send + Sync>;
type Callback = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug)]
enum WorkerFailure {
    Panicked,
    Closed,
    ResponseLost,
}
impl fmt::Display for WorkerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Panicked => "SQLite worker callback panicked",
            Self::Closed => "SQLite worker mailbox closed",
            Self::ResponseLost => "SQLite worker response lost",
        })
    }
}
impl Error for WorkerFailure {}

struct Mailbox {
    ordinary: Option<QueuedJob>,
    release_after_jobs: Option<Callback>,
    outstanding: usize,
    owner_gone: bool,
}
struct State {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    slot: Arc<Semaphore>,
    #[cfg(test)]
    signals: Mutex<TestSignals>,
}
#[cfg(test)]
#[derive(Default)]
struct TestSignals {
    queued: Option<oneshot::Sender<()>>,
    released: Option<std::sync::mpsc::Sender<()>>,
    stopped: Option<std::sync::mpsc::Sender<()>>,
}
#[cfg(test)]
impl State {
    fn queued_for_test(&self) {
        let signal = self.signals.lock().unwrap().queued.take();
        if let Some(signal) = signal {
            let _ = signal.send(());
        }
    }
    fn released_for_test(&self) {
        let signal = self.signals.lock().unwrap().released.take();
        if let Some(signal) = signal {
            let _ = signal.send(());
        }
    }
    fn stopped_for_test(&self) {
        let signal = self.signals.lock().unwrap().stopped.take();
        if let Some(signal) = signal {
            let _ = signal.send(());
        }
    }
}
struct Control {
    state: Arc<State>,
}
impl Drop for Control {
    fn drop(&mut self) {
        // The native thread owns State, not Control, so it does not count as an
        // external owner and cannot keep its own lifetime alive forever.
        self.state.mailbox.lock().unwrap().owner_gone = true;
        self.state.wake.notify_one();
    }
}

#[derive(Clone)]
pub(super) struct WorkerHandle(Arc<Control>);

struct Outstanding(Arc<State>);
impl Drop for Outstanding {
    fn drop(&mut self) {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        mailbox.outstanding = mailbox
            .outstanding
            .checked_sub(1)
            .expect("one completion per registered SQLite job");
        let release = if mailbox.outstanding == 0 {
            mailbox.release_after_jobs.take()
        } else {
            None
        };
        drop(mailbox);
        self.0.wake.notify_one();
        // OwnedJob destroys its opaque callback/inner Arc before this guard.
        // No mailbox/pool lock may remain held during recursive Admission Drop.
        if let Some(release) = release {
            release();
            #[cfg(test)]
            self.0.released_for_test();
        }
    }
}
struct OwnedJob {
    // Field order is intentional: an unqueued callback and its private inner
    // connection Arc must be destroyed BEFORE the outstanding guard decrements.
    callback: Option<Callback>,
    _outstanding: Outstanding,
}
impl OwnedJob {
    fn run(mut self) {
        if let Some(callback) = self.callback.take() {
            // The typed callback already catches a user panic. This outer guard
            // also keeps a response-value destructor panic from losing the
            // worker or its final admission-release path.
            let _ = catch_unwind(AssertUnwindSafe(callback));
        }
    }
}
struct QueuedJob {
    job: OwnedJob,
    permit: OwnedSemaphorePermit,
}

pub(super) struct Prepared<R> {
    job: Option<OwnedJob>,
    result: oneshot::Receiver<Result<R, WorkerFailure>>,
    handle: WorkerHandle,
}
impl<R: Send + 'static> Prepared<R> {
    pub(super) async fn run(mut self) -> Result<R, BoxError> {
        let permit = self
            .handle
            .0
            .state
            .slot
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| -> BoxError { Box::new(WorkerFailure::Closed) })?;
        let job = self
            .job
            .take()
            .expect("prepared SQLite job is submitted once");
        {
            let mut mailbox = self.handle.0.state.mailbox.lock().unwrap();
            debug_assert!(mailbox.ordinary.is_none());
            mailbox.ordinary = Some(QueuedJob { job, permit });
        }
        #[cfg(test)]
        self.handle.0.state.queued_for_test();
        self.handle.0.state.wake.notify_one();
        self.result
            .await
            .map_err(|_| -> BoxError { Box::new(WorkerFailure::ResponseLost) })?
            .map_err(|error| -> BoxError { Box::new(error) })
    }
}

impl WorkerHandle {
    pub(super) fn start() -> std::io::Result<Self> {
        let state = Arc::new(State {
            mailbox: Mutex::new(Mailbox {
                ordinary: None,
                release_after_jobs: None,
                outstanding: 0,
                owner_gone: false,
            }),
            wake: Condvar::new(),
            slot: Arc::new(Semaphore::new(1)),
            #[cfg(test)]
            signals: Mutex::new(TestSignals::default()),
        });
        let control = Arc::new(Control {
            state: state.clone(),
        });
        // Detach the JoinHandle. Native worker drain has an explicit terminal
        // condition; caller/worker Drop never joins or blocks a Tokio executor.
        drop(
            std::thread::Builder::new()
                .name("sqlite-connection".into())
                .spawn(move || run(state))?,
        );
        Ok(Self(control))
    }

    pub(super) fn prepare<R: Send + 'static>(
        &self,
        callback: impl FnOnce() -> R + Send + 'static,
    ) -> Prepared<R> {
        // Register before predecessor/mailbox awaits. Even a future never
        // enqueued must hold admission until its callback is destroyed.
        {
            let mut mailbox = self.0.state.mailbox.lock().unwrap();
            mailbox.outstanding = mailbox
                .outstanding
                .checked_add(1)
                .expect("SQLite outstanding count overflow");
        }
        let outstanding = Outstanding(self.0.state.clone());
        let (send, result) = oneshot::channel();
        let callback = Box::new(move || {
            let outcome =
                catch_unwind(AssertUnwindSafe(callback)).map_err(|_| WorkerFailure::Panicked);
            // If its receiver was canceled, R is destroyed here, on this worker.
            // ManagedSqlite also handles a later receiver-side cancellation.
            let _ = send.send(outcome);
        });
        Prepared {
            job: Some(OwnedJob {
                callback: Some(callback),
                _outstanding: outstanding,
            }),
            result,
            handle: self.clone(),
        }
    }

    /// Query-only policy: this payload is an Admission anchor, NEVER a
    /// physical connection. ManagedSqlite's unique Drop registers it once.
    /// Final release has no affinity and must not wait for native scheduling.
    pub(super) fn release_after_jobs<T: Send + 'static>(&self, owned: T) {
        let mut release: Option<Callback> = Some(Box::new(move || drop(owned)));
        {
            let mut mailbox = self.0.state.mailbox.lock().unwrap();
            debug_assert!(mailbox.release_after_jobs.is_none());
            if mailbox.outstanding != 0 {
                mailbox.release_after_jobs = release.take();
            }
        }
        // A concurrent final Outstanding drop either takes the registered
        // callback or precedes this zero test. Exactly one side owns it.
        if let Some(release) = release {
            release();
            #[cfg(test)]
            self.0.state.released_for_test();
        }
    }

    #[cfg(test)]
    pub(super) fn test_on_queued(&self, signal: oneshot::Sender<()>) {
        self.0.state.signals.lock().unwrap().queued = Some(signal);
    }
    #[cfg(test)]
    pub(super) fn test_on_release(&self, signal: std::sync::mpsc::Sender<()>) {
        self.0.state.signals.lock().unwrap().released = Some(signal);
    }
    #[cfg(test)]
    pub(super) fn test_on_stopped(&self, signal: std::sync::mpsc::Sender<()>) {
        self.0.state.signals.lock().unwrap().stopped = Some(signal);
    }
}

enum Next {
    Job(QueuedJob),
    Stop,
}
fn run(state: Arc<State>) {
    loop {
        let next = {
            let mut mailbox = state.mailbox.lock().unwrap();
            loop {
                if let Some(job) = mailbox.ordinary.take() {
                    break Next::Job(job);
                }
                if mailbox.outstanding == 0 && mailbox.owner_gone {
                    debug_assert!(mailbox.release_after_jobs.is_none());
                    break Next::Stop;
                }
                mailbox = state.wake.wait(mailbox).unwrap();
            }
        };
        match next {
            Next::Job(QueuedJob { job, permit }) => {
                drop(permit);
                // Covers Outstanding's lock-free release too, if an anchor's
                // recursive destruction unexpectedly unwinds. Never lose the
                // native worker merely because response/anchor Drop panicked.
                let _ = catch_unwind(AssertUnwindSafe(|| job.run()));
            }
            Next::Stop => {
                state.slot.close();
                #[cfg(test)]
                state.stopped_for_test();
                break;
            }
        }
    }
}
