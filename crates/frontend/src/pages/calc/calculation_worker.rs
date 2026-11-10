use js_sys::{Array, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, MessageEvent, Worker, WorkerOptions, WorkerType};

use super::{AuthState, Cage, Editor, Rc, RefCell, Scope, Timeout, calculation};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Progress {
    Idle,
    Pending,
    Ready,
    Cancelled,
    Failed,
    TimedOut,
}

impl Progress {
    pub(super) fn allows_raw_save(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::TimedOut)
    }

    pub(super) fn message(self) -> String {
        let key = match self {
            Self::Pending => "calc.formula_pending",
            Self::Cancelled => "calc.formula_cancelled",
            Self::Failed => "calc.formula_unavailable",
            Self::TimedOut => "calc.formula_timeout",
            Self::Idle | Self::Ready => return String::new(),
        };
        crate::i18n::t(key)
    }
}

struct WorkerHandle {
    id: u32,
    worker: Worker,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(Event)>,
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onmessageerror(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

struct Job {
    id: u32,
    revision: u64,
    _deadline: Timeout,
}

#[derive(Default)]
struct State {
    scheduled: Option<Timeout>,
    worker: Option<WorkerHandle>,
    active: Option<Job>,
    last_revision: Option<u64>,
    serial: u32,
}

impl State {
    fn stop_busy(&mut self) {
        self.scheduled.take();
        if self.active.take().is_some() {
            self.worker.take();
        }
    }
}

pub(super) struct Controller {
    scope: glory::ui::WeakScope,
    editor: Editor,
    auth: Cage<AuthState>,
    token: String,
    state: RefCell<State>,
    pub(super) progress: Cage<(u64, Progress)>,
}

impl Controller {
    pub(super) fn install(
        scope: &Scope,
        editor: Editor,
        loaded: Cage<bool>,
        auth: Cage<AuthState>,
        token: String,
    ) -> Rc<Self> {
        let controller = Rc::new(Self {
            scope: scope.downgrade(),
            editor,
            auth,
            token,
            state: RefCell::new(State::default()),
            progress: scope.cage((0, Progress::Idle)),
        });
        scope.effect({
            let controller = controller.clone();
            move || {
                if !auth.with(|state| state.token.as_deref() == Some(controller.token.as_str())) {
                    let mut state = controller.state.borrow_mut();
                    state.stop_busy();
                    state.worker.take();
                    state.last_revision = None;
                    return;
                }
                if !loaded.get() {
                    return;
                }
                let revision = editor.book.with(|book| book.revision);
                if controller.state.borrow().last_revision != Some(revision) {
                    controller.schedule(revision);
                }
            }
        });
        let _ = scope.on_cleanup({
            let controller = controller.clone();
            move || *controller.state.borrow_mut() = State::default()
        });
        controller
    }

    fn is_current(&self, revision: u64) -> bool {
        self.scope.upgrade().is_some()
            && self
                .auth
                .with_untracked(|state| state.token.as_deref() == Some(self.token.as_str()))
            && self
                .editor
                .book
                .with_untracked(|book| book.revision == revision)
    }

    pub(super) fn cancel(&self) {
        let (revision, progress) = self.progress.get_untracked();
        if progress == Progress::Pending && self.is_current(revision) {
            self.state.borrow_mut().stop_busy();
            self.progress
                .revise(|state| *state = (revision, Progress::Cancelled));
        }
    }

    pub(super) fn retry(self: &Rc<Self>) {
        let (revision, progress) = self.progress.get_untracked();
        if progress.allows_raw_save() && self.is_current(revision) {
            self.schedule(revision);
        }
    }

    fn schedule(self: &Rc<Self>, revision: u64) {
        let needs_worker = self
            .editor
            .book
            .with_untracked(|book| book.pending_calculation().is_some());
        {
            let mut state = self.state.borrow_mut();
            state.stop_busy();
            state.last_revision = Some(revision);
            if needs_worker {
                let weak = Rc::downgrade(self);
                state.scheduled = Some(Timeout::new(100, move || {
                    if let Some(controller) = weak.upgrade() {
                        controller.start(revision);
                    }
                }));
            }
        }
        let progress = if needs_worker {
            Progress::Pending
        } else {
            Progress::Idle
        };
        self.progress.revise(|state| *state = (revision, progress));
    }

    fn create_worker(self: &Rc<Self>, id: u32) -> Result<WorkerHandle, JsValue> {
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options("/assets/calc-worker.js", &options)?;
        let weak = Rc::downgrade(self);
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(controller) = weak.upgrade() {
                controller.on_message(id, event.data());
            }
        });
        let weak = Rc::downgrade(self);
        let error = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            event.prevent_default();
            if let Some(controller) = weak.upgrade() {
                controller.on_error(id);
            }
        });
        worker.set_onmessage(Some(message.as_ref().unchecked_ref()));
        worker.set_onmessageerror(Some(error.as_ref().unchecked_ref()));
        worker.set_onerror(Some(error.as_ref().unchecked_ref()));
        Ok(WorkerHandle {
            id,
            worker,
            _message: message,
            _error: error,
        })
    }

    fn start(self: &Rc<Self>, revision: u64) {
        if !self.is_current(revision) {
            return;
        }
        self.state.borrow_mut().scheduled.take();
        let input = self
            .editor
            .book
            .with_untracked(|book| book.pending_calculation());
        let Some(bytes) = input.and_then(|input| calculation::encode(input.as_ref()).ok()) else {
            self.progress
                .revise(|state| *state = (revision, Progress::Failed));
            return;
        };
        if self.send(revision, &bytes).is_err() {
            let mut state = self.state.borrow_mut();
            state.active.take();
            state.worker.take();
            drop(state);
            self.progress
                .revise(|state| *state = (revision, Progress::Failed));
        }
    }

    fn send(self: &Rc<Self>, revision: u64, bytes: &[u8]) -> Result<(), JsValue> {
        let mut state = self.state.borrow_mut();
        state.serial = state.serial.wrapping_add(1);
        let id = state.serial;
        if state.worker.is_none() {
            state.worker = Some(self.create_worker(id)?);
        }
        let data = Uint8Array::from(bytes);
        let message = Array::of2(&JsValue::from(id), data.as_ref());
        let transfer = Array::of1(data.buffer().as_ref());
        state
            .worker
            .as_ref()
            .expect("created worker")
            .worker
            .post_message_with_transfer(message.as_ref(), transfer.as_ref())?;
        let weak = Rc::downgrade(self);
        state.active = Some(Job {
            id,
            revision,
            _deadline: Timeout::new(10_000, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.finish(id, Progress::TimedOut, None);
                }
            }),
        });
        Ok(())
    }

    fn on_message(&self, worker: u32, data: JsValue) {
        let Some(id) = self.active_id(worker) else {
            return;
        };
        if !Array::is_array(&data) {
            self.finish(id, Progress::Failed, None);
            return;
        }
        let response: Array = data.unchecked_into();
        if response.get(0).as_f64() != Some(f64::from(id)) {
            return;
        }
        let results = response
            .get(1)
            .dyn_into::<Uint8Array>()
            .ok()
            .filter(|bytes| bytes.length() as usize <= calculation::MAX_WIRE_BYTES)
            .and_then(|bytes| calculation::decode(&bytes.to_vec()));
        self.finish(id, Progress::Failed, results);
    }

    fn on_error(&self, worker: u32) {
        if let Some(id) = self.active_id(worker) {
            self.finish(id, Progress::Failed, None);
        }
    }

    fn active_id(&self, worker: u32) -> Option<u32> {
        let state = self.state.borrow();
        if state.worker.as_ref()?.id != worker {
            return None;
        }
        Some(state.active.as_ref()?.id)
    }

    fn finish(&self, id: u32, fallback: Progress, values: Option<calculation::FormulaResults>) {
        let revision = {
            let mut state = self.state.borrow_mut();
            let Some(job) = state.active.as_ref().filter(|job| job.id == id) else {
                return;
            };
            let revision = job.revision;
            state.active.take();
            if values.is_none() {
                state.worker.take();
            }
            revision
        };
        if self.is_current(revision) {
            let progress = if values.is_some_and(|values| {
                self.editor
                    .book
                    .with_untracked(|book| book.accept_calculation(revision, values))
            }) {
                self.editor.calculation_epoch.revise(|epoch| *epoch += 1);
                Progress::Ready
            } else {
                fallback
            };
            self.progress.revise(|state| *state = (revision, progress));
        }
    }
}
