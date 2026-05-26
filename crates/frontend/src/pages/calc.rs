use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::components::navbar::Navbar;
use crate::state::{AuthState, api_get, api_put};

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

#[derive(Clone, Debug, Default, PartialEq)]
struct CellStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    number_format: String,
}

fn current_selection_cells(
    active: (u32, u32),
    sel_start: Option<(u32, u32)>,
    sel_end: Option<(u32, u32)>,
    extra: &[(u32, u32)],
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let (Some(a), Some(b)) = (sel_start, sel_end) {
        let (r1, r2) = (a.0.min(b.0), a.0.max(b.0));
        let (c1, c2) = (a.1.min(b.1), a.1.max(b.1));
        for r in r1..=r2 {
            for c in c1..=c2 {
                out.push((r, c));
            }
        }
    } else {
        out.push(active);
    }
    for &c in extra {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

fn format_number(value: &str, fmt: &str) -> String {
    match fmt {
        "currency" => value
            .parse::<f64>()
            .map(|n| format!("${:.2}", n))
            .unwrap_or_else(|_| value.to_string()),
        "percent" => value
            .parse::<f64>()
            .map(|n| format!("{:.0}%", n * 100.0))
            .unwrap_or_else(|_| value.to_string()),
        "decimal0" => value
            .parse::<f64>()
            .map(|n| format!("{:.0}", n))
            .unwrap_or_else(|_| value.to_string()),
        "decimal2" => value
            .parse::<f64>()
            .map(|n| format!("{:.2}", n))
            .unwrap_or_else(|_| value.to_string()),
        _ => value.to_string(),
    }
}

fn style_to_json(style: &CellStyle) -> Option<serde_json::Value> {
    if style == &CellStyle::default() {
        return None;
    }
    let mut obj = serde_json::Map::new();
    if style.bold {
        obj.insert("bold".to_string(), serde_json::Value::Bool(true));
    }
    if style.italic {
        obj.insert("italic".to_string(), serde_json::Value::Bool(true));
    }
    if style.underline {
        obj.insert("underline".to_string(), serde_json::Value::Bool(true));
    }
    if !style.number_format.is_empty() && style.number_format != "general" {
        obj.insert(
            "number_format".to_string(),
            serde_json::Value::String(style.number_format.clone()),
        );
    }
    Some(serde_json::Value::Object(obj))
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

fn style_from_json(v: &serde_json::Value) -> CellStyle {
    CellStyle {
        bold: v.get("bold").and_then(|b| b.as_bool()).unwrap_or(false),
        italic: v.get("italic").and_then(|b| b.as_bool()).unwrap_or(false),
        underline: v.get("underline").and_then(|b| b.as_bool()).unwrap_or(false),
        number_format: v
            .get("number_format")
            .and_then(|s| s.as_str())
            .unwrap_or("general")
            .to_string(),
    }
}

fn hydrate_workbook(
    content: &serde_json::Value,
) -> (
    Vec<String>,
    HashMap<(usize, u32, u32), String>,
    HashMap<(usize, u32, u32), CellStyle>,
) {
    let wb: WorkbookLite = match serde_json::from_value(content.clone()) {
        Ok(w) => w,
        Err(_) => return (Vec::new(), HashMap::new(), HashMap::new()),
    };
    let mut names = Vec::new();
    let mut cells = HashMap::new();
    let mut styles = HashMap::new();
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
                if let Some(style_json) = cell.get("style") {
                    let style = style_from_json(style_json);
                    if style != CellStyle::default() {
                        styles.insert((sheet_idx, row, col), style);
                    }
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
                if let Some(style_json) = cell.get("style") {
                    let style = style_from_json(style_json);
                    if style != CellStyle::default() {
                        styles.insert((sheet_idx, row, col), style);
                    }
                }
            }
        }
    }
    (names, cells, styles)
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
    let mut cell_styles = use_signal(|| HashMap::<(usize, u32, u32), CellStyle>::new());

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
            let (names, hydrated, styles) = hydrate_workbook(&doc.content);
            if !names.is_empty() {
                sheets.set(names);
            }
            cells.set(hydrated);
            cell_styles.set(styles);
            active_sheet.set(0);
            loaded.set(true);
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_workbook = move || -> serde_json::Value {
        let names = sheets.read().clone();
        let cells_ref = cells.read();
        let styles_ref = cell_styles.read();
        let mut per_sheet: Vec<Vec<serde_json::Value>> = names.iter().map(|_| Vec::new()).collect();
        let mut emitted: std::collections::HashSet<(usize, u32, u32)> =
            std::collections::HashSet::new();
        for ((si, r, c), value) in cells_ref.iter() {
            if *si >= per_sheet.len() {
                continue;
            }
            let cref = serde_json::json!({ "row": r, "col": c });
            let is_formula = value.starts_with('=');
            let mut cell = if is_formula {
                serde_json::json!({
                    "formula": value,
                    "value": { "type": "Empty" }
                })
            } else if let Ok(n) = value.parse::<f64>() {
                serde_json::json!({
                    "formula": serde_json::Value::Null,
                    "value": { "type": "Number", "value": n }
                })
            } else {
                serde_json::json!({
                    "formula": serde_json::Value::Null,
                    "value": { "type": "Text", "value": value }
                })
            };
            if let Some(style) = styles_ref.get(&(*si, *r, *c)) {
                if let Some(style_json) = style_to_json(style) {
                    if let Some(obj) = cell.as_object_mut() {
                        obj.insert("style".to_string(), style_json);
                    }
                }
            }
            per_sheet[*si].push(serde_json::json!([cref, cell]));
            emitted.insert((*si, *r, *c));
        }
        for ((si, r, c), style) in styles_ref.iter() {
            if *si >= per_sheet.len() {
                continue;
            }
            if emitted.contains(&(*si, *r, *c)) {
                continue;
            }
            let Some(style_json) = style_to_json(style) else { continue };
            let cref = serde_json::json!({ "row": r, "col": c });
            let cell = serde_json::json!({
                "formula": serde_json::Value::Null,
                "value": { "type": "Empty" },
                "style": style_json,
            });
            per_sheet[*si].push(serde_json::json!([cref, cell]));
        }
        let sheets_json: Vec<serde_json::Value> = names
            .iter()
            .enumerate()
            .map(|(i, name)| serde_json::json!({ "name": name, "cells": per_sheet[i] }))
            .collect();
        serde_json::json!({ "sheets": sheets_json })
    };

    let perform_save = {
        let id = id.clone();
        move |body: serde_json::Value, mut status: Signal<String>, token: String| {
            let id = id.clone();
            status.set("Saving…".to_string());
            wasm_bindgen_futures::spawn_local(async move {
                let req = serde_json::json!({ "content": body });
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token).await {
                    Ok(_) => status.set("All changes saved".to_string()),
                    Err(e) => status.set(format!("Save failed: {e}")),
                }
            });
        }
    };

    let schedule_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_workbook = serialize_workbook.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set("Unsaved changes".to_string());
            let body = serialize_workbook();
            let token = auth.read().token.clone().unwrap_or_default();
            let status = saved_status;
            let perform_save = perform_save.clone();
            let new_timer = Timeout::new(1500, move || {
                perform_save(body, status, token);
            });
            *save_timer.borrow_mut() = Some(new_timer);
        }
    };

    let flush_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_workbook = serialize_workbook.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            save_timer.borrow_mut().take();
            let body = serialize_workbook();
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    // ── Derived values ───────────────────────────────────────────────────
    let (active_row, active_col) = *active_cell.read();
    let sheet_idx = *active_sheet.read();

    // Helper: commit the current formula bar contents into the cell map.
    let commit_formula_bar = {
        let mut schedule_save = schedule_save.clone();
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
            schedule_save();
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
            onkeydown: {
                let mut schedule_save = schedule_save.clone();
                let mut flush_save = flush_save.clone();
                let mut commit_formula_bar = commit_formula_bar.clone();
                move |evt: KeyboardEvent| {
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
                        schedule_save();
                    }

                    // Ctrl+S — force-flush save
                    Key::Character(ref ch) if ch == "s" && modifiers.contains(Modifiers::CONTROL) => {
                        evt.prevent_default();
                        flush_save();
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
                }
            },

            // Global mouseup to stop drag-select even outside cells
            onmouseup: {
                let mut schedule_save = schedule_save.clone();
                move |_| {
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
                    schedule_save();
                    fill_mode.set(None);
                }
                }
            },

            // ── Title bar ────────────────────────────────────────────────
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: {
                        let mut schedule_save = schedule_save.clone();
                        move |e: FormEvent| {
                            doc_title.set(e.value());
                            schedule_save();
                        }
                    },
                    placeholder: "Untitled Spreadsheet",
                }
                span { class: "save-status", "{saved_status}" }
            }

            // ── Toolbar ──────────────────────────────────────────────────
            {
                let mut schedule_save_bold = schedule_save.clone();
                let mut schedule_save_italic = schedule_save.clone();
                let mut schedule_save_under = schedule_save.clone();
                let mut schedule_save_curr = schedule_save.clone();
                let mut schedule_save_pct = schedule_save.clone();
                let mut schedule_save_d0 = schedule_save.clone();
                let mut schedule_save_d2 = schedule_save.clone();
                let apply_toggle = move |which: u8| {
                    let active = *active_cell.read();
                    let sel_s = *selection_start.read();
                    let sel_e = *selection_end.read();
                    let extra = extra_selection.read().clone();
                    let targets = current_selection_cells(active, sel_s, sel_e, &extra);
                    let si = *active_sheet.read();
                    let all_on = targets.iter().all(|(r, c)| {
                        cell_styles
                            .read()
                            .get(&(si, *r, *c))
                            .map(|s| match which {
                                0 => s.bold,
                                1 => s.italic,
                                _ => s.underline,
                            })
                            .unwrap_or(false)
                    });
                    let mut w = cell_styles.write();
                    for (r, c) in targets {
                        let entry = w.entry((si, r, c)).or_default();
                        match which {
                            0 => entry.bold = !all_on,
                            1 => entry.italic = !all_on,
                            _ => entry.underline = !all_on,
                        }
                        if entry == &CellStyle::default() {
                            w.remove(&(si, r, c));
                        }
                    }
                };
                let apply_format = move |fmt: &'static str| {
                    let active = *active_cell.read();
                    let sel_s = *selection_start.read();
                    let sel_e = *selection_end.read();
                    let extra = extra_selection.read().clone();
                    let targets = current_selection_cells(active, sel_s, sel_e, &extra);
                    let si = *active_sheet.read();
                    let mut w = cell_styles.write();
                    for (r, c) in targets {
                        let entry = w.entry((si, r, c)).or_default();
                        entry.number_format = fmt.to_string();
                        if entry == &CellStyle::default() {
                            w.remove(&(si, r, c));
                        }
                    }
                };
                let apply_toggle_b = apply_toggle.clone();
                let apply_toggle_i = apply_toggle.clone();
                let apply_toggle_u = apply_toggle.clone();
                let apply_format_c = apply_format.clone();
                let apply_format_p = apply_format.clone();
                let apply_format_d0 = apply_format.clone();
                let apply_format_d2 = apply_format.clone();
                rsx! {
                    div { class: "toolbar",
                        div { class: "toolbar-group",
                            select { class: "toolbar-select font-family",
                                option { "Arial" }
                                option { "Times New Roman" }
                                option { "Courier New" }
                            }
                            select { class: "toolbar-select font-size",
                                option { value: "10", "10" }
                                option { value: "11", selected: true, "11" }
                                option { value: "12", "12" }
                                option { value: "14", "14" }
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn",
                                title: "Bold",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_b.clone();
                                    f(0);
                                    schedule_save_bold();
                                },
                                "B"
                            }
                            button {
                                class: "toolbar-btn italic",
                                title: "Italic",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_i.clone();
                                    f(1);
                                    schedule_save_italic();
                                },
                                "I"
                            }
                            button {
                                class: "toolbar-btn underline",
                                title: "Underline",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_u.clone();
                                    f(2);
                                    schedule_save_under();
                                },
                                "U"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn",
                                title: "Currency",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_c.clone();
                                    f("currency");
                                    schedule_save_curr();
                                },
                                "$"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Percent",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_p.clone();
                                    f("percent");
                                    schedule_save_pct();
                                },
                                "%"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Decrease Decimal",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_d0.clone();
                                    f("decimal0");
                                    schedule_save_d0();
                                },
                                ".0"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Increase Decimal",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_d2.clone();
                                    f("decimal2");
                                    schedule_save_d2();
                                },
                                ".00"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Merge Cells", "Merge" }
                            button { class: "toolbar-btn", title: "Cell Borders", "Borders" }
                            input { r#type: "color", class: "toolbar-color", title: "Fill Color", value: "#ffffff" }
                            input { r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000" }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Align Left", "L" }
                            button { class: "toolbar-btn", title: "Align Center", "C" }
                            button { class: "toolbar-btn", title: "Align Right", "R" }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Sort A-Z", "Sort" }
                            button { class: "toolbar-btn", title: "Filter", "Filter" }
                            button { class: "toolbar-btn", title: "Insert Function", "f(x)" }
                            button { class: "toolbar-btn", title: "Insert Chart", "Chart" }
                        }
                    }
                }
            }

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
                    onkeydown: {
                        let mut commit_formula_bar = commit_formula_bar.clone();
                        move |evt: KeyboardEvent| {
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
                                        let base_class = match (is_active, is_selected, fill_preview, editing) {
                                            (_, _, true, _) => "cell fill-preview",
                                            (_, _, _, true) => "cell active editing",
                                            (true, _, _, _) => "cell active",
                                            (false, true, _, _) => "cell selected",
                                            _ => "cell",
                                        };

                                        let si = sheet_idx;
                                        let cell_value = cells.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                        let style = cell_styles.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                        let mut style_classes = String::new();
                                        if style.bold { style_classes.push_str(" cell-bold"); }
                                        if style.italic { style_classes.push_str(" cell-italic"); }
                                        if style.underline { style_classes.push_str(" cell-underline"); }
                                        let class_str = format!("{base_class}{style_classes}");
                                        let display = if style.number_format.is_empty() || style.number_format == "general" {
                                            display_value(&cell_value).to_string()
                                        } else {
                                            format_number(&cell_value, &style.number_format)
                                        };
                                        let mut commit_formula_bar = commit_formula_bar.clone();

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
                        let mut commit_formula_bar = commit_formula_bar.clone();
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
