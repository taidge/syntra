//! Browser gestures which Glory's semantic event vocabulary does not expose.
//! Keep these listeners scoped to Calc and dispose them with its owning scope.
use super::*;
use wasm_bindgen::{JsCast, closure::Closure};

fn focus_cell(point: (u32, u32)) {
    if let Some(document) = web_sys::window().and_then(|w| w.document())
        && let Ok(Some(element)) = document.query_selector(&format!(
            ".calc-editor .cell-button[aria-label='{}']",
            cell_ref(point.0, point.1)
        ))
        && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
    {
        let _ = element.focus();
    }
}
fn point(element: &web_sys::Element) -> Option<(u32, u32)> {
    parse_cell_ref(&element.get_attribute("aria-label")?)
}
pub(super) fn install(scope: &Scope, ed: Editor, save: Rc<dyn Fn()>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let weak = scope.downgrade();
    let double = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(element) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let Some(target) = element
            .closest(".calc-editor .cell-button")
            .ok()
            .flatten()
            .and_then(|e| point(&e))
        else {
            return;
        };
        event.prevent_default();
        let weak = weak.clone();
        // The preceding second click still has a queued semantic Activate.
        // Let that selection finish before opening its editing session.
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if weak.upgrade().is_some() {
                ed.begin_edit(target, None);
            }
        });
    });
    let weak = scope.downgrade();
    let focus_document = document.clone();
    let key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if (event.ctrl_key() || event.meta_key())
                && event.key().eq_ignore_ascii_case("s")
                && let Some(element) = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                && element.closest(".calc-editor").ok().flatten().is_some()
            {
                event.prevent_default();
                event.stop_propagation();
                if let Some(input) = element.dyn_ref::<web_sys::HtmlInputElement>() {
                    ed.formula.revise(|v| *v = input.value());
                    if ed.editing.get().is_none()
                        && element.get_attribute("aria-label").as_deref() == Some("Formula")
                    {
                        let (si, (r, c)) = (ed.sheet.get(), ed.active.get());
                        if !ed.commit_at((si, r, c), input.value()) {
                            return;
                        }
                    }
                }
                if ed.finish_edit(false) {
                    save();
                }
                return;
            }
            if event.is_composing() || event.ctrl_key() || event.meta_key() || event.alt_key() {
                return;
            }
            let Some(element) = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            else {
                return;
            };
            let editing = element
                .matches(".calc-editor .calc-cell-input")
                .unwrap_or(false);
            let target = element
                .closest(".calc-editor .cell-button")
                .ok()
                .flatten()
                .and_then(|e| point(&e));
            let name = event.key();
            let pending_edit =
                target.is_some_and(|(r, c)| ed.editing.get() == Some((ed.sheet.get(), r, c)));
            let finishing =
                editing || pending_edit && matches!(name.as_str(), "Enter" | "Tab" | "Escape");
            let handled = if editing {
                matches!(name.as_str(), "Enter" | "Tab" | "Escape")
            } else {
                target.is_some()
                    && (matches!(
                        name.as_str(),
                        "F2" | "Enter" | "Tab" | "Delete" | "Backspace"
                    ) || shift_arrow(&event)
                        || pending_edit && name == "Escape"
                        || name.chars().count() == 1)
            };
            if !handled {
                return;
            }
            event.prevent_default();
            event.stop_propagation();
            let draft = element
                .dyn_ref::<web_sys::HtmlInputElement>()
                .map(|e| e.value());
            let shift = event.shift_key();
            if weak.upgrade().is_none() {
                return;
            }
            // Record cell keystrokes before the next render. Deferring them
            // lets a newly focused input overtake characters still buffered
            // on its former button and can silently drop or reorder text.
            if finishing {
                if let Some(value) = draft {
                    ed.formula.revise(|v| *v = value);
                }
                if !ed.finish_edit(name == "Escape") {
                    return;
                }
                match name.as_str() {
                    "Enter" => ed.step(if shift { -1 } else { 1 }, 0, false),
                    "Tab" => ed.step(0, if shift { -1 } else { 1 }, false),
                    _ => ed.activate(ed.active.get()),
                }
            } else if let Some(target) = target {
                match name.as_str() {
                    "Tab" => ed.step(0, if shift { -1 } else { 1 }, false),
                    "ArrowLeft" => ed.step(0, -1, true),
                    "ArrowRight" => ed.step(0, 1, true),
                    "ArrowUp" => ed.step(-1, 0, true),
                    "ArrowDown" => ed.step(1, 0, true),
                    "Delete" | "Backspace" => {
                        // Shift navigation updates the logical selection before
                        // its deferred DOM focus. A focused cell still in that
                        // selection must not collapse the range when clearing.
                        if !in_selection(target, ed.anchor.get(), ed.end.get())
                            && !ed.extra.with(|cells| cells.contains(&target))
                        {
                            ed.activate(target);
                        }
                        let cells = ed.selected();
                        ed.book.revise(|b| {
                            b.checkpoint();
                            for (r, c) in cells {
                                b.snapshot.cells.remove(&(ed.sheet.get(), r, c));
                            }
                        });
                        ed.formula.revise(String::clear);
                    }
                    _ => {
                        if name.chars().count() == 1 && ed.editing.get().is_some() {
                            ed.formula.revise(|v| v.push_str(&name));
                        } else {
                            ed.begin_edit(
                                target,
                                if name == "F2" || name == "Enter" {
                                    None
                                } else {
                                    Some(name.clone())
                                },
                            );
                        }
                    }
                }
            }
            let weak = weak.clone();
            let focus_document = focus_document.clone();
            let focus_origin = focus_document.active_element();
            let focus_point = ed.active.get();
            let focus_sheet = ed.sheet.get();
            wasm_bindgen_futures::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(20).await;
                if weak.upgrade().is_none()
                    || ed.editing.get().is_some()
                    || ed.active.get() != focus_point
                    || ed.sheet.get() != focus_sheet
                {
                    return;
                }
                let focused = focus_document.active_element();
                let origin_retired = focus_origin
                    .as_ref()
                    .is_some_and(|origin| !origin.is_connected())
                    && focused == focus_document.body().map(Into::into);
                // A later selection or another focused control owns its focus.
                // A retired inline input can hand focus back through body.
                if focused == focus_origin || origin_retired {
                    focus_cell(focus_point);
                }
            });
        });
    let _ = document.add_event_listener_with_callback("dblclick", double.as_ref().unchecked_ref());
    let _ = document.add_event_listener_with_callback_and_bool(
        "keydown",
        key.as_ref().unchecked_ref(),
        true,
    );
    let _ = scope.on_cleanup(move || {
        let _ = document
            .remove_event_listener_with_callback("dblclick", double.as_ref().unchecked_ref());
        let _ = document.remove_event_listener_with_callback_and_bool(
            "keydown",
            key.as_ref().unchecked_ref(),
            true,
        );
    });
}

fn shift_arrow(event: &web_sys::KeyboardEvent) -> bool {
    event.shift_key()
        && matches!(
            event.key().as_str(),
            "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown"
        )
}
