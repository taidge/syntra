use crate::components::ui::*;
use glory::prelude::*;
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
pub fn push_toast(bus: Cage<Vec<Toast>>, kind: ToastKind, message: impl Into<String>) {
    let toast = Toast {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        message: message.into(),
        kind,
    };
    let _ = bus.try_revise(|items| items.push(toast));
}
#[allow(non_snake_case)]
pub fn ToastRack(_scope: Scope, bus: Cage<Vec<Toast>>) -> impl Widget {
    col().class("toast-rack").fill(each(
        bus,
        |toast| toast.id,
        move |item, _| {
            widget(move |scope| {
                let toast = item.get();
                let id = toast.id;
                #[cfg(target_arch = "wasm32")]
                {
                    let timeout = gloo_timers::callback::Timeout::new(4_000, move || {
                        let _ = bus.try_revise(|items| items.retain(|toast| toast.id != id));
                    });
                    scope.retain(timeout)?;
                }
                let class = match toast.kind {
                    ToastKind::Info => "toast toast-info",
                    ToastKind::Success => "toast toast-success",
                    ToastKind::Warning => "toast toast-warning",
                    ToastKind::Error => "toast toast-error",
                };
                row()
                    .class(class)
                    .fill((
                        text().content_fn(move || item.get().message),
                        action("Dismiss", move || {
                            let _ = bus.try_revise(|items| items.retain(|toast| toast.id != id));
                        }),
                    ))
                    .build(scope)
            })
        },
    ))
}
