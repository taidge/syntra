use std::collections::HashMap;

use dioxus::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::components::navbar::Navbar;
use crate::components::toolbar::Toolbar;
use crate::state::{AuthState, api_get};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct WorkbookLite {
    #[serde(default)]
    sheets: Vec<SheetLite>,
}

#[derive(Deserialize)]
struct SheetLite {
    #[serde(default)]
    name: String,
    #[serde(default)]
    cells: serde_json::Value,
}

fn cell_value_to_string(v: &serde_json::Value) -> String {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let inner = v.get("value");
    match ty {
        "Empty" => String::new(),
        "Text" => inner.and_then(|x| x.as_str()).unwrap_or("").to_string(),
        "Number" => inner
            .and_then(|x| x.as_f64())
            .map(|n| n.to_string())
            .unwrap_or_default(),
        "Boolean" => inner
            .and_then(|x| x.as_bool())
            .map(|b| if b { "TRUE" } else { "FALSE" }.to_string())
            .unwrap_or_default(),
        "Date" => inner.and_then(|x| x.as_str()).unwrap_or("").to_string(),
        "Error" => format!(
            "#ERR: {}",
            inner.and_then(|x| x.as_str()).unwrap_or("")
        ),
        _ => String::new(),
    }
}

fn hydrate_workbook(
    content: &serde_json::Value,
) -> (Vec<String>, HashMap<(usize, u32, u32), String>) {
    let wb: WorkbookLite = match serde_json::from_value(content.clone()) {
        Ok(w) => w,
        Err(_) => return (Vec::new(), HashMap::new()),
    };
    let mut names = Vec::new();
    let mut cells = HashMap::new();
    for (sheet_idx, sheet) in wb.sheets.iter().enumerate() {
        names.push(sheet.name.clone());
        // `cells` may be either a HashMap object {"row,col": Cell} OR
        // a JSON object whose keys are JSON-encoded CellRef. Serde JSON
        // serializes HashMap<CellRef, Cell> as an array of [CellRef, Cell]
        // pairs because CellRef is not a string. Handle both shapes.
        if let Some(arr) = sheet.cells.as_array() {
            for pair in arr {
                let Some(pair_arr) = pair.as_array() else { continue };
                if pair_arr.len() != 2 { continue; }
                let cref = &pair_arr[0];
                let cell = &pair_arr[1];
                let row = cref.get("row").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let col = cref.get("col").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let value = cell.get("value");
                let formula = cell
                    .get("formula")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
                let display = if let Some(f) = formula {
                    f
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
            }
        } else if let Some(obj) = sheet.cells.as_object() {
            for (k, cell) in obj {
                let (row, col) = if let Ok(cref) = serde_json::from_str::<serde_json::Value>(k) {
                    (
                        cref.get("row").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        cref.get("col").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    )
                } else {
                    continue;
                };
                let value = cell.get("value");
                let formula = cell
                    .get("formula")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
                let display = if let Some(f) = formula {
                    f
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
            }
        }
    }
    (names, cells)
}

const FORMULA_INPUT_ID: &str = "calc-formula-input";

fn focus_formula_input() {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id(FORMULA_INPUT_ID) {
            if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
                let _ = input.focus();
                let len = input.value().len() as u32;
                let _ = input.set_selection_range(len, len);
            }
        }
    }
}

/// Total number of columns in the spreadsheet grid.
const NUM_COLS: u32 = 26;
/// Total number of rows in the spreadsheet grid.
const NUM_ROWS: u32 = 50;

/// Convert a 0-based column index to a letter (A-Z).
fn col_letter(col: u32) -> char {
    (b'A' + col as u8) as char
}

/// Human-readable cell reference (e.g. "A1", "C12").
fn cell_ref(row: u32, col: u32) -> String {
    format!("{}{}", col_letter(col), row + 1)
}

/// Return the display value for a cell.  Formulas (starting with `=`) are
/// shown verbatim for now — evaluation will happen server-side on save.
fn display_value(raw: &str) -> &str {
    raw
}

/// Return `true` when `cell` is inside the rectangle defined by corners `a`
/// and `b` (inclusive, order-independent).
fn in_selection(cell: (u32, u32), a: (u32, u32), b: (u32, u32)) -> bool {
    let min_row = a.0.min(b.0);
    let max_row = a.0.max(b.0);
    let min_col = a.1.min(b.1);
    let max_col = a.1.max(b.1);
    cell.0 >= min_row && cell.0 <= max_row && cell.1 >= min_col && cell.1 <= max_col
}

/// Tracks an in-progress drag-to-fill gesture.
#[derive(Clone, Copy, PartialEq)]
struct FillState {
    origin_start: (u32, u32),
    origin_end: (u32, u32),
    target: (u32, u32),
}

/// Compute the union rectangle covered by the fill preview.
fn fill_rect(state: &FillState) -> ((u32, u32), (u32, u32)) {
    let (sr, sc) = state.origin_start;
    let (er, ec) = state.origin_end;
    let (tr, tc) = state.target;
    let min_row = sr.min(er).min(tr);
    let max_row = sr.max(er).max(tr);
    let min_col = sc.min(ec).min(tc);
    let max_col = sc.max(ec).max(tc);
    ((min_row, min_col), (max_row, max_col))
}

/// Spreadsheet editor with formula bar, cell grid, and sheet tabs.
#[component]
pub fn Calc(id: String) -> Element {
    // ── Cell data ────────────────────────────────────────────────────────
    // Key: (sheet_index, row, col)  Value: raw text / formula
    let mut cells = use_signal(|| HashMap::<(usize, u32, u32), String>::new());

    // ── Active cell & editing ────────────────────────────────────────────
    let mut active_cell = use_signal(|| (0u32, 0u32));
    let mut formula_bar = use_signal(|| String::new());
    let mut is_editing = use_signal(|| false);

    // ── Multi-cell selection ─────────────────────────────────────────────
    let mut selection_start = use_signal(|| None::<(u32, u32)>);
    let mut selection_end = use_signal(|| None::<(u32, u32)>);
    let mut is_selecting = use_signal(|| false);
    let mut extra_selection = use_signal(|| Vec::<(u32, u32)>::new());

    // ── Drag-to-fill state ───────────────────────────────────────────────
    let mut fill_mode = use_signal(|| None::<FillState>);

    // ── Sheet tabs ───────────────────────────────────────────────────────
    let mut sheets = use_signal(|| {
        vec![
            "Sheet1".to_string(),
            "Sheet2".to_string(),
            "Sheet3".to_string(),
        ]
    });
    let mut active_sheet = use_signal(|| 0usize);

    // ── Document chrome ──────────────────────────────────────────────────
    let mut doc_title = use_signal(|| "Untitled Spreadsheet".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut loaded = use_signal(|| false);

    let auth = use_context::<Signal<AuthState>>();
    let id_for_resource = id.clone();
    let doc_resource = use_resource(move || {
        let id = id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            api_get::<DocumentResponse>(&format!("/documents/{id}"), &token).await
        }
    });

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        if let Some(Ok(doc)) = doc_resource.read().as_ref() {
            doc_title.set(doc.name.clone());
            let (names, hydrated) = hydrate_workbook(&doc.content);
            if !names.is_empty() {
                sheets.set(names);
            }
            cells.set(hydrated);
            active_sheet.set(0);
            loaded.set(true);
        }
    });

    // ── Derived values ───────────────────────────────────────────────────
    let (active_row, active_col) = *active_cell.read();
    let sheet_idx = *active_sheet.read();

    // Helper: commit the current formula bar contents into the cell map.
    let mut commit_formula_bar = {
        move || {
            let value = formula_bar.read().clone();
            let (r, c) = *active_cell.read();
            let si = *active_sheet.read();
            if value.is_empty() {
                cells.write().remove(&(si, r, c));
            } else {
                cells.write().insert((si, r, c), value);
            }
            is_editing.set(false);
            saved_status.set("Unsaved changes".to_string());
        }
    };

    // ── Status bar aggregates (sum / average / count) ────────────────────
    let (stat_sum, stat_avg, stat_count) = {
        let sel_start = *selection_start.read();
        let sel_end = *selection_end.read();
        match (sel_start, sel_end) {
            (Some(a), Some(b)) => {
                let cells_ref = cells.read();
                let min_row = a.0.min(b.0);
                let max_row = a.0.max(b.0);
                let min_col = a.1.min(b.1);
                let max_col = a.1.max(b.1);
                let mut sum: f64 = 0.0;
                let mut numeric_count: u32 = 0;
                let mut non_empty: u32 = 0;
                for r in min_row..=max_row {
                    for c in min_col..=max_col {
                        if let Some(val) = cells_ref.get(&(sheet_idx, r, c)) {
                            if !val.is_empty() {
                                non_empty += 1;
                                if let Ok(n) = val.parse::<f64>() {
                                    sum += n;
                                    numeric_count += 1;
                                }
                            }
                        }
                    }
                }
                let avg = if numeric_count > 0 {
                    sum / numeric_count as f64
                } else {
                    0.0
                };
                (sum, avg, non_empty)
            }
            _ => (0.0, 0.0, 0),
        }
    };

    rsx! {
        Navbar {}
        div {
            class: "editor-container",
            tabindex: "0",

            // ── Keyboard navigation ──────────────────────────────────────
            onkeydown: move |evt: KeyboardEvent| {
                let key = evt.key();
                let modifiers = evt.modifiers();
                let (mut r, mut c) = *active_cell.read();

                match key {
                    // Arrow keys — move active cell (only when not editing)
                    Key::ArrowUp if !*is_editing.read() => {
                        evt.prevent_default();
                        if r > 0 { r -= 1; }
                    }
                    Key::ArrowDown if !*is_editing.read() => {
                        evt.prevent_default();
                        if r + 1 < NUM_ROWS { r += 1; }
                    }
                    Key::ArrowLeft if !*is_editing.read() => {
                        evt.prevent_default();
                        if c > 0 { c -= 1; }
                    }
                    Key::ArrowRight if !*is_editing.read() => {
                        evt.prevent_default();
                        if c + 1 < NUM_COLS { c += 1; }
                    }

                    // Tab / Shift+Tab
                    Key::Tab => {
                        evt.prevent_default();
                        if *is_editing.read() {
                            commit_formula_bar();
                        }
                        if modifiers.contains(Modifiers::SHIFT) {
                            if c > 0 { c -= 1; }
                        } else if c + 1 < NUM_COLS {
                            c += 1;
                        }
                    }

                    // Enter — confirm edit, move down
                    Key::Enter if *is_editing.read() => {
                        evt.prevent_default();
                        commit_formula_bar();
                        if r + 1 < NUM_ROWS { r += 1; }
                    }

                    // Escape — cancel edit
                    Key::Escape => {
                        // Restore original value
                        let si = *active_sheet.read();
                        let orig = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                        formula_bar.set(orig);
                        is_editing.set(false);
                        fill_mode.set(None);
                    }

                    // Delete / Backspace — clear cell
                    Key::Delete | Key::Backspace if !*is_editing.read() => {
                        evt.prevent_default();
                        let si = *active_sheet.read();
                        cells.write().remove(&(si, r, c));
                        formula_bar.set(String::new());
                        saved_status.set("Unsaved changes".to_string());
                    }

                    // Ctrl+S — save stub
                    Key::Character(ref ch) if ch == "s" && modifiers.contains(Modifiers::CONTROL) => {
                        evt.prevent_default();
                        saved_status.set("All changes saved".to_string());
                    }

                    // Any printable character starts an edit by seeding the formula bar
                    Key::Character(ref ch)
                        if !*is_editing.read()
                            && !modifiers.contains(Modifiers::CONTROL)
                            && !modifiers.contains(Modifiers::META)
                            && !modifiers.contains(Modifiers::ALT) =>
                    {
                        evt.prevent_default();
                        formula_bar.set(ch.clone());
                        is_editing.set(true);
                        saved_status.set("Unsaved changes".to_string());
                        focus_formula_input();
                    }

                    // F2 enters edit mode preserving the existing value
                    Key::F2 if !*is_editing.read() => {
                        evt.prevent_default();
                        is_editing.set(true);
                        focus_formula_input();
                    }

                    _ => {}
                }

                // Update active cell if it changed
                if (r, c) != *active_cell.read() {
                    // Commit any pending edit before moving
                    if *is_editing.read() {
                        commit_formula_bar();
                    }
                    active_cell.set((r, c));
                    let si = *active_sheet.read();
                    let val = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                    formula_bar.set(val);
                    // Reset selection to single cell
                    selection_start.set(Some((r, c)));
                    selection_end.set(Some((r, c)));
                }
            },

            // Global mouseup to stop drag-select even outside cells
            onmouseup: move |_| {
                is_selecting.set(false);
                let current_fill = *fill_mode.read();
                if let Some(state) = current_fill {
                    let ((min_r, min_c), (max_r, max_c)) = fill_rect(&state);
                    let (osr, osc) = state.origin_start;
                    let (oer, oec) = state.origin_end;
                    let o_min_r = osr.min(oer);
                    let o_max_r = osr.max(oer);
                    let o_min_c = osc.min(oec);
                    let o_max_c = osc.max(oec);
                    let si = *active_sheet.read();
                    let src: Vec<Vec<String>> = (o_min_r..=o_max_r)
                        .map(|r| {
                            (o_min_c..=o_max_c)
                                .map(|c| cells.read().get(&(si, r, c)).cloned().unwrap_or_default())
                                .collect()
                        })
                        .collect();
                    let src_rows = (o_max_r - o_min_r + 1) as usize;
                    let src_cols = (o_max_c - o_min_c + 1) as usize;
                    let mut writer = cells.write();
                    for r in min_r..=max_r {
                        for c in min_c..=max_c {
                            if r >= o_min_r && r <= o_max_r && c >= o_min_c && c <= o_max_c {
                                continue;
                            }
                            let sr_i = ((r - o_min_r) as usize) % src_rows;
                            let sc_i = ((c - o_min_c) as usize) % src_cols;
                            let mut val = src[sr_i][sc_i].clone();
                            if src_rows == 1 && src_cols == 1 {
                                if let Ok(n) = val.parse::<f64>() {
                                    let offset = if r != o_min_r {
                                        (r as i64) - (o_min_r as i64)
                                    } else {
                                        (c as i64) - (o_min_c as i64)
                                    };
                                    val = format!("{}", n + offset as f64);
                                }
                            }
                            if val.is_empty() {
                                writer.remove(&(si, r, c));
                            } else {
                                writer.insert((si, r, c), val);
                            }
                        }
                    }
                    drop(writer);
                    selection_start.set(Some((min_r, min_c)));
                    selection_end.set(Some((max_r, max_c)));
                    saved_status.set("Unsaved changes".to_string());
                    fill_mode.set(None);
                }
            },

            // ── Title bar ────────────────────────────────────────────────
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: move |e| doc_title.set(e.value()),
                    placeholder: "Untitled Spreadsheet",
                }
                span { class: "save-status", "{saved_status}" }
            }

            // ── Toolbar ──────────────────────────────────────────────────
            Toolbar { doc_type: "calc".to_string() }

            // ── Formula bar ──────────────────────────────────────────────
            div { class: "formula-bar",
                span { class: "cell-ref", "{cell_ref(active_row, active_col)}" }
                button {
                    class: "formula-icon",
                    r#type: "button",
                    title: "Insert formula",
                    onclick: move |_| {
                        let current = formula_bar.read().clone();
                        if !current.starts_with('=') {
                            formula_bar.set(format!("={current}"));
                        }
                        is_editing.set(true);
                        focus_formula_input();
                    },
                    "f(x)"
                }
                input {
                    class: "formula-input",
                    id: FORMULA_INPUT_ID,
                    value: "{formula_bar}",
                    placeholder: "Enter value or formula...",
                    oninput: move |e| {
                        formula_bar.set(e.value());
                        is_editing.set(true);
                    },
                    onkeydown: move |evt: KeyboardEvent| {
                        if evt.key() == Key::Enter {
                            evt.prevent_default();
                            commit_formula_bar();
                            // Move active cell down
                            let (r, c) = *active_cell.read();
                            if r + 1 < NUM_ROWS {
                                active_cell.set((r + 1, c));
                                let si = *active_sheet.read();
                                let val = cells.read().get(&(si, r + 1, c)).cloned().unwrap_or_default();
                                formula_bar.set(val);
                            }
                        } else if evt.key() == Key::Escape {
                            let (r, c) = *active_cell.read();
                            let si = *active_sheet.read();
                            let orig = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                            formula_bar.set(orig);
                            is_editing.set(false);
                        }
                    },
                }
            }

            // ── Loading / error overlay ─────────────────────────────────
            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(_)) => rsx! {},
            }

            // ── Spreadsheet grid ─────────────────────────────────────────
            div { class: "spreadsheet",
                table { class: "sheet-grid",
                    thead {
                        tr {
                            th { class: "row-header corner-cell" }
                            for col in 0..NUM_COLS {
                                th {
                                    class: if active_col == col { "col-header active-header" } else { "col-header" },
                                    "{col_letter(col)}"
                                }
                            }
                        }
                    }
                    tbody {
                        for row in 0..NUM_ROWS {
                            tr {
                                th {
                                    class: if active_row == row { "row-header active-header" } else { "row-header" },
                                    "{row + 1}"
                                }
                                for col in 0..NUM_COLS {
                                    {
                                        let is_active = (row, col) == (active_row, active_col);
                                        let sel_s = *selection_start.read();
                                        let sel_e = *selection_end.read();
                                        let is_selected = match (sel_s, sel_e) {
                                            (Some(a), Some(b)) => in_selection((row, col), a, b),
                                            (Some(a), None) => (row, col) == a,
                                            _ => false,
                                        } || extra_selection.read().contains(&(row, col));
                                        let fill_preview = match *fill_mode.read() {
                                            Some(state) => {
                                                let (a, b) = fill_rect(&state);
                                                in_selection((row, col), a, b)
                                            }
                                            None => false,
                                        };

                                        let editing = is_active && *is_editing.read();
                                        let class_str = match (is_active, is_selected, fill_preview, editing) {
                                            (_, _, true, _) => "cell fill-preview",
                                            (_, _, _, true) => "cell active editing",
                                            (true, _, _, _) => "cell active",
                                            (false, true, _, _) => "cell selected",
                                            _ => "cell",
                                        };

                                        let si = sheet_idx;
                                        let cell_value = cells.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                        let display = display_value(&cell_value).to_string();

                                        rsx! {
                                            td {
                                                class: "{class_str}",

                                                onmousedown: move |evt: MouseEvent| {
                                                    if *is_editing.read() {
                                                        commit_formula_bar();
                                                    }
                                                    let modifiers = evt.modifiers();
                                                    let ctrl_like = modifiers.contains(Modifiers::CONTROL) || modifiers.contains(Modifiers::META);
                                                    let shift = modifiers.contains(Modifiers::SHIFT);

                                                    if shift {
                                                        if selection_start.read().is_none() {
                                                            selection_start.set(Some((row, col)));
                                                        }
                                                        selection_end.set(Some((row, col)));
                                                        active_cell.set((row, col));
                                                        is_selecting.set(false);
                                                    } else if ctrl_like {
                                                        let cell = (row, col);
                                                        let mut extra = extra_selection.write();
                                                        if extra.contains(&cell) {
                                                            extra.retain(|&c| c != cell);
                                                        } else {
                                                            extra.push(cell);
                                                        }
                                                        active_cell.set(cell);
                                                        is_selecting.set(false);
                                                    } else {
                                                        active_cell.set((row, col));
                                                        selection_start.set(Some((row, col)));
                                                        selection_end.set(Some((row, col)));
                                                        extra_selection.write().clear();
                                                        is_selecting.set(true);
                                                    }

                                                    let si = *active_sheet.read();
                                                    let val = cells.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                                    formula_bar.set(val);
                                                },

                                                onmouseover: move |_| {
                                                    if *is_selecting.read() {
                                                        selection_end.set(Some((row, col)));
                                                    } else {
                                                        let current = *fill_mode.read();
                                                        if let Some(mut state) = current {
                                                            state.target = (row, col);
                                                            fill_mode.set(Some(state));
                                                        }
                                                    }
                                                },

                                                onmouseup: move |_| {
                                                    is_selecting.set(false);
                                                },

                                                ondoubleclick: move |_| {
                                                    is_editing.set(true);
                                                    focus_formula_input();
                                                },

                                                // Cell display value
                                                "{display}"

                                                if is_active {
                                                    div {
                                                        class: "autofill-handle",
                                                        onmousedown: move |evt: MouseEvent| {
                                                            evt.stop_propagation();
                                                            let sel_s = selection_start.read().unwrap_or((row, col));
                                                            let sel_e = selection_end.read().unwrap_or(sel_s);
                                                            fill_mode.set(Some(FillState {
                                                                origin_start: sel_s,
                                                                origin_end: sel_e,
                                                                target: (row, col),
                                                            }));
                                                            is_selecting.set(false);
                                                        },
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Sheet tabs ───────────────────────────────────────────────
            div { class: "sheet-tabs",
                for (idx, name) in sheets.read().iter().enumerate() {
                    {
                        let tab_class = if idx == *active_sheet.read() {
                            "sheet-tab active"
                        } else {
                            "sheet-tab"
                        };
                        let tab_name = name.clone();
                        rsx! {
                            button {
                                class: "{tab_class}",
                                onclick: move |_| {
                                    // Commit pending edit before switching sheets
                                    if *is_editing.read() {
                                        commit_formula_bar();
                                    }
                                    active_sheet.set(idx);
                                    // Reload formula bar for active cell on new sheet
                                    let (r, c) = *active_cell.read();
                                    let val = cells.read().get(&(idx, r, c)).cloned().unwrap_or_default();
                                    formula_bar.set(val);
                                },
                                "{tab_name}"
                            }
                        }
                    }
                }
                button {
                    class: "sheet-tab-add",
                    title: "Add sheet",
                    onclick: move |_| {
                        let count = sheets.read().len() + 1;
                        sheets.write().push(format!("Sheet{count}"));
                        // Switch to the newly added sheet
                        let new_idx = sheets.read().len() - 1;
                        active_sheet.set(new_idx);
                        let (r, c) = *active_cell.read();
                        let val = cells.read().get(&(new_idx, r, c)).cloned().unwrap_or_default();
                        formula_bar.set(val);
                    },
                    "+"
                }
            }

            // ── Status bar ───────────────────────────────────────────────
            div { class: "status-bar",
                span { "Cell: {cell_ref(active_row, active_col)}" }
                span { "Sum: {stat_sum:.4}" }
                span { "Average: {stat_avg:.4}" }
                span { "Count: {stat_count}" }
            }
        }
    }
}
