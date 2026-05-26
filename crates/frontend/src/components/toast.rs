use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub kind: ToastKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

pub fn push_toast(mut bus: Signal<Vec<Toast>>, kind: ToastKind, message: impl Into<String>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let toast = Toast {
        id,
        message: message.into(),
        kind,
    };
    bus.write().push(toast);
    let mut bus_for_timeout = bus;
    let t = Timeout::new(4000, move || {
        bus_for_timeout.write().retain(|t| t.id != id);
    });
    t.forget();
}

#[component]
pub fn ToastRack() -> Element {
    let mut bus = use_context::<Signal<Vec<Toast>>>();
    rsx! {
        div { class: "toast-rack",
            for t in bus.read().iter().cloned() {
                {
                    let kind = match t.kind {
                        ToastKind::Info => "toast toast-info",
                        ToastKind::Success => "toast toast-success",
                        ToastKind::Warning => "toast toast-warning",
                        ToastKind::Error => "toast toast-error",
                    };
                    let id = t.id;
                    rsx! {
                        div { class: "{kind}", key: "{id}",
                            span { class: "toast-msg", "{t.message}" }
                            button {
                                class: "toast-close",
                                onclick: move |_| bus.write().retain(|x| x.id != id),
                                "\u{2715}"
                            }
                        }
                    }
                }
            }
        }
    }
}
