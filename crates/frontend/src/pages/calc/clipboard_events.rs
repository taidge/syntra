//! Native clipboard and command keys for grid focus. Text editors keep their
//! browser clipboard, text selection and text undo behavior.
use super::*;
use wasm_bindgen::{JsCast, closure::Closure};

const WORKBOOK_MIME: &str = "application/x-syntra-calc-cells+json";
const LEGACY_WORKBOOK_MIME: &str = "application/x-\x63ontrix-calc-cells+json";
const MAX_CLIPBOARD_BYTES: usize = 1_048_576;
type ClipboardResult<T> = std::result::Result<T, String>;

thread_local! {
    // Plaintext-only clipboard APIs cannot carry the workbook MIME. Retain only
    // data, never owners, and use it only while the OS text still matches.
    static CLIPBOARD_CACHE: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

fn remember(plain: String, internal: String) {
    CLIPBOARD_CACHE.with(|cache| *cache.borrow_mut() = Some((plain, internal)));
}

fn cached(plain: &str) -> String {
    CLIPBOARD_CACHE.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|(text, _)| text == plain)
            .map(|(_, internal)| internal.clone())
            .unwrap_or_default()
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ClipboardCell {
    value: String,
    format: Option<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ClipboardCells {
    version: u8,
    origin: (u32, u32),
    cut: bool,
    cells: Vec<Vec<ClipboardCell>>,
}

fn grid_target(event: &web_sys::Event, ed: Editor) -> bool {
    if ed.editing.get().is_some() {
        return false;
    }
    let target = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .filter(|target| !matches!(target.tag_name().as_str(), "BODY" | "HTML"))
        .or_else(|| {
            web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.active_element())
        });
    let Some(target) = target else {
        return false;
    };
    target
        .closest("input,textarea,select,[contenteditable]:not([contenteditable='false'])")
        .ok()
        .flatten()
        .is_none()
        && target
            .closest(".calc-editor .sheet-scroll")
            .ok()
            .flatten()
            .is_some()
}

fn selection(ed: Editor) -> MergedRange {
    MergedRange {
        start: ed.anchor.get(),
        end: ed.end.get(),
    }
    .normalized()
}

fn message(ed: Editor, value: impl Into<String>) {
    ed.status.revise(|status| *status = value.into());
}

fn restore_grid_focus(weak: &glory::ui::WeakScope, ed: Editor) {
    let weak = weak.clone();
    wasm_bindgen_futures::spawn_local(async move {
        // Undo rebuilds the grid; wait for that render before restoring the
        // active cell instead of leaving keyboard focus on a detached button.
        gloo_timers::future::TimeoutFuture::new(20).await;
        if weak.upgrade().is_none() || ed.editing.get().is_some() {
            return;
        }
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        if document.active_element().is_some_and(|element| {
            element
                .matches("input,textarea,select,[contenteditable]:not([contenteditable='false'])")
                .unwrap_or(false)
        }) {
            return;
        }
        let (row, col) = ed.active.get();
        if let Ok(Some(cell)) = document.query_selector(&format!(
            ".calc-editor .cell-button[aria-label='{}']",
            cell_ref(row, col)
        )) && let Ok(cell) = cell.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = cell.focus();
        }
    });
}

fn quote_tsv(value: &str) -> String {
    if value.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn clipboard_format(style: &CellStyle) -> Option<serde_json::Value> {
    (style != &CellStyle::default()).then(|| {
        serde_json::json!({
            "bold": style.bold,
            "italic": style.italic,
            "underline": style.underline,
            "number_format": style.number_format,
            "horizontal_align": style.align,
            "bg_color": style.fill_color,
            "text_color": style.text_color,
            "font_family": style.font_family,
            "font_size": style.font_size,
            "borders": {
                "top": style.borders.top, "right": style.borders.right,
                "bottom": style.borders.bottom, "left": style.borders.left,
            },
        })
    })
}

fn parse_tsv(value: &str) -> ClipboardResult<Vec<Vec<String>>> {
    if value.len() > MAX_CLIPBOARD_BYTES {
        return Err("Clipboard data is too large".into());
    }
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut row_ended = false;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        row_ended = false;
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
        } else {
            match ch {
                '"' if field.is_empty() => quoted = true,
                '\t' => row.push(std::mem::take(&mut field)),
                '\r' | '\n' => {
                    if ch == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut row));
                    row_ended = true;
                }
                _ => field.push(ch),
            }
        }
        if row.len() > NUM_COLS as usize || records.len() > NUM_ROWS as usize {
            return Err("Clipboard range exceeds the worksheet bounds".into());
        }
    }
    if quoted {
        return Err("Clipboard contains an unfinished quoted cell".into());
    }
    if !row_ended || !row.is_empty() || !field.is_empty() {
        row.push(field);
        records.push(row);
    }
    let width = records.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 || width > NUM_COLS as usize || records.len() > NUM_ROWS as usize {
        return Err("Clipboard range exceeds the worksheet bounds".into());
    }
    for row in &mut records {
        row.resize(width, String::new());
    }
    Ok(records)
}

fn copied_cells(ed: Editor, cut: bool) -> ClipboardResult<ClipboardCells> {
    if !ed.extra.get().is_empty() {
        return Err("Copy and cut require a rectangular selection".into());
    }
    let range = selection(ed);
    let sheet = ed.sheet.get();
    ed.book.with(|book| {
        if book
            .snapshot
            .merged_cells
            .get(&sheet)
            .is_some_and(|ranges| ranges.iter().any(|merged| merged.overlaps(range)))
        {
            return Err("Unmerge the selected cells before copying or cutting".into());
        }
        if cut {
            for row in range.start.0..=range.end.0 {
                for col in range.start.1..=range.end.1 {
                    if let Some(error) = validation_error_for(&book.content, sheet, row, col, "") {
                        return Err(error);
                    }
                }
            }
        }
        let cells = (range.start.0..=range.end.0)
            .map(|row| {
                (range.start.1..=range.end.1)
                    .map(|col| {
                        let key = (sheet, row, col);
                        ClipboardCell {
                            value: book.snapshot.cells.get(&key).cloned().unwrap_or_default(),
                            format: book.snapshot.styles.get(&key).and_then(clipboard_format),
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(ClipboardCells {
            version: 1,
            origin: range.start,
            cut,
            cells,
        })
    })
}

fn payload(ed: Editor, cut: bool) -> ClipboardResult<(String, String)> {
    let cells = copied_cells(ed, cut)?;
    let plain = cells
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| quote_tsv(&cell.value))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let internal = serde_json::to_string(&cells)
        .map_err(|_| "Could not encode the selected cells".to_string())?;
    if plain.len() > MAX_CLIPBOARD_BYTES || internal.len() > MAX_CLIPBOARD_BYTES {
        return Err("Selected data is too large for the clipboard".into());
    }
    Ok((plain, internal))
}

fn clear_cut(ed: Editor, sheet: usize, range: MergedRange) -> ClipboardResult<()> {
    let cells: Vec<_> = (range.start.0..=range.end.0)
        .flat_map(|row| (range.start.1..=range.end.1).map(move |col| (row, col)))
        .collect();
    ed.book.with(|book| {
        for &(row, col) in &cells {
            if let Some(error) = validation_error_for(&book.content, sheet, row, col, "") {
                return Err(error);
            }
        }
        Ok(())
    })?;
    ed.book.revise(|book| {
        if cells
            .iter()
            .any(|&(row, col)| book.snapshot.cells.contains_key(&(sheet, row, col)))
        {
            book.checkpoint();
            for (row, col) in cells {
                book.snapshot.cells.remove(&(sheet, row, col));
            }
        }
    });
    if ed.sheet.get() == sheet && range.contains(ed.active.get()) {
        ed.formula.revise(String::clear);
    }
    Ok(())
}

/// Context menus share the native clipboard event's data/format model. Browser
/// clipboard promises finish only while the originating Calc scope is alive.
pub(super) fn request_copy(scope: &Scope, ed: Editor, cut: bool) {
    if !ed.finish_edit(false) {
        return;
    }
    let (plain, internal) = match payload(ed, cut) {
        Ok(data) => data,
        Err(error) => {
            message(ed, error);
            return;
        }
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = window.navigator().clipboard().write_text(&plain);
    let weak = scope.downgrade();
    let sheet = ed.sheet.get();
    let source = selection(ed);
    let revision = ed.book.with(|book| book.revision);
    let _ = scope.spawn(async move {
        let result = wasm_bindgen_futures::JsFuture::from(promise).await;
        if weak.upgrade().is_none() {
            return;
        }
        match result {
            Ok(_) => {
                if cut {
                    if ed.book.with(|book| book.revision) != revision {
                        message(
                            ed,
                            "Selection copied; cut cancelled because the workbook changed",
                        );
                        return;
                    }
                    if let Err(error) = clear_cut(ed, sheet, source) {
                        message(ed, error);
                        return;
                    }
                }
                remember(plain, internal);
                message(
                    ed,
                    if cut {
                        "Selection cut"
                    } else {
                        "Selection copied"
                    },
                );
            }
            Err(_) => message(
                ed,
                "Clipboard access was denied; use Ctrl/Cmd+C or Ctrl/Cmd+X",
            ),
        }
    });
}

pub(super) fn request_paste(scope: &Scope, ed: Editor) {
    if !ed.finish_edit(false) {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = window.navigator().clipboard().read_text();
    let weak = scope.downgrade();
    let sheet = ed.sheet.get();
    let target = selection(ed);
    let _ = scope.spawn(async move {
        let result = wasm_bindgen_futures::JsFuture::from(promise).await;
        if weak.upgrade().is_none() {
            return;
        }
        if ed.sheet.get() != sheet || selection(ed) != target || ed.editing.get().is_some() {
            message(ed, "Selection changed; paste again at the intended cells");
            return;
        }
        match result {
            Ok(text) => {
                let plain = text.as_string().unwrap_or_default();
                let internal = cached(&plain);
                if plain.is_empty() && internal.is_empty() {
                    message(ed, "The clipboard contains no cell data");
                } else {
                    match paste(ed, &plain, &internal) {
                        Ok(()) => {
                            message(ed, "Cells pasted");
                            restore_grid_focus(&weak, ed);
                        }
                        Err(error) => message(ed, error),
                    }
                }
            }
            Err(_) => message(ed, "Clipboard access was denied; use Ctrl/Cmd+V"),
        }
    });
}

fn paste(ed: Editor, plain: &str, internal: &str) -> ClipboardResult<()> {
    let cells: Option<ClipboardCells> = (internal.len() <= MAX_CLIPBOARD_BYTES)
        .then(|| serde_json::from_str(internal).ok())
        .flatten();
    let cells = cells.filter(|cells| {
        cells.version == 1 && cells.origin.0 < NUM_ROWS && cells.origin.1 < NUM_COLS
    });
    let mut values = if let Some(cells) = &cells {
        cells
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.value.clone())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    } else {
        parse_tsv(plain)?
    };
    let height = values.len();
    let width = values.first().map_or(0, Vec::len);
    if height == 0
        || width == 0
        || height > NUM_ROWS as usize
        || width > NUM_COLS as usize
        || values.iter().any(|row| row.len() != width)
    {
        return Err("Clipboard cells must form a rectangular worksheet range".into());
    }
    if !ed.extra.get().is_empty() {
        return Err("Paste requires a rectangular selection".into());
    }
    let start = selection(ed).start;
    let end = (start.0 + height as u32 - 1, start.1 + width as u32 - 1);
    if end.0 >= NUM_ROWS || end.1 >= NUM_COLS {
        return Err("Pasted range exceeds the worksheet bounds".into());
    }
    if let Some(cells) = &cells
        && !cells.cut
    {
        let dr = start.0 as i32 - cells.origin.0 as i32;
        let dc = start.1 as i32 - cells.origin.1 as i32;
        for row in &mut values {
            for value in row {
                if value.starts_with('=') {
                    *value = super::super::fill::shift_formula(value, dr, dc);
                }
            }
        }
    }
    let sheet = ed.sheet.get();
    ed.book.with(|book| {
        let target = MergedRange { start, end };
        if book
            .snapshot
            .merged_cells
            .get(&sheet)
            .is_some_and(|ranges| ranges.iter().any(|merged| merged.overlaps(target)))
        {
            return Err("Unmerge destination cells before pasting".into());
        }
        for (dr, row) in values.iter().enumerate() {
            for (dc, value) in row.iter().enumerate() {
                if let Some(error) = validation_error_for(
                    &book.content,
                    sheet,
                    start.0 + dr as u32,
                    start.1 + dc as u32,
                    value,
                ) {
                    return Err(error);
                }
            }
        }
        Ok(())
    })?;
    ed.book.revise(|book| {
        let changed = values.iter().enumerate().any(|(dr, row)| {
            row.iter().enumerate().any(|(dc, value)| {
                let key = (sheet, start.0 + dr as u32, start.1 + dc as u32);
                book.snapshot
                    .cells
                    .get(&key)
                    .map(String::as_str)
                    .unwrap_or("")
                    != value
                    || cells.as_ref().is_some_and(|cells| {
                        let style = cells.cells[dr][dc]
                            .format
                            .as_ref()
                            .map(style_from_json)
                            .unwrap_or_default();
                        book.snapshot.styles.get(&key).cloned().unwrap_or_default() != style
                    })
            })
        });
        if !changed {
            return;
        }
        book.checkpoint();
        for (dr, row) in values.iter().enumerate() {
            for (dc, value) in row.iter().enumerate() {
                let key = (sheet, start.0 + dr as u32, start.1 + dc as u32);
                if value.is_empty() {
                    book.snapshot.cells.remove(&key);
                } else {
                    book.snapshot.cells.insert(key, value.clone());
                }
                if let Some(cells) = &cells {
                    if let Some(format) = &cells.cells[dr][dc].format {
                        book.snapshot.styles.insert(key, style_from_json(format));
                    } else {
                        book.snapshot.styles.remove(&key);
                    }
                }
            }
        }
    });
    ed.active.revise(|value| *value = start);
    ed.anchor.revise(|value| *value = start);
    ed.end.revise(|value| *value = end);
    ed.extra.revise(Vec::clear);
    let first = values[0][0].clone();
    ed.formula.revise(|value| *value = first);
    Ok(())
}

/// Explicit editable TSV fallback uses the same atomic plan as OS paste, without
/// treating text edited by the user as an internal formatted clipboard payload.
pub(super) fn request_paste_text(ed: Editor, value: String) {
    if !ed.finish_edit(false) {
        return;
    }
    match paste(ed, &value, "") {
        Ok(()) => message(ed, "Cells pasted"),
        Err(error) => message(ed, error),
    }
}

pub(super) fn install(scope: &Scope, ed: Editor) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let mut listeners = Vec::new();
    for kind in ["copy", "cut", "paste"] {
        let weak = scope.downgrade();
        let listener = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(
            move |event: web_sys::ClipboardEvent| {
                if weak.upgrade().is_none() || !grid_target(event.as_ref(), ed) {
                    return;
                }
                let Some(data) = event.clipboard_data() else {
                    return;
                };
                if kind == "paste" {
                    let plain = data.get_data("text/plain").unwrap_or_default();
                    let internal = data.get_data(WORKBOOK_MIME).unwrap_or_default();
                    let internal = if internal.is_empty() {
                        data.get_data(LEGACY_WORKBOOK_MIME).unwrap_or_default()
                    } else {
                        internal
                    };
                    let internal = if internal.is_empty() {
                        cached(&plain)
                    } else {
                        internal
                    };
                    if plain.is_empty() && internal.is_empty() {
                        return;
                    }
                    event.prevent_default();
                    event.stop_immediate_propagation();
                    match paste(ed, &plain, &internal) {
                        Ok(()) => {
                            message(ed, "Cells pasted");
                            restore_grid_focus(&weak, ed);
                        }
                        Err(error) => message(ed, error),
                    }
                } else {
                    event.prevent_default();
                    event.stop_immediate_propagation();
                    let result = payload(ed, kind == "cut").and_then(|(plain, internal)| {
                        data.set_data("text/plain", &plain)
                            .map_err(|_| "Could not write the system clipboard".to_string())?;
                        let _ = data.set_data(WORKBOOK_MIME, &internal);
                        if kind == "cut" {
                            clear_cut(ed, ed.sheet.get(), selection(ed))?;
                        }
                        remember(plain, internal);
                        Ok(())
                    });
                    match result {
                        Ok(()) => message(
                            ed,
                            if kind == "cut" {
                                "Selection cut"
                            } else {
                                "Selection copied"
                            },
                        ),
                        Err(error) => message(ed, error),
                    }
                }
            },
        );
        let _ = document.add_event_listener_with_callback_and_bool(
            kind,
            listener.as_ref().unchecked_ref(),
            true,
        );
        listeners.push((kind, listener));
    }
    let weak = scope.downgrade();
    let key =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if weak.upgrade().is_none()
                || event.is_composing()
                || event.alt_key()
                || !(event.ctrl_key() || event.meta_key())
                || !grid_target(event.as_ref(), ed)
            {
                return;
            }
            match event.key().to_ascii_lowercase().as_str() {
                "c" | "x" | "v" => {
                    // Keep the browser's default action which dispatches the trusted
                    // ClipboardEvent, but do not also run Glory command handling.
                    event.stop_immediate_propagation();
                }
                "z" | "y" => {
                    event.prevent_default();
                    event.stop_immediate_propagation();
                    ed.history(event.key().eq_ignore_ascii_case("y") || event.shift_key());
                    restore_grid_focus(&weak, ed);
                }
                "a" => {
                    event.prevent_default();
                    event.stop_immediate_propagation();
                    ed.anchor.revise(|value| *value = (0, 0));
                    ed.end.revise(|value| *value = (NUM_ROWS - 1, NUM_COLS - 1));
                    ed.extra.revise(Vec::clear);
                }
                _ => {}
            }
        });
    let _ = document.add_event_listener_with_callback_and_bool(
        "keydown",
        key.as_ref().unchecked_ref(),
        true,
    );
    let _ = scope.on_cleanup(move || {
        for (kind, listener) in &listeners {
            let _ = document.remove_event_listener_with_callback_and_bool(
                kind,
                listener.as_ref().unchecked_ref(),
                true,
            );
        }
        let _ = document.remove_event_listener_with_callback_and_bool(
            "keydown",
            key.as_ref().unchecked_ref(),
            true,
        );
    });
}
