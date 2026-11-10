#[cfg(target_arch = "wasm32")]
use crate::collab_client::CollabHandle;
use crate::collab_client::ConnectionState;
#[cfg(target_arch = "wasm32")]
use crate::collab_client::{CollabMessage, connect_with_reconnect};
use crate::state::{AuthState, DocumentSave, api_get, export_document, save_document};
use gloo_timers::callback::Timeout;
use glory::prelude::*;
use glory::render::EventKind;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
    version: i32,
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
    #[serde(default)]
    merged_cells: Vec<MergedRangeLite>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct CellRefLite {
    row: u32,
    col: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct MergedRangeLite {
    start: CellRefLite,
    end: CellRefLite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MergedRange {
    start: (u32, u32),
    end: (u32, u32),
}

impl MergedRange {
    fn normalized(self) -> Self {
        Self {
            start: (self.start.0.min(self.end.0), self.start.1.min(self.end.1)),
            end: (self.start.0.max(self.end.0), self.start.1.max(self.end.1)),
        }
    }

    fn contains(self, cell: (u32, u32)) -> bool {
        in_selection(cell, self.start, self.end)
    }

    fn overlaps(self, other: Self) -> bool {
        let a = self.normalized();
        let b = other.normalized();
        a.start.0 <= b.end.0 && a.end.0 >= b.start.0 && a.start.1 <= b.end.1 && a.end.1 >= b.start.1
    }

    fn row_span(self) -> u32 {
        let range = self.normalized();
        range.end.0 - range.start.0 + 1
    }

    fn col_span(self) -> u32 {
        let range = self.normalized();
        range.end.1 - range.start.1 + 1
    }
}

fn merged_range_at(
    merged: &HashMap<usize, Vec<MergedRange>>,
    sheet: usize,
    cell: (u32, u32),
) -> Option<MergedRange> {
    merged
        .get(&sheet)?
        .iter()
        .copied()
        .find(|range| range.contains(cell))
        .map(MergedRange::normalized)
}

fn sanitize_merged_ranges(ranges: impl IntoIterator<Item = MergedRange>) -> Vec<MergedRange> {
    let mut sanitized: Vec<MergedRange> = Vec::new();
    for range in ranges {
        let range = range.normalized();
        if range.start == range.end
            || range.end.0 >= NUM_ROWS
            || range.end.1 >= NUM_COLS
            || sanitized.iter().any(|existing| existing.overlaps(range))
        {
            continue;
        }
        sanitized.push(range);
    }
    sanitized
}

fn merged_range_overlaps(
    merged: &HashMap<usize, Vec<MergedRange>>,
    sheet: usize,
    start: (u32, u32),
    end: (u32, u32),
) -> bool {
    let requested = MergedRange { start, end }.normalized();
    merged
        .get(&sheet)
        .is_some_and(|ranges| ranges.iter().any(|range| range.overlaps(requested)))
}

#[cfg(test)]
fn merged_column_overlaps(
    merged: &HashMap<usize, Vec<MergedRange>>,
    sheet: usize,
    column: u32,
) -> bool {
    merged_range_overlaps(merged, sheet, (0, column), (NUM_ROWS - 1, column))
}

#[cfg(any(test, target_arch = "wasm32"))]
fn workbook_replace_from_operation(
    payload: &serde_json::Value,
) -> Option<(String, serde_json::Value)> {
    let user_id = payload.get("user_id")?.as_str()?.to_string();
    let op_type = payload.get("op_type")?;
    if op_type.get("type")?.as_str()? != "Replace" {
        return None;
    }
    let path = op_type.get("path")?.as_array()?;
    if path.len() != 1 || path[0].as_str()? != "workbook" {
        return None;
    }
    Some((user_id, op_type.get("new_value")?.clone()))
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq)]
struct CellPatch {
    sheet: usize,
    row: u32,
    col: u32,
    old_value: serde_json::Value,
    new_value: serde_json::Value,
}

fn workbook_cells(value: &serde_json::Value) -> HashMap<(usize, u32, u32), serde_json::Value> {
    workbook_cell_entries(value)
        .map(|(key, cell)| (key, cell.clone()))
        .collect()
}

fn workbook_cell_entries(
    value: &serde_json::Value,
) -> impl Iterator<Item = (CellKey, &serde_json::Value)> {
    value
        .get("sheets")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .flat_map(|(sheet_index, sheet)| {
            sheet["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |pair| {
                    let pair = pair.as_array().filter(|pair| pair.len() == 2)?;
                    let row = pair[0]["row"].as_u64().unwrap_or(0) as u32;
                    let col = pair[0]["col"].as_u64().unwrap_or(0) as u32;
                    Some(((sheet_index, row, col), &pair[1]))
                })
        })
}

#[cfg(test)]
fn workbook_without_cells(value: &serde_json::Value) -> serde_json::Value {
    let mut stripped = value.clone();
    if let Some(sheets) = stripped
        .get_mut("sheets")
        .and_then(|value| value.as_array_mut())
    {
        for sheet in sheets {
            if let Some(object) = sheet.as_object_mut() {
                object.insert("cells".into(), serde_json::Value::Array(Vec::new()));
            }
        }
    }
    stripped
}

#[cfg(test)]
fn single_cell_patch(old: &serde_json::Value, new: &serde_json::Value) -> Option<CellPatch> {
    if workbook_without_cells(old) != workbook_without_cells(new) {
        return None;
    }
    let old_cells = workbook_cells(old);
    let new_cells = workbook_cells(new);
    let mut keys: std::collections::HashSet<(usize, u32, u32)> =
        old_cells.keys().chain(new_cells.keys()).copied().collect();
    let mut changed = keys
        .drain()
        .filter(|key| old_cells.get(key) != new_cells.get(key));
    let (sheet, row, col) = changed.next()?;
    if changed.next().is_some() {
        return None;
    }
    Some(CellPatch {
        sheet,
        row,
        col,
        old_value: old_cells
            .get(&(sheet, row, col))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        new_value: new_cells
            .get(&(sheet, row, col))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

#[cfg(any(test, target_arch = "wasm32"))]
fn apply_cell_patch(content: &mut serde_json::Value, patch: &CellPatch) -> bool {
    let Some(cells) = content
        .get_mut("sheets")
        .and_then(|value| value.as_array_mut())
        .and_then(|sheets| sheets.get_mut(patch.sheet))
        .and_then(|sheet| sheet.get_mut("cells"))
        .and_then(|cells| cells.as_array_mut())
    else {
        return false;
    };
    let existing = cells.iter().position(|pair| {
        pair.as_array().is_some_and(|pair| {
            pair.len() == 2
                && pair[0].get("row").and_then(|value| value.as_u64()) == Some(patch.row as u64)
                && pair[0].get("col").and_then(|value| value.as_u64()) == Some(patch.col as u64)
        })
    });
    if patch.new_value.is_null() {
        if let Some(index) = existing {
            cells.remove(index);
        }
    } else {
        let pair = serde_json::json!([
            { "row": patch.row, "col": patch.col },
            patch.new_value.clone()
        ]);
        if let Some(index) = existing {
            cells[index] = pair;
        } else {
            cells.push(pair);
        }
    }
    true
}

#[cfg(any(test, target_arch = "wasm32"))]
fn cell_patch_from_operation(payload: &serde_json::Value) -> Option<(String, CellPatch)> {
    let user_id = payload.get("user_id")?.as_str()?.to_string();
    let op_type = payload.get("op_type")?;
    if op_type.get("type")?.as_str()? != "SetProperty" {
        return None;
    }
    let path = op_type.get("path")?.as_array()?;
    if path.len() != 4
        || path[0].as_str()? != "workbook"
        || path[1].as_str()? != "sheets"
        || path[3].as_str()? != "cells"
    {
        return None;
    }
    let sheet = path[2].as_str()?.parse().ok()?;
    let (row, col) = op_type.get("key")?.as_str()?.split_once(':')?;
    Some((
        user_id,
        CellPatch {
            sheet,
            row: row.parse().ok()?,
            col: col.parse().ok()?,
            old_value: op_type
                .get("old_value")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            new_value: op_type.get("value")?.clone(),
        },
    ))
}

fn append_array_entry(object: &mut serde_json::Value, key: &str, entry: serde_json::Value) -> bool {
    let Some(object) = object.as_object_mut() else {
        return false;
    };
    let Some(array) = object
        .entry(key)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
    else {
        return false;
    };
    array.push(entry);
    true
}

fn resolve_named_range(content: &serde_json::Value, name: &str) -> Option<(usize, MergedRange)> {
    let range = content["named_ranges"].as_array()?.iter().find(|range| {
        range["name"]
            .as_str()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(name.trim()))
    })?;
    let sheet = content["sheets"]
        .as_array()?
        .iter()
        .position(|sheet| sheet["id"] == range["sheet_id"])?;
    let coord = |value: &serde_json::Value| {
        let row = u32::try_from(value["row"].as_u64()?).ok()?;
        let col = u32::try_from(value["col"].as_u64()?).ok()?;
        (row < NUM_ROWS && col < NUM_COLS).then_some((row, col))
    };
    Some((
        sheet,
        MergedRange {
            start: coord(&range["start"])?,
            end: coord(&range["end"])?,
        }
        .normalized(),
    ))
}

fn pivot_sum_rows(
    cells: &HashMap<(usize, u32, u32), String>,
    sheet: usize,
    start: (u32, u32),
    end: (u32, u32),
) -> Vec<(String, f64)> {
    let mut groups = std::collections::BTreeMap::<String, f64>::new();
    for row in start.0..=end.0 {
        let label = cells
            .get(&(sheet, row, start.1))
            .cloned()
            .unwrap_or_default();
        let value = cells
            .get(&(sheet, row, (start.1 + 1).min(end.1)))
            .and_then(|value| value.parse::<f64>().ok());
        if let Some(value) = value {
            *groups.entry(label).or_default() += value;
        }
    }
    groups.into_iter().collect()
}

fn conditional_style_for(
    content: &serde_json::Value,
    sheet: usize,
    row: u32,
    col: u32,
    raw_value: &str,
) -> Option<CellStyle> {
    let rules = content
        .get("sheets")?
        .as_array()?
        .get(sheet)?
        .get("conditional_formats")?
        .as_array()?;
    rules.iter().rev().find_map(|rule| {
        let start = rule.get("range_start")?;
        let end = rule.get("range_end")?;
        let inside = row >= start.get("row")?.as_u64()? as u32
            && row <= end.get("row")?.as_u64()? as u32
            && col >= start.get("col")?.as_u64()? as u32
            && col <= end.get("col")?.as_u64()? as u32;
        if !inside {
            return None;
        }
        let predicate = rule.get("rule")?;
        let number = raw_value.parse::<f64>().ok();
        let matches = if let Some(limit) = predicate.get("GreaterThan").and_then(|v| v.as_f64()) {
            number.is_some_and(|value| value > limit)
        } else if let Some(limit) = predicate.get("LessThan").and_then(|v| v.as_f64()) {
            number.is_some_and(|value| value < limit)
        } else if let Some(bounds) = predicate.get("Between").and_then(|v| v.as_array()) {
            bounds.len() == 2
                && number.is_some_and(|value| {
                    value >= bounds[0].as_f64().unwrap_or(f64::INFINITY)
                        && value <= bounds[1].as_f64().unwrap_or(f64::NEG_INFINITY)
                })
        } else if let Some(expected) = predicate.get("EqualTo").and_then(|v| v.as_str()) {
            raw_value == expected
        } else if let Some(fragment) = predicate.get("ContainsText").and_then(|v| v.as_str()) {
            raw_value.contains(fragment)
        } else {
            predicate.as_str() == Some("IsEmpty") && raw_value.is_empty()
                || predicate.as_str() == Some("IsNotEmpty") && !raw_value.is_empty()
        };
        matches.then(|| rule.get("format").map(style_from_json).unwrap_or_default())
    })
}

fn validation_error_for(
    content: &serde_json::Value,
    sheet: usize,
    row: u32,
    col: u32,
    raw_value: &str,
) -> Option<String> {
    let validations = content
        .get("sheets")?
        .as_array()?
        .get(sheet)?
        .get("data_validations")?
        .as_array()?;
    for validation in validations {
        let start = validation.get("range_start")?;
        let end = validation.get("range_end")?;
        if row < start.get("row")?.as_u64()? as u32
            || row > end.get("row")?.as_u64()? as u32
            || col < start.get("col")?.as_u64()? as u32
            || col > end.get("col")?.as_u64()? as u32
        {
            continue;
        }
        let rule = validation.get("rule")?;
        let accepted = if let Some(options) = rule.get("List").and_then(|v| v.as_array()) {
            options
                .iter()
                .any(|value| value.as_str() == Some(raw_value))
        } else if let Some(bounds) = rule.get("WholeNumberBetween").and_then(|v| v.as_array()) {
            raw_value.parse::<i64>().ok().is_some_and(|value| {
                bounds.len() == 2
                    && value >= bounds[0].as_i64().unwrap_or(i64::MAX)
                    && value <= bounds[1].as_i64().unwrap_or(i64::MIN)
            })
        } else if let Some(bounds) = rule.get("DecimalBetween").and_then(|v| v.as_array()) {
            raw_value.parse::<f64>().ok().is_some_and(|value| {
                bounds.len() == 2
                    && value >= bounds[0].as_f64().unwrap_or(f64::INFINITY)
                    && value <= bounds[1].as_f64().unwrap_or(f64::NEG_INFINITY)
            })
        } else {
            true
        };
        if !accepted {
            return Some(
                validation
                    .get("error_message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Cell value violates data validation")
                    .to_string(),
            );
        }
    }
    None
}

fn insert_row_in_merged(merged: &mut HashMap<usize, Vec<MergedRange>>, sheet: usize, at: u32) {
    if let Some(ranges) = merged.get_mut(&sheet) {
        *ranges = ranges
            .iter()
            .copied()
            .filter_map(|range| {
                let mut range = range.normalized();
                if at <= range.start.0 {
                    range.start.0 += 1;
                    range.end.0 += 1;
                } else if at <= range.end.0 {
                    range.end.0 += 1;
                }
                (range.end.0 < NUM_ROWS).then_some(range)
            })
            .collect();
    }
}

fn delete_row_from_merged(merged: &mut HashMap<usize, Vec<MergedRange>>, sheet: usize, at: u32) {
    if let Some(ranges) = merged.get_mut(&sheet) {
        *ranges = ranges
            .iter()
            .copied()
            .filter_map(|range| {
                let mut range = range.normalized();
                if at < range.start.0 {
                    range.start.0 -= 1;
                    range.end.0 -= 1;
                } else if at <= range.end.0 {
                    if range.start.0 == range.end.0 {
                        return None;
                    }
                    range.end.0 -= 1;
                }
                (range.start != range.end).then_some(range)
            })
            .collect();
    }
}

fn insert_col_in_merged(merged: &mut HashMap<usize, Vec<MergedRange>>, sheet: usize, at: u32) {
    if let Some(ranges) = merged.get_mut(&sheet) {
        *ranges = ranges
            .iter()
            .copied()
            .filter_map(|range| {
                let mut range = range.normalized();
                if at <= range.start.1 {
                    range.start.1 += 1;
                    range.end.1 += 1;
                } else if at <= range.end.1 {
                    range.end.1 += 1;
                }
                (range.end.1 < NUM_COLS).then_some(range)
            })
            .collect();
    }
}

fn delete_col_from_merged(merged: &mut HashMap<usize, Vec<MergedRange>>, sheet: usize, at: u32) {
    if let Some(ranges) = merged.get_mut(&sheet) {
        *ranges = ranges
            .iter()
            .copied()
            .filter_map(|range| {
                let mut range = range.normalized();
                if at < range.start.1 {
                    range.start.1 -= 1;
                    range.end.1 -= 1;
                } else if at <= range.end.1 {
                    if range.start.1 == range.end.1 {
                        return None;
                    }
                    range.end.1 -= 1;
                }
                (range.start != range.end).then_some(range)
            })
            .collect();
    }
}

fn delete_sheet_from_merged(merged: &mut HashMap<usize, Vec<MergedRange>>, sheet: usize) {
    let old = std::mem::take(merged);
    for (index, ranges) in old {
        if index == sheet {
            continue;
        }
        merged.insert(if index > sheet { index - 1 } else { index }, ranges);
    }
}

fn swap_sheets_in_merged(
    merged: &mut HashMap<usize, Vec<MergedRange>>,
    first: usize,
    second: usize,
) {
    let first_ranges = merged.remove(&first);
    let second_ranges = merged.remove(&second);
    if let Some(ranges) = first_ranges {
        merged.insert(second, ranges);
    }
    if let Some(ranges) = second_ranges {
        merged.insert(first, ranges);
    }
}

fn parse_cell_ref(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().to_uppercase();
    let mut col = 0u32;
    let mut row_start = 0usize;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_alphabetic() {
            col = col * 26 + (ch as u32 - 'A' as u32 + 1);
        } else {
            row_start = i;
            break;
        }
    }
    if row_start == 0 {
        return None;
    }
    let row: u32 = s[row_start..].parse().ok()?;
    if row == 0 || col == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CellBorders {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CellStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    number_format: String,
    borders: CellBorders,
    /// Horizontal alignment: "", "left", "center" or "right".
    align: String,
    /// Background colour as a CSS hex string (empty = none).
    fill_color: String,
    /// Text colour as a CSS hex string (empty = default).
    text_color: String,
    /// Font family name (empty = default).
    font_family: String,
    /// Font size in points as a string (empty = default).
    font_size: String,
}

#[derive(Clone, PartialEq)]
struct CalcSnapshot {
    cells: HashMap<(usize, u32, u32), String>,
    styles: HashMap<(usize, u32, u32), CellStyle>,
    merged_cells: HashMap<usize, Vec<MergedRange>>,
    sheets: Vec<String>,
}

type CellKey = (usize, u32, u32);
type HydratedWorkbook = (
    Vec<String>,
    HashMap<CellKey, String>,
    HashMap<CellKey, CellStyle>,
    HashMap<usize, Vec<MergedRange>>,
);
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
    (style != &CellStyle::default()).then(|| style_to_value(style))
}

fn style_to_value(style: &CellStyle) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("bold".to_string(), serde_json::Value::Bool(style.bold));
    obj.insert("italic".to_string(), serde_json::Value::Bool(style.italic));
    obj.insert(
        "underline".to_string(),
        serde_json::Value::Bool(style.underline),
    );
    obj.insert(
        "font_family".to_string(),
        if !style.font_family.is_empty() {
            serde_json::Value::String(style.font_family.clone())
        } else {
            serde_json::Value::Null
        },
    );
    obj.insert(
        "font_size".to_string(),
        style
            .font_size
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
    );
    obj.insert(
        "text_color".to_string(),
        if !style.text_color.is_empty() {
            serde_json::Value::String(style.text_color.clone())
        } else {
            serde_json::Value::Null
        },
    );
    obj.insert(
        "bg_color".to_string(),
        if !style.fill_color.is_empty() {
            serde_json::Value::String(style.fill_color.clone())
        } else {
            serde_json::Value::Null
        },
    );
    obj.insert(
        "horizontal_align".to_string(),
        serde_json::Value::String(
            match style.align.as_str() {
                "center" => "Center",
                "right" => "Right",
                _ => "Left",
            }
            .to_string(),
        ),
    );
    obj.insert(
        "vertical_align".to_string(),
        serde_json::Value::String("Bottom".into()),
    );
    obj.insert(
        "number_format".to_string(),
        if !style.number_format.is_empty() && style.number_format != "general" {
            serde_json::Value::String(style.number_format.clone())
        } else {
            serde_json::Value::Null
        },
    );
    let b = &style.borders;
    if b.top || b.right || b.bottom || b.left {
        let edge = || serde_json::json!({ "style": "Thin", "color": "#000000" });
        obj.insert(
            "borders".to_string(),
            serde_json::json!({
                "top": b.top.then(edge),
                "right": b.right.then(edge),
                "bottom": b.bottom.then(edge),
                "left": b.left.then(edge),
            }),
        );
    } else {
        obj.insert(
            "borders".to_string(),
            serde_json::json!({ "top": null, "right": null, "bottom": null, "left": null }),
        );
    }
    obj.insert("wrap_text".to_string(), serde_json::Value::Bool(false));
    serde_json::Value::Object(obj)
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
        "Error" => format!("#ERR: {}", inner.and_then(|x| x.as_str()).unwrap_or("")),
        _ => String::new(),
    }
}

fn style_from_json(v: &serde_json::Value) -> CellStyle {
    let borders = v
        .get("borders")
        .map(|b| CellBorders {
            top: b
                .get("top")
                .is_some_and(|x| !x.is_null() && x.as_bool() != Some(false)),
            right: b
                .get("right")
                .is_some_and(|x| !x.is_null() && x.as_bool() != Some(false)),
            bottom: b
                .get("bottom")
                .is_some_and(|x| !x.is_null() && x.as_bool() != Some(false)),
            left: b
                .get("left")
                .is_some_and(|x| !x.is_null() && x.as_bool() != Some(false)),
        })
        .unwrap_or_default();
    CellStyle {
        bold: v.get("bold").and_then(|b| b.as_bool()).unwrap_or(false),
        italic: v.get("italic").and_then(|b| b.as_bool()).unwrap_or(false),
        underline: v
            .get("underline")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        number_format: v
            .get("number_format")
            .and_then(|s| s.as_str())
            .unwrap_or("general")
            .to_string(),
        borders,
        align: v
            .get("horizontal_align")
            .and_then(|value| value.as_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| string_field(v, "align")),
        fill_color: v
            .get("bg_color")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| {
                v.get("fill_color")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
            })
            .to_string(),
        text_color: string_field(v, "text_color"),
        font_family: string_field(v, "font_family"),
        font_size: v
            .get("font_size")
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| value.as_f64().map(|value| value.to_string()))
            })
            .unwrap_or_default(),
    }
}

fn string_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

fn hydrate_workbook(content: &serde_json::Value) -> HydratedWorkbook {
    let wb: WorkbookLite = match serde_json::from_value(content.clone()) {
        Ok(w) => w,
        Err(_) => {
            return (Vec::new(), HashMap::new(), HashMap::new(), HashMap::new());
        }
    };
    let mut names = Vec::new();
    let mut cells = HashMap::new();
    let mut styles = HashMap::new();
    let mut merged_cells = HashMap::new();
    for (sheet_idx, sheet) in wb.sheets.iter().enumerate() {
        names.push(sheet.name.clone());
        let ranges = sanitize_merged_ranges(sheet.merged_cells.iter().map(|range| MergedRange {
            start: (range.start.row, range.start.col),
            end: (range.end.row, range.end.col),
        }));
        if !ranges.is_empty() {
            merged_cells.insert(sheet_idx, ranges);
        }
        // `cells` may be either a HashMap object {"row,col": Cell} OR
        // a JSON object whose keys are JSON-encoded CellRef. Serde JSON
        // serializes HashMap<CellRef, Cell> as an array of [CellRef, Cell]
        // pairs because CellRef is not a string. Handle both shapes.
        if let Some(arr) = sheet.cells.as_array() {
            for pair in arr {
                let Some(pair_arr) = pair.as_array() else {
                    continue;
                };
                if pair_arr.len() != 2 {
                    continue;
                }
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
                    if f.starts_with('=') {
                        f
                    } else {
                        format!("={f}")
                    }
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
                if let Some(style_json) = cell.get("format").or_else(|| cell.get("style")) {
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
                    if f.starts_with('=') {
                        f
                    } else {
                        format!("={f}")
                    }
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
                if let Some(style_json) = cell.get("format").or_else(|| cell.get("style")) {
                    let style = style_from_json(style_json);
                    if style != CellStyle::default() {
                        styles.insert((sheet_idx, row, col), style);
                    }
                }
            }
        }
    }
    (names, cells, styles, merged_cells)
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

fn format_number_default(n: f64) -> String {
    if n.is_nan() {
        "#ERR".to_string()
    } else if n == 0.0 {
        "0".to_string()
    } else if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

// Legacy arithmetic scenarios exercise the shared engine used by the browser.
#[cfg(test)]
mod eval {
    use contrix_doc_engine::calc::{CellValue, Sheet};
    use std::collections::HashMap;
    #[derive(Debug, PartialEq)]
    pub enum EvalError {
        Cycle,
        DivByZero,
        Error(String),
    }
    pub fn evaluate(
        formula: &str,
        sheet_idx: usize,
        cells: &HashMap<(usize, u32, u32), String>,
    ) -> Result<f64, EvalError> {
        let mut sheet = Sheet::new(String::new());
        for ((si, row, col), raw) in cells {
            if *si != sheet_idx {
                continue;
            }
            if raw.starts_with('=') {
                sheet.set_formula(*row, *col, raw.clone());
            } else if let Ok(value) = raw.parse::<f64>() {
                sheet.set_cell(*row, *col, CellValue::Number(value));
            } else {
                sheet.set_cell(*row, *col, CellValue::Text(raw.clone()));
            }
        }
        match sheet.evaluate_formula(formula) {
            CellValue::Number(value) => Ok(value),
            CellValue::Empty => Ok(0.0),
            CellValue::Boolean(value) => Ok(if value { 1.0 } else { 0.0 }),
            CellValue::Error(error) if error.contains("Circular") => Err(EvalError::Cycle),
            CellValue::Error(error) if error.contains("DIV/0") => Err(EvalError::DivByZero),
            CellValue::Error(error) | CellValue::Text(error) | CellValue::Date(error) => {
                Err(EvalError::Error(error))
            }
        }
    }
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

mod fill;
mod glory_calc;
pub use glory_calc::Calc;

#[cfg(test)]
mod eval_tests {
    use super::eval::{EvalError, evaluate};
    use super::{
        MergedRange, apply_cell_patch, delete_col_from_merged, delete_row_from_merged,
        hydrate_workbook, insert_col_in_merged, insert_row_in_merged, merged_column_overlaps,
        pivot_sum_rows, sanitize_merged_ranges, single_cell_patch, workbook_replace_from_operation,
    };
    use std::collections::HashMap;

    fn make(items: &[((usize, u32, u32), &str)]) -> HashMap<(usize, u32, u32), String> {
        items.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn plain_arithmetic() {
        let cells = HashMap::new();
        assert_eq!(evaluate("1+2", 0, &cells).unwrap(), 3.0);
    }

    #[test]
    fn precedence_and_parens() {
        let cells = HashMap::new();
        assert_eq!(evaluate("1+2*3", 0, &cells).unwrap(), 7.0);
        assert_eq!(evaluate("(1+2)*3", 0, &cells).unwrap(), 9.0);
    }

    #[test]
    fn cell_ref_addition() {
        let cells = make(&[((0, 0, 0), "5"), ((0, 0, 1), "7")]);
        assert_eq!(evaluate("A1+B1", 0, &cells).unwrap(), 12.0);
    }

    #[test]
    fn sum_range() {
        let cells = make(&[((0, 0, 0), "1"), ((0, 1, 0), "2"), ((0, 2, 0), "3")]);
        assert_eq!(evaluate("SUM(A1:A3)", 0, &cells).unwrap(), 6.0);
    }

    #[test]
    fn avg_range() {
        let cells = make(&[((0, 0, 0), "10"), ((0, 1, 0), "20")]);
        assert_eq!(evaluate("AVG(A1:A2)", 0, &cells).unwrap(), 15.0);
    }

    #[test]
    fn if_truthy_and_falsy() {
        let cells_t = make(&[((0, 0, 0), "1")]);
        assert_eq!(evaluate("IF(A1, 100, 200)", 0, &cells_t).unwrap(), 100.0);
        let cells_f = make(&[((0, 0, 0), "0")]);
        assert_eq!(evaluate("IF(A1, 100, 200)", 0, &cells_f).unwrap(), 200.0);
    }

    #[test]
    fn cycle_detection() {
        let cells = make(&[((0, 0, 0), "=B1"), ((0, 0, 1), "=A1")]);
        let res = evaluate("A1", 0, &cells);
        assert!(matches!(res, Err(EvalError::Cycle)));
    }

    #[test]
    fn min_max_count() {
        let cells = make(&[
            ((0, 0, 0), "3"),
            ((0, 1, 0), "1"),
            ((0, 2, 0), "9"),
            ((0, 3, 0), "hello"),
        ]);
        assert_eq!(evaluate("MIN(A1:A4)", 0, &cells).unwrap(), 1.0);
        assert_eq!(evaluate("MAX(A1:A4)", 0, &cells).unwrap(), 9.0);
        assert_eq!(evaluate("COUNT(A1:A4)", 0, &cells).unwrap(), 3.0);
    }

    #[test]
    fn unary_and_div_by_zero() {
        let cells = HashMap::new();
        assert_eq!(evaluate("-2+5", 0, &cells).unwrap(), 3.0);
        assert!(matches!(
            evaluate("1/0", 0, &cells),
            Err(EvalError::DivByZero)
        ));
    }

    #[test]
    fn dollar_absolute_refs_ignored() {
        let cells = make(&[((0, 0, 0), "5")]);
        assert_eq!(evaluate("$A$1+1", 0, &cells).unwrap(), 6.0);
    }

    #[test]
    fn hydrate_workbook_preserves_merged_ranges() {
        let content = serde_json::json!({
            "sheets": [{
                "name": "Sheet1",
                "cells": [],
                "merged_cells": [{
                    "start": {"row": 3, "col": 4},
                    "end": {"row": 1, "col": 2}
                }]
            }]
        });

        let (_, _, _, merged) = hydrate_workbook(&content);
        assert_eq!(
            merged.get(&0),
            Some(&vec![MergedRange {
                start: (1, 2),
                end: (3, 4),
            }])
        );
    }

    #[test]
    fn merged_ranges_follow_row_and_column_edits() {
        let mut merged = HashMap::from([(
            0,
            vec![MergedRange {
                start: (1, 1),
                end: (2, 2),
            }],
        )]);

        insert_row_in_merged(&mut merged, 0, 2);
        insert_col_in_merged(&mut merged, 0, 2);
        assert_eq!(
            merged[&0][0],
            MergedRange {
                start: (1, 1),
                end: (3, 3),
            }
        );

        delete_row_from_merged(&mut merged, 0, 1);
        delete_col_from_merged(&mut merged, 0, 1);
        assert_eq!(
            merged[&0][0],
            MergedRange {
                start: (1, 1),
                end: (2, 2),
            }
        );
    }

    #[test]
    fn merged_range_sanitization_rejects_overlap_and_out_of_bounds() {
        let valid = MergedRange {
            start: (1, 1),
            end: (2, 2),
        };
        let sanitized = sanitize_merged_ranges([
            valid,
            MergedRange {
                start: (2, 2),
                end: (3, 3),
            },
            MergedRange {
                start: (99, 0),
                end: (100, 1),
            },
            MergedRange {
                start: (4, 4),
                end: (4, 4),
            },
        ]);
        assert_eq!(sanitized, vec![valid]);
    }

    #[test]
    fn extracts_workbook_collaboration_replacement() {
        let operation = serde_json::json!({
            "user_id": "user-1",
            "op_type": {
                "type": "Replace",
                "path": ["workbook"],
                "old_value": null,
                "new_value": {"sheets": []}
            }
        });
        let (user_id, workbook) = workbook_replace_from_operation(&operation).unwrap();
        assert_eq!(user_id, "user-1");
        assert_eq!(workbook, serde_json::json!({"sheets": []}));
    }

    #[test]
    fn collaboration_uses_and_applies_a_single_cell_patch() {
        let old = serde_json::json!({"sheets":[{"name":"Sheet1","cells":[]}]});
        let new = serde_json::json!({"sheets":[{"name":"Sheet1","cells":[[{"row":2,"col":3},{"value":{"type":"Text","value":"hello"},"formula":null}]]}]});
        let patch = single_cell_patch(&old, &new).expect("one cell changed");
        assert_eq!((patch.sheet, patch.row, patch.col), (0, 2, 3));
        let mut applied = old;
        assert!(apply_cell_patch(&mut applied, &patch));
        assert_eq!(applied, new);
    }

    #[test]
    fn editor_cells_are_accepted_by_the_export_engine() {
        let mut content =
            serde_json::to_value(contrix_doc_engine::Workbook::new("Book".into())).unwrap();
        let style = super::CellStyle {
            bold: true,
            ..Default::default()
        };
        content["sheets"][0]["cells"] = serde_json::json!([
            [{"row":0,"col":0},{"value":{"type":"Text","value":"Header"},"formula":null,"format":super::style_to_json(&style)}],
            [{"row":1,"col":0},{"value":{"type":"Number","value":42.0},"formula":null}]
        ]);
        let workbook: contrix_doc_engine::Workbook = serde_json::from_value(content).unwrap();
        assert!(workbook.sheets[0].get_cell(0, 0).unwrap().format.bold);
        let roundtrip = serde_json::to_value(workbook).unwrap();
        assert_eq!(hydrate_workbook(&roundtrip).1[&(0, 1, 0)], "42");
    }

    #[test]
    fn name_box_resolves_named_ranges_to_their_sheet_and_selection() {
        let content = serde_json::json!({"sheets":[{"id":"first"},{"id":"second"}],"named_ranges":[{
            "name":"Sales","sheet_id":"second","start":{"row":1,"col":2},"end":{"row":5,"col":3}
        }]});
        let (sheet, range) = super::resolve_named_range(&content, " sales ").unwrap();
        assert_eq!(sheet, 1);
        assert_eq!(range.start, (1, 2));
        assert_eq!(range.end, (5, 3));
        assert!(super::resolve_named_range(&content, "unknown").is_none());
    }

    #[test]
    fn pivot_rows_are_grouped_and_summed() {
        let values = make(&[
            ((0, 0, 0), "Region"),
            ((0, 0, 1), "Sales"),
            ((0, 1, 0), "East"),
            ((0, 1, 1), "10"),
            ((0, 2, 0), "East"),
            ((0, 2, 1), "5"),
            ((0, 3, 0), "West"),
            ((0, 3, 1), "7"),
        ]);
        assert_eq!(
            pivot_sum_rows(&values, 0, (0, 0), (3, 1)),
            vec![("East".into(), 15.0), ("West".into(), 7.0)]
        );
    }

    #[test]
    fn merged_column_overlap_uses_column_coordinates() {
        let mut merged = HashMap::new();
        merged.insert(
            0,
            vec![MergedRange {
                start: (5, 2),
                end: (6, 3),
            }],
        );
        assert!(merged_column_overlaps(&merged, 0, 2));
        assert!(merged_column_overlaps(&merged, 0, 3));
        assert!(!merged_column_overlaps(&merged, 0, 4));
    }
}
