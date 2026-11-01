//! Native pointer gestures over the semantic cells; all data changes remain in Editor.
use super::*;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, closure::Closure};

enum Gesture {
    Selection {
        start: (u32, u32),
        axis: Option<bool>,
        origin: (i32, i32),
    },
    Fill {
        source: MergedRange,
        extent: MergedRange,
    },
    Resize {
        column: bool,
        index: u32,
        start: i32,
        original: f64,
        size: f64,
    },
}
type Drag = Rc<RefCell<Option<Gesture>>>;
fn closest(element: &web_sys::Element, selector: &str) -> Option<web_sys::Element> {
    element.closest(selector).ok().flatten()
}
fn cell_point(element: &web_sys::Element) -> Option<(u32, u32)> {
    let cell = closest(element, ".calc-editor .cell")?;
    let button = cell.query_selector(".cell-button").ok().flatten()?;
    parse_cell_ref(&button.get_attribute("aria-label")?)
}
fn axis_index(element: &web_sys::Element, prefix: &str, column: bool) -> Option<u32> {
    let name = element.get_attribute("aria-label")?;
    let value = name.strip_prefix(prefix)?;
    if column {
        parse_cell_ref(&format!("{value}1")).map(|p| p.1)
    } else {
        value.parse::<u32>().ok()?.checked_sub(1)
    }
}
fn focus(element: &web_sys::Element) {
    if let Some(element) = element.dyn_ref::<web_sys::HtmlElement>() {
        let _ = element.focus();
    }
}
fn extent(source: MergedRange, point: (u32, u32)) -> MergedRange {
    let vertical = if point.0 < source.start.0 {
        source.start.0 - point.0
    } else {
        point.0.saturating_sub(source.end.0)
    };
    let horizontal = if point.1 < source.start.1 {
        source.start.1 - point.1
    } else {
        point.1.saturating_sub(source.end.1)
    };
    if vertical >= horizontal {
        MergedRange {
            start: (point.0.min(source.start.0), source.start.1),
            end: (point.0.max(source.end.0), source.end.1),
        }
    } else {
        MergedRange {
            start: (source.start.0, point.1.min(source.start.1)),
            end: (source.end.0, point.1.max(source.end.1)),
        }
    }
}
fn update(document: &web_sys::Document, ed: Editor, gesture_state: &Drag, x: i32, y: i32) {
    let mut state = gesture_state.borrow_mut();
    match state.as_mut() {
        Some(Gesture::Resize {
            column,
            index,
            start,
            original,
            size,
        }) => {
            *size = (*original + ((if *column { x } else { y }) - *start) as f64)
                .clamp(24., if *column { 800. } else { 400. });
            ed.resize_preview
                .revise(|v| *v = Some((*column, *index, *size)));
        }
        Some(gesture) => {
            if let Gesture::Selection { origin, .. } = gesture
                && (x - origin.0).abs() + (y - origin.1).abs() < 4
            {
                return;
            }
            let Some(pane) = document
                .query_selector(".calc-editor .sheet-scroll")
                .ok()
                .flatten()
            else {
                return;
            };
            let bounds = pane.get_bounding_client_rect();
            let dx = if x as f64 > bounds.right() - 22. {
                24.
            } else if (x as f64) < bounds.left() + 22. {
                -24.
            } else {
                0.
            };
            let dy = if y as f64 > bounds.bottom() - 22. {
                24.
            } else if (y as f64) < bounds.top() + 46. {
                -24.
            } else {
                0.
            };
            pane.scroll_by_with_x_and_y(dx, dy);
            let px = (x as f64).clamp(bounds.left() + 82., bounds.right() - 12.) as f32;
            let py = (y as f64).clamp(bounds.top() + 26., bounds.bottom() - 12.) as f32;
            let Some(point) = document
                .element_from_point(px, py)
                .and_then(|element| cell_point(&element))
            else {
                return;
            };
            match gesture {
                Gesture::Selection {
                    start,
                    axis,
                    origin,
                } => {
                    if (x - origin.0).abs() + (y - origin.1).abs() < 4 {
                        return;
                    }
                    let end = match axis {
                        Some(true) => (NUM_ROWS - 1, point.1),
                        Some(false) => (point.0, NUM_COLS - 1),
                        None => point,
                    };
                    ed.anchor.revise(|v| *v = *start);
                    ed.end.revise(|v| *v = end);
                }
                Gesture::Fill {
                    source,
                    extent: target,
                } => {
                    *target = extent(*source, point);
                    ed.fill_preview.revise(|v| *v = Some(*target));
                }
                Gesture::Resize { .. } => {}
            }
        }
        None => {}
    }
}
pub(super) fn install(scope: &Scope, ed: Editor) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let gesture_state: Drag = Rc::new(RefCell::new(None));
    let cursor = Rc::new(std::cell::Cell::new((0, 0)));
    let timer = Rc::new(RefCell::new(None::<gloo_timers::callback::Interval>));
    let down = {
        let gesture_state = gesture_state.clone();
        let document = document.clone();
        let timer = timer.clone();
        let cursor = cursor.clone();
        let weak = scope.downgrade();
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |event: web_sys::PointerEvent| {
            if weak.upgrade().is_none() {
                return;
            }
            let Some(element) = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            else {
                return;
            };
            if event.button() == 2 {
                if let Some(point) = cell_point(&element)
                    && !ed.selected().contains(&point)
                {
                    ed.activate(point);
                }
                return;
            }
            if event.button() != 0 || closest(&element, ".calc-editor .calc-cell-input").is_some() {
                return;
            }
            let position = (event.client_x(), event.client_y());
            let gesture = if closest(&element, ".calc-editor .calc-fill-handle").is_some() {
                event.prevent_default();
                if !ed.finish_edit(false) {
                    return;
                }
                let source = ed.range();
                ed.fill_preview.revise(|v| *v = Some(source));
                Some(Gesture::Fill {
                    source,
                    extent: source,
                })
            } else if let Some(handle) = closest(
                &element,
                ".calc-editor .calc-column-resize,.calc-editor .calc-row-resize",
            ) {
                event.prevent_default();
                let column = handle.class_list().contains("calc-column-resize");
                let Some(index) = axis_index(
                    &handle,
                    if column {
                        "Resize column "
                    } else {
                        "Resize row "
                    },
                    column,
                ) else {
                    return;
                };
                let original = ed
                    .book
                    .with(|b| sheet_axis_size(&b.content["sheets"][ed.sheet.get()], column, index));
                Some(Gesture::Resize {
                    column,
                    index,
                    start: if column { position.0 } else { position.1 },
                    original,
                    size: original,
                })
            } else if let Some(header) = closest(&element, ".calc-editor .calc-grid-header") {
                event.prevent_default();
                focus(&header);
                if header.class_list().contains("calc-select-all") {
                    ed.select_range(MergedRange {
                        start: (0, 0),
                        end: (NUM_ROWS - 1, NUM_COLS - 1),
                    });
                    None
                } else {
                    let column = header.class_list().contains("calc-column-header");
                    let Some(index) = axis_index(
                        &header,
                        if column {
                            "Select column "
                        } else {
                            "Select row "
                        },
                        column,
                    ) else {
                        return;
                    };
                    let start = if event.shift_key() {
                        if column {
                            (0, ed.anchor.get().1)
                        } else {
                            (ed.anchor.get().0, 0)
                        }
                    } else if column {
                        (0, index)
                    } else {
                        (index, 0)
                    };
                    let end = if column {
                        (NUM_ROWS - 1, index)
                    } else {
                        (index, NUM_COLS - 1)
                    };
                    ed.select_range(MergedRange { start, end });
                    (event.pointer_type() != "touch").then_some(Gesture::Selection {
                        start,
                        axis: Some(column),
                        origin: position,
                    })
                }
            } else if let Some(point) = cell_point(&element) {
                ed.select_point(
                    point,
                    event.shift_key(),
                    event.ctrl_key() || event.meta_key(),
                );
                (event.pointer_type() != "touch").then_some(Gesture::Selection {
                    start: ed.anchor.get(),
                    axis: None,
                    origin: position,
                })
            } else {
                return;
            };
            *gesture_state.borrow_mut() = gesture;
            cursor.set(position);
            if gesture_state.borrow().is_some() {
                let document = document.clone();
                let gesture_state = gesture_state.clone();
                let cursor = cursor.clone();
                let weak = weak.clone();
                *timer.borrow_mut() = Some(gloo_timers::callback::Interval::new(60, move || {
                    if weak.upgrade().is_some() {
                        let (x, y) = cursor.get();
                        update(&document, ed, &gesture_state, x, y);
                    }
                }));
            }
        })
    };
    let movement = {
        let document = document.clone();
        let gesture_state = gesture_state.clone();
        let cursor = cursor.clone();
        let weak = scope.downgrade();
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |event: web_sys::PointerEvent| {
            if gesture_state.borrow().is_none() || weak.upgrade().is_none() {
                return;
            }
            event.prevent_default();
            cursor.set((event.client_x(), event.client_y()));
            update(
                &document,
                ed,
                &gesture_state,
                event.client_x(),
                event.client_y(),
            );
        })
    };
    let up = {
        let document = document.clone();
        let gesture_state = gesture_state.clone();
        let timer = timer.clone();
        let weak = scope.downgrade();
        Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |event: web_sys::PointerEvent| {
            if gesture_state.borrow().is_none() || weak.upgrade().is_none() {
                return;
            }
            update(
                &document,
                ed,
                &gesture_state,
                event.client_x(),
                event.client_y(),
            );
            let gesture = gesture_state.borrow_mut().take();
            timer.borrow_mut().take();
            ed.fill_preview.revise(|v| *v = None);
            ed.resize_preview.revise(|v| *v = None);
            match gesture {
                Some(Gesture::Fill { source, extent }) => {
                    ed.drag_fill(source, extent);
                }
                Some(Gesture::Resize {
                    column,
                    index,
                    original,
                    size,
                    ..
                }) if (size - original).abs() >= 1. => ed.resize_axis(column, index, size),
                _ => {}
            }
        })
    };
    let cancel = {
        let gesture_state = gesture_state.clone();
        let timer = timer.clone();
        let weak = scope.downgrade();
        Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            gesture_state.borrow_mut().take();
            timer.borrow_mut().take();
            if weak.upgrade().is_some() {
                ed.fill_preview.revise(|v| *v = None);
                ed.resize_preview.revise(|v| *v = None);
            }
        })
    };
    let escape = {
        let gesture_state = gesture_state.clone();
        let timer = timer.clone();
        let weak = scope.downgrade();
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if event.key() == "Escape"
                && gesture_state.borrow().is_some()
                && weak.upgrade().is_some()
            {
                event.prevent_default();
                gesture_state.borrow_mut().take();
                timer.borrow_mut().take();
                ed.fill_preview.revise(|v| *v = None);
                ed.resize_preview.revise(|v| *v = None);
            }
        })
    };
    let click = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
        move |event: web_sys::MouseEvent| {
            if event.detail()>0&&event.target().and_then(|t|t.dyn_into::<web_sys::Element>().ok()).is_some_and(|element|closest(&element,".calc-editor .cell-button,.calc-editor .calc-grid-header,.calc-editor .calc-fill-handle,.calc-editor .calc-column-resize,.calc-editor .calc-row-resize").is_some()){
            // Pointer-down already selected the cell; a queued Activate would
            // collapse a gesture_state/Shift selection when the mouse is released.
            event.stop_propagation();
        }
        },
    );
    let _ = document.add_event_listener_with_callback_and_bool(
        "pointerdown",
        down.as_ref().unchecked_ref(),
        true,
    );
    let _ = document.add_event_listener_with_callback_and_bool(
        "pointermove",
        movement.as_ref().unchecked_ref(),
        true,
    );
    let _ = document.add_event_listener_with_callback_and_bool(
        "pointerup",
        up.as_ref().unchecked_ref(),
        true,
    );
    let _ =
        document.add_event_listener_with_callback("pointercancel", cancel.as_ref().unchecked_ref());
    let _ = window.add_event_listener_with_callback("blur", cancel.as_ref().unchecked_ref());
    let _ = document.add_event_listener_with_callback_and_bool(
        "keydown",
        escape.as_ref().unchecked_ref(),
        true,
    );
    let _ = window.add_event_listener_with_callback_and_bool(
        "click",
        click.as_ref().unchecked_ref(),
        true,
    );
    let _ = scope.on_cleanup(move || {
        timer.borrow_mut().take();
        let _ = document.remove_event_listener_with_callback_and_bool(
            "pointerdown",
            down.as_ref().unchecked_ref(),
            true,
        );
        let _ = document.remove_event_listener_with_callback_and_bool(
            "pointermove",
            movement.as_ref().unchecked_ref(),
            true,
        );
        let _ = document.remove_event_listener_with_callback_and_bool(
            "pointerup",
            up.as_ref().unchecked_ref(),
            true,
        );
        let _ = document
            .remove_event_listener_with_callback("pointercancel", cancel.as_ref().unchecked_ref());
        let _ = window.remove_event_listener_with_callback("blur", cancel.as_ref().unchecked_ref());
        let _ = document.remove_event_listener_with_callback_and_bool(
            "keydown",
            escape.as_ref().unchecked_ref(),
            true,
        );
        let _ = window.remove_event_listener_with_callback_and_bool(
            "click",
            click.as_ref().unchecked_ref(),
            true,
        );
    });
}
