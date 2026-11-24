//! Application-owned workbook state and semantic Glory/Sapid presentation.
use super::*;
#[path = "calculation.rs"]
mod calculation;
use calculation::{CalculationCell, CalculationInput};
#[path = "snapshot_merge.rs"]
mod snapshot_merge;

#[cfg(target_arch = "wasm32")]
#[path = "calculation_worker.rs"]
mod calculation_worker;
#[cfg(target_arch = "wasm32")]
#[path = "cell_events.rs"]
mod cell_events;
#[cfg(target_arch = "wasm32")]
#[path = "clipboard_events.rs"]
mod clipboard_events;
#[cfg(target_arch = "wasm32")]
#[path = "pointer_events.rs"]
mod pointer_events;

fn cell_input_text(cell: &serde_json::Value) -> String {
    if let Some(formula) = cell["formula"].as_str() {
        if formula.starts_with('=') {
            formula.into()
        } else {
            format!("={formula}")
        }
    } else {
        cell_value_to_string(&cell["value"])
    }
}
fn calculated_text(value: &syntra_doc_engine::calc::CellValue) -> String {
    use syntra_doc_engine::calc::CellValue;
    match value {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => format_number_default(*n),
        CellValue::Boolean(v) => if *v { "TRUE" } else { "FALSE" }.into(),
        CellValue::Text(s) | CellValue::Date(s) => s.clone(),
        CellValue::Error(s) => format!("#ERR: {s}"),
    }
}

/// Apply only properties represented and changed in the editing surface.
fn merge_visible_delta(
    target: &mut serde_json::Value,
    before: &serde_json::Value,
    after: &serde_json::Value,
) {
    if before == after {
        return;
    }
    if let (Some(old), Some(new)) = (before.as_object(), after.as_object()) {
        if !target.is_object() {
            *target = serde_json::json!({});
        }
        for (key, value) in new {
            let previous = old.get(key).unwrap_or(&serde_json::Value::Null);
            // IndexMut inserts Null for a missing key. Do not touch unchanged
            // properties: bool/enumeration fields must retain serde defaults.
            if previous != value {
                merge_visible_delta(&mut target[key], previous, value);
            }
        }
    } else {
        *target = after.clone();
    }
}

type CalculatedCells = HashMap<CellKey, syntra_doc_engine::calc::CellValue>;

#[derive(Clone)]
struct CalculationCache {
    values: CalculatedCells,
    #[cfg(target_arch = "wasm32")]
    pending: Option<Rc<CalculationInput>>,
}

#[derive(Clone)]
struct Book {
    snapshot: CalcSnapshot,
    content: serde_json::Value,
    baseline: serde_json::Value,
    undo: Vec<(CalcSnapshot, serde_json::Value)>,
    redo: Vec<(CalcSnapshot, serde_json::Value)>,
    revision: u64,
    dirty: bool,
    calculation: RefCell<Option<CalculationCache>>,
    conflict: Option<serde_json::Value>,
}
impl Default for Book {
    fn default() -> Self {
        Self {
            snapshot: CalcSnapshot {
                cells: HashMap::new(),
                styles: HashMap::new(),
                merged_cells: HashMap::new(),
                sheets: vec!["Sheet1".into()],
            },
            content: serde_json::json!({"sheets": []}),
            baseline: serde_json::Value::Null,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            dirty: false,
            calculation: RefCell::new(None),
            conflict: None,
        }
    }
}
impl Book {
    fn hydrate(&mut self, content: serde_json::Value) {
        let (mut sheets, cells, styles, merged_cells) = hydrate_workbook(&content);
        if sheets.is_empty() {
            sheets.push("Sheet1".into());
        }
        self.calculation.borrow_mut().take();
        self.conflict = None;
        self.snapshot = CalcSnapshot {
            sheets,
            cells,
            styles,
            merged_cells,
        };
        self.baseline = content.clone();
        self.content = content;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.revision += 1;
    }
    fn apply_snapshot(&mut self, remote: serde_json::Value) -> bool {
        if !self.dirty {
            self.hydrate(remote);
            return true;
        }
        if self.conflict.is_some() {
            self.conflict = Some(remote);
            return false;
        }
        let local = self.serialize_raw();
        let Some(merged) = snapshot_merge::merge(&self.baseline, &local, &remote) else {
            if self.baseline.is_null() && !self.content["id"].is_string() {
                self.content = remote.clone();
            }
            self.conflict = Some(remote);
            return false;
        };
        if !merged.pending {
            self.hydrate(remote);
            return true;
        }
        let rebase = |history: Vec<(CalcSnapshot, serde_json::Value)>| {
            history
                .into_iter()
                .filter_map(|(snapshot, content)| {
                    let old = Book {
                        snapshot,
                        content,
                        ..Default::default()
                    }
                    .serialize_raw();
                    let content = merged.rebase_history(&old)?;
                    let mut book = Book::default();
                    book.hydrate(content);
                    Some((book.snapshot, book.content))
                })
                .collect()
        };
        let undo = rebase(std::mem::take(&mut self.undo));
        let redo = rebase(std::mem::take(&mut self.redo));
        self.hydrate(merged.content.clone());
        self.baseline = remote;
        self.undo = undo;
        self.redo = redo;
        self.dirty = true;
        true
    }
    fn observe_commit(&mut self, committed: serde_json::Value, revision: Option<u64>) {
        self.baseline = committed.clone();
        if revision == Some(self.revision) {
            self.content = committed;
            self.dirty = false;
        }
    }
    fn structure_changed(&mut self) {
        self.content["collaboration_structure"] = serde_json::json!(uuid::Uuid::new_v4());
    }
    fn checkpoint(&mut self) {
        self.undo
            .push((self.snapshot.clone(), self.content.clone()));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.changed();
    }
    fn changed(&mut self) {
        self.calculation.borrow_mut().take();
        self.revision += 1;
        self.dirty = true;
    }
    fn serialize_raw(&self) -> serde_json::Value {
        let snap = &self.snapshot;
        let mut root = self.content.as_object().cloned().unwrap_or_default();
        let original_cells: HashMap<_, _> = workbook_cell_entries(&self.content).collect();
        let sheets = snap
            .sheets
            .iter()
            .enumerate()
            .map(|(si, name)| {
                let mut sheet = self
                    .content
                    .get("sheets")
                    .and_then(|s| s.get(si))
                    .and_then(|s| s.as_object())
                    .cloned()
                    .unwrap_or_else(|| {
                        serde_json::json!({"id":uuid::Uuid::new_v4(), "column_widths":{},
                        "row_heights":{}, "frozen_rows":0, "frozen_cols":0,
                        "conditional_formats":[], "data_validations":[], "charts":[]})
                        .as_object()
                        .unwrap()
                        .clone()
                    });
                let keys: std::collections::BTreeSet<_> = snap
                    .cells
                    .keys()
                    .chain(snap.styles.keys())
                    .filter(|key| key.0 == si)
                    .map(|key| (key.1, key.2))
                    .collect();
                let cells = keys
                    .into_iter()
                    .map(|(r, c)| {
                        let value = snap.cells.get(&(si, r, c)).cloned().unwrap_or_default();
                        let original = original_cells.get(&(si, r, c)).copied();
                        let original_display = original.map(cell_input_text);
                        let mut cell = original
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"comment":null}));
                        if original_display.as_deref() != Some(value.as_str()) {
                            let edited = CalculationCell::edited(&value);
                            cell["formula"] = serde_json::json!(edited.formula);
                            cell["value"] = serde_json::json!(edited.value);
                        }
                        let previous_format = original
                            .and_then(|cell| cell.get("format"))
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        let previous_style = style_from_json(&previous_format);
                        let next_style = snap.styles.get(&(si, r, c)).cloned().unwrap_or_default();
                        if previous_style != next_style {
                            let mut format = previous_format;
                            merge_visible_delta(
                                &mut format,
                                &style_to_value(&previous_style),
                                &style_to_value(&next_style),
                            );
                            cell["format"] = format;
                        }
                        serde_json::json!([{"row":r,"col":c},cell])
                    })
                    .collect();
                let ranges: Vec<_> = snap
                    .merged_cells
                    .get(&si)
                    .into_iter()
                    .flatten()
                    .map(|range| {
                        let range = range.normalized();
                        serde_json::json!({"start":{"row":range.start.0,"col":range.start.1},
                    "end":{"row":range.end.0,"col":range.end.1}})
                    })
                    .collect();
                sheet.insert("name".into(), name.clone().into());
                sheet.insert("cells".into(), serde_json::Value::Array(cells));
                sheet.insert("merged_cells".into(), ranges.into());
                serde_json::Value::Object(sheet)
            })
            .collect();
        root.insert("sheets".into(), serde_json::Value::Array(sheets));
        serde_json::Value::Object(root)
    }
    fn ensure_calculated(&self) {
        if self.calculation.borrow().is_some() {
            return;
        }
        let input = CalculationInput::from_workbook(&self.snapshot, &self.content);
        let mut values = input.constants();
        #[cfg(target_arch = "wasm32")]
        if input.needs_worker() {
            *self.calculation.borrow_mut() = Some(CalculationCache {
                values,
                pending: Some(Rc::new(input)),
            });
            return;
        }
        values.extend(
            input
                .calculate(usize::MAX)
                .expect("calculation fits in memory"),
        );
        *self.calculation.borrow_mut() = Some(CalculationCache {
            values,
            #[cfg(target_arch = "wasm32")]
            pending: None,
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn pending_calculation(&self) -> Option<Rc<CalculationInput>> {
        self.ensure_calculated();
        self.calculation
            .borrow()
            .as_ref()
            .expect("calculated")
            .pending
            .clone()
    }

    #[cfg(target_arch = "wasm32")]
    fn calculation_allows_save(
        &self,
        (revision, progress): (u64, calculation_worker::Progress),
    ) -> bool {
        self.pending_calculation().is_none()
            || (self.revision == revision && progress.allows_raw_save())
    }

    #[cfg(target_arch = "wasm32")]
    fn accept_calculation(&self, revision: u64, values: calculation::FormulaResults) -> bool {
        if self.revision != revision {
            return false;
        }
        let mut calculation = self.calculation.borrow_mut();
        let Some(cache) = calculation.as_mut() else {
            return false;
        };
        if cache.pending.take().is_none() {
            return false;
        }
        cache.values.extend(values);
        true
    }

    fn display(&self, si: usize, row: u32, col: u32) -> String {
        self.ensure_calculated();
        self.calculation
            .borrow()
            .as_ref()
            .expect("calculated")
            .values
            .get(&(si, row, col))
            .map(calculated_text)
            .or_else(|| self.snapshot.cells.get(&(si, row, col)).cloned())
            .unwrap_or_default()
    }
    fn numeric(&self, si: usize, row: u32, col: u32) -> Option<f64> {
        self.ensure_calculated();
        match self
            .calculation
            .borrow()
            .as_ref()
            .expect("calculated")
            .values
            .get(&(si, row, col))
        {
            Some(syntra_doc_engine::calc::CellValue::Number(n)) => Some(*n),
            _ => None,
        }
    }
    fn serialize(&self) -> serde_json::Value {
        let mut content = self.serialize_raw();
        self.ensure_calculated();
        let calculation = self.calculation.borrow();
        let cache = calculation.as_ref().expect("calculated");
        for (si, sheet) in content["sheets"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for pair in sheet["cells"].as_array_mut().into_iter().flatten() {
                let row = pair[0]["row"].as_u64().unwrap_or(0) as u32;
                let col = pair[0]["col"].as_u64().unwrap_or(0) as u32;
                if !pair[1]["formula"].is_null() {
                    pair[1]["value"] = serde_json::to_value(
                        cache
                            .values
                            .get(&(si, row, col))
                            .unwrap_or(&syntra_doc_engine::CellValue::Empty),
                    )
                    .expect("cell values serialize");
                }
            }
        }
        content
    }
    #[cfg(any(test, target_arch = "wasm32"))]
    fn apply_remote(&mut self, payload: &serde_json::Value) {
        if let Some((_user, patch)) = cell_patch_from_operation(payload) {
            let mut merged = self.serialize_raw();
            if self.dirty {
                let base = workbook_cells(&self.content);
                let local = workbook_cells(&merged);
                let key = (patch.sheet, patch.row, patch.col);
                if snapshot_merge::semantic(base.get(&key))
                    != snapshot_merge::semantic(local.get(&key))
                {
                    let mut server = self.content.clone();
                    apply_cell_patch(&mut server, &patch);
                    self.conflict = Some(server);
                    return;
                }
                if !apply_cell_patch(&mut merged, &patch) {
                    return;
                }
                let rebase = |history: Vec<(CalcSnapshot, serde_json::Value)>| {
                    history
                        .into_iter()
                        .map(|(snapshot, content)| {
                            let mut checkpoint = Book {
                                snapshot,
                                content,
                                ..Default::default()
                            };
                            let mut merged = checkpoint.serialize_raw();
                            apply_cell_patch(&mut merged, &patch);
                            checkpoint.hydrate(merged);
                            (checkpoint.snapshot, checkpoint.content)
                        })
                        .collect()
                };
                let undo = rebase(std::mem::take(&mut self.undo));
                let redo = rebase(std::mem::take(&mut self.redo));
                let mut baseline = self.content.clone();
                apply_cell_patch(&mut baseline, &patch);
                self.hydrate(merged);
                self.baseline = baseline.clone();
                self.content = baseline;
                self.undo = undo;
                self.redo = redo;
                self.dirty = true;
            } else {
                if apply_cell_patch(&mut merged, &patch) {
                    self.hydrate(merged);
                }
            }
        } else if let Some((_user, replacement)) = workbook_replace_from_operation(payload) {
            if self.dirty {
                self.conflict = Some(replacement);
            } else {
                self.hydrate(replacement);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Editor {
    book: Cage<Book>,
    calculation_epoch: Cage<u64>,
    sheet: Cage<usize>,
    active: Cage<(u32, u32)>,
    anchor: Cage<(u32, u32)>,
    end: Cage<(u32, u32)>,
    extra: Cage<Vec<(u32, u32)>>,
    extend: Cage<bool>,
    additive: Cage<bool>,
    filter: Cage<Option<(u32, String)>>,
    formula: Cage<String>,
    editing: Cage<Option<(usize, u32, u32)>>,
    fill_preview: Cage<Option<MergedRange>>,
    resize_preview: Cage<Option<(bool, u32, f64)>>,
    status: Cage<String>,
    layout: Cage<u64>,
}
impl Editor {
    fn range(self) -> MergedRange {
        MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized()
    }
    fn read_active(self) {
        let si = self.sheet.get();
        let (r, c) = self.active.get();
        let raw = self.book.with(|b| {
            b.snapshot
                .cells
                .get(&(si, r, c))
                .cloned()
                .unwrap_or_default()
        });
        self.formula.revise(|v| *v = raw);
    }
    fn select_range(self, range: MergedRange) {
        if !self.finish_edit(false) {
            return;
        }
        let range = range.normalized();
        self.active.revise(|v| *v = range.start);
        self.anchor.revise(|v| *v = range.start);
        self.end.revise(|v| *v = range.end);
        self.extra.revise(Vec::clear);
        self.read_active();
    }
    fn select_point(self, point: (u32, u32), extend: bool, additive: bool) {
        if !self.finish_edit(false) {
            return;
        }
        if extend || self.extend.get() {
            self.end.revise(|v| *v = point);
        } else if additive || self.additive.get() {
            let mut cells = self.selected();
            if cells.contains(&point) && cells.len() > 1 {
                cells.retain(|p| *p != point);
            } else if !cells.contains(&point) {
                cells.push(point);
            }
            let active = if cells.contains(&point) {
                point
            } else {
                cells[0]
            };
            cells.retain(|p| *p != active);
            self.active.revise(|v| *v = active);
            self.anchor.revise(|v| *v = active);
            self.end.revise(|v| *v = active);
            self.extra.revise(|v| *v = cells);
        } else {
            self.activate(point);
        }
        self.read_active();
    }
    fn dimension(self, sheet: &serde_json::Value, column: bool, index: u32) -> f64 {
        self.resize_preview
            .get()
            .filter(|(axis, i, _)| *axis == column && *i == index)
            .map(|(_, _, size)| size)
            .unwrap_or_else(|| sheet_axis_size(sheet, column, index))
    }
    #[cfg(target_arch = "wasm32")]
    fn resize_axis(self, column: bool, index: u32, size: f64) {
        let si = self.sheet.get();
        self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize_raw();
            let key = if column {
                "column_widths"
            } else {
                "row_heights"
            };
            if !b.content["sheets"][si][key].is_object() {
                b.content["sheets"][si][key] = serde_json::json!({});
            }
            b.content["sheets"][si][key][index.to_string()] = serde_json::json!(size);
        });
    }
    fn drag_fill(self, source: MergedRange, extent: MergedRange) -> bool {
        let si = self.sheet.get();
        let patch = self
            .book
            .with(|b| super::fill::plan_fill(&b.snapshot, si, source, extent));
        let patch = match patch {
            Ok(patch) => patch,
            Err(error) => {
                self.status.revise(|v| *v = error.to_string());
                return false;
            }
        };
        let error = self.book.with(|b| {
            patch.changes.iter().find_map(|change| {
                validation_error_for(
                    &b.content,
                    si,
                    change.point.0,
                    change.point.1,
                    change.value.as_deref().unwrap_or(""),
                )
            })
        });
        if let Some(error) = error {
            self.status.revise(|v| *v = error);
            return false;
        }
        if !patch.changes.is_empty() {
            self.book.revise(|b| {
                b.checkpoint();
                patch.apply_to(&mut b.snapshot);
            });
        }
        self.select_range(patch.range);
        true
    }
    fn selected(self) -> Vec<(u32, u32)> {
        current_selection_cells(
            self.active.get(),
            Some(self.anchor.get()),
            Some(self.end.get()),
            &self.extra.get(),
        )
    }
    fn step(self, dr: i32, dc: i32, extend: bool) {
        let si = self.sheet.get();
        let current = self.active.get();
        let range = self
            .book
            .with(|b| merged_range_at(&b.snapshot.merged_cells, si, current));
        let origin = range
            .map(|range| {
                (
                    if dr > 0 { range.end.0 } else { range.start.0 },
                    if dc > 0 { range.end.1 } else { range.start.1 },
                )
            })
            .unwrap_or(current);
        let mut target = (
            (origin.0 as i32 + dr).clamp(0, NUM_ROWS as i32 - 1) as u32,
            (origin.1 as i32 + dc).clamp(0, NUM_COLS as i32 - 1) as u32,
        );
        // Filtered-out rows must never receive an invisible active cell.
        while dr != 0
            && self.filter.with(|filter| {
                filter.as_ref().is_some_and(|(col, query)| {
                    target.0 > 0
                        && self.book.with(|b| {
                            !b.snapshot
                                .cells
                                .get(&(si, target.0, *col))
                                .is_some_and(|v| v.to_lowercase().contains(&query.to_lowercase()))
                        })
                })
            })
        {
            let next = (target.0 as i32 + dr.signum()).clamp(0, NUM_ROWS as i32 - 1) as u32;
            if next == target.0 {
                return;
            }
            target.0 = next;
        }
        if extend || self.extend.get() {
            let target = self.book.with(|b| {
                merged_range_at(&b.snapshot.merged_cells, si, target)
                    .map(|range| range.start)
                    .unwrap_or(target)
            });
            self.active.revise(|v| *v = target);
            self.end.revise(|v| *v = target);
            let raw = self.book.with(|b| {
                b.snapshot
                    .cells
                    .get(&(si, target.0, target.1))
                    .cloned()
                    .unwrap_or_default()
            });
            self.formula.revise(|v| *v = raw);
        } else {
            self.activate(target);
        }
    }
    fn activate(self, point: (u32, u32)) {
        if !self.finish_edit(false) {
            return;
        }
        let sheet = self.sheet.get();
        let point = self.book.with(|book| {
            merged_range_at(&book.snapshot.merged_cells, sheet, point)
                .map(|range| range.start)
                .unwrap_or(point)
        });
        self.active.revise(|v| *v = point);
        self.anchor.revise(|v| *v = point);
        self.end.revise(|v| *v = point);
        self.extra.revise(Vec::clear);
        let raw = self.book.with(|b| {
            b.snapshot
                .cells
                .get(&(sheet, point.0, point.1))
                .cloned()
                .unwrap_or_default()
        });
        self.formula.revise(|v| *v = raw);
    }
    fn commit(self, value: String) {
        let si = self.sheet.get();
        let (r, c) = self.active.get();
        self.commit_at((si, r, c), value);
    }
    fn commit_at(self, (si, r, c): (usize, u32, u32), value: String) -> bool {
        let error = self
            .book
            .with(|b| validation_error_for(&b.content, si, r, c, &value));
        if let Some(error) = error {
            self.status.revise(|v| *v = error);
            return false;
        }
        self.book.revise(|b| {
            if b.snapshot
                .cells
                .get(&(si, r, c))
                .map(String::as_str)
                .unwrap_or("")
                == value
            {
                return;
            }
            b.checkpoint();
            if value.is_empty() {
                b.snapshot.cells.remove(&(si, r, c));
            } else {
                b.snapshot.cells.insert((si, r, c), value);
            }
        });
        true
    }
    #[cfg(target_arch = "wasm32")]
    fn begin_edit(self, point: (u32, u32), replacement: Option<String>) {
        if !self.finish_edit(false) {
            return;
        }
        self.activate(point);
        let (r, c) = self.active.get();
        if let Some(value) = replacement {
            self.formula.revise(|v| *v = value);
        }
        self.editing.revise(|v| *v = Some((self.sheet.get(), r, c)));
    }
    fn finish_edit(self, cancel: bool) -> bool {
        let Some(target) = self.editing.get() else {
            return true;
        };
        if !cancel && !self.commit_at(target, self.formula.get()) {
            return false;
        }
        // Clear the session before unmounting its input: a subsequent blur must
        // never commit a cancelled draft or write it into the next active cell.
        self.editing.revise(|v| *v = None);
        true
    }
    fn format(self, update: impl Fn(&mut CellStyle)) {
        let si = self.sheet.get();
        let cells = self.selected();
        self.book.revise(|b| {
            b.checkpoint();
            for (r, c) in cells {
                update(b.snapshot.styles.entry((si, r, c)).or_default());
            }
        });
    }
    fn history(self, redo: bool) {
        self.book.revise(|b| {
            let previous = if redo { b.redo.pop() } else { b.undo.pop() };
            if let Some((previous, content)) = previous {
                let current = (
                    std::mem::replace(&mut b.snapshot, previous),
                    std::mem::replace(&mut b.content, content),
                );
                if redo {
                    b.undo.push(current)
                } else {
                    b.redo.push(current)
                }
                b.changed();
            }
        });
        let count = self.book.with(|b| b.snapshot.sheets.len());
        self.sheet
            .revise(|v| *v = (*v).min(count.saturating_sub(1)));
        self.layout.revise(|v| *v += 1);
        self.activate(self.active.get());
    }
    fn merge(self, unmerge: bool) {
        let si = self.sheet.get();
        let range = MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized();
        self.book.revise(|b| {
            b.checkpoint();
            let ranges = b.snapshot.merged_cells.entry(si).or_default();
            ranges.retain(|r| !r.overlaps(range));
            if !unmerge && range.start != range.end {
                ranges.push(range);
            }
        });
        self.layout.revise(|v| *v += 1);
    }
    fn row_column(self, column: bool, delete: bool) {
        let si = self.sheet.get();
        let at = if column {
            self.active.get().1
        } else {
            self.active.get().0
        };
        self.book.revise(|b| {
            b.checkpoint();
            b.structure_changed();
            fn shift<T>(
                map: &mut HashMap<CellKey, T>,
                si: usize,
                at: u32,
                column: bool,
                delete: bool,
            ) {
                *map = std::mem::take(map)
                    .into_iter()
                    .filter_map(|((s, r, c), value)| {
                        if s != si {
                            return Some(((s, r, c), value));
                        }
                        let n = if column { c } else { r };
                        if delete && n == at {
                            return None;
                        }
                        let shifted = if n >= at {
                            if delete { n.saturating_sub(1) } else { n + 1 }
                        } else {
                            n
                        };
                        let (r, c) = if column { (r, shifted) } else { (shifted, c) };
                        (r < NUM_ROWS && c < NUM_COLS).then_some(((s, r, c), value))
                    })
                    .collect();
            }
            shift(&mut b.snapshot.cells, si, at, column, delete);
            shift(&mut b.snapshot.styles, si, at, column, delete);
            match (column, delete) {
                (false, false) => insert_row_in_merged(&mut b.snapshot.merged_cells, si, at),
                (false, true) => delete_row_from_merged(&mut b.snapshot.merged_cells, si, at),
                (true, false) => insert_col_in_merged(&mut b.snapshot.merged_cells, si, at),
                (true, true) => delete_col_from_merged(&mut b.snapshot.merged_cells, si, at),
            }
        });
        self.layout.revise(|v| *v += 1);
    }
    fn sort(self, descending: bool) {
        let si = self.sheet.get();
        let a = self.anchor.get();
        let z = self.end.get();
        let (first, last) = if a.0 == z.0 {
            (0, NUM_ROWS - 1)
        } else {
            (a.0.min(z.0), a.0.max(z.0))
        };
        let col = self.active.get().1;
        if self.book.with(|b| {
            merged_range_overlaps(
                &b.snapshot.merged_cells,
                si,
                (first, 0),
                (last, NUM_COLS - 1),
            )
        }) {
            self.status
                .revise(|v| *v = "Unmerge the selected rows before sorting".into());
            return;
        }
        self.book.revise(|b| {
            b.checkpoint();
            b.structure_changed();
            let mut rows: Vec<_> = (first..=last)
                .map(|r| {
                    let values: Vec<_> = (0..NUM_COLS)
                        .map(|c| {
                            (
                                b.snapshot.cells.get(&(si, r, c)).cloned(),
                                b.snapshot.styles.get(&(si, r, c)).cloned(),
                            )
                        })
                        .collect();
                    (
                        b.snapshot
                            .cells
                            .get(&(si, r, col))
                            .cloned()
                            .unwrap_or_default(),
                        values,
                    )
                })
                .collect();
            rows.sort_by(|a, b| {
                match (a.0.is_empty(), b.0.is_empty()) {
                    (true, false) => return std::cmp::Ordering::Greater,
                    (false, true) => return std::cmp::Ordering::Less,
                    _ => {}
                }
                let ord = match (a.0.parse::<f64>(), b.0.parse::<f64>()) {
                    (Ok(a), Ok(b)) => a.total_cmp(&b),
                    _ => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
                };
                if descending { ord.reverse() } else { ord }
            });
            for (offset, (_, values)) in rows.into_iter().enumerate() {
                for (c, (value, style)) in values.into_iter().enumerate() {
                    let key = (si, first + offset as u32, c as u32);
                    b.snapshot.cells.remove(&key);
                    b.snapshot.styles.remove(&key);
                    if let Some(value) = value {
                        b.snapshot.cells.insert(key, value);
                    }
                    if let Some(style) = style {
                        b.snapshot.styles.insert(key, style);
                    }
                }
            }
        });
    }
    fn fill(self, down: bool) {
        let range = self.range();
        let source = MergedRange {
            start: range.start,
            end: if down {
                (range.start.0, range.end.1)
            } else {
                (range.end.0, range.start.1)
            },
        };
        self.drag_fill(source, range);
    }
    fn metadata(self, operation: impl FnOnce(&mut serde_json::Value, usize, MergedRange)) {
        let si = self.sheet.get();
        let range = MergedRange {
            start: self.anchor.get(),
            end: self.end.get(),
        }
        .normalized();
        self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize_raw();
            operation(&mut b.content, si, range);
        });
    }
    fn duplicate_sheet(self) {
        let si = self.sheet.get();
        let target = self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize_raw();
            let target = b.snapshot.sheets.len();
            let name = format!("{} copy", b.snapshot.sheets[si]);
            b.snapshot.sheets.push(name.clone());
            let cells: Vec<_> = b
                .snapshot
                .cells
                .iter()
                .filter(|(k, _)| k.0 == si)
                .map(|((_, r, c), v)| ((target, *r, *c), v.clone()))
                .collect();
            let styles: Vec<_> = b
                .snapshot
                .styles
                .iter()
                .filter(|(k, _)| k.0 == si)
                .map(|((_, r, c), v)| ((target, *r, *c), v.clone()))
                .collect();
            b.snapshot.cells.extend(cells);
            b.snapshot.styles.extend(styles);
            if let Some(ranges) = b.snapshot.merged_cells.get(&si).cloned() {
                b.snapshot.merged_cells.insert(target, ranges);
            }
            let mut sheet = b.content["sheets"][si].clone();
            sheet["id"] = serde_json::json!(uuid::Uuid::new_v4());
            sheet["name"] = name.into();
            append_array_entry(&mut b.content, "sheets", sheet);
            target
        });
        self.sheet.revise(|v| *v = target);
        self.activate((0, 0));
        self.layout.revise(|v| *v += 1);
    }
    fn move_sheet(self, right: bool) {
        let from = self.sheet.get();
        let count = self.book.with(|b| b.snapshot.sheets.len());
        let to = if right {
            (from + 1).min(count - 1)
        } else {
            from.saturating_sub(1)
        };
        if from == to {
            return;
        }
        self.book.revise(|b| {
            b.checkpoint();
            b.content = b.serialize_raw();
            b.snapshot.sheets.swap(from, to);
            let swap = |s| {
                if s == from {
                    to
                } else if s == to {
                    from
                } else {
                    s
                }
            };
            b.snapshot.cells = std::mem::take(&mut b.snapshot.cells)
                .into_iter()
                .map(|((s, r, c), v)| ((swap(s), r, c), v))
                .collect();
            b.snapshot.styles = std::mem::take(&mut b.snapshot.styles)
                .into_iter()
                .map(|((s, r, c), v)| ((swap(s), r, c), v))
                .collect();
            swap_sheets_in_merged(&mut b.snapshot.merged_cells, from, to);
            if let Some(sheets) = b.content["sheets"].as_array_mut() {
                sheets.swap(from, to);
            }
        });
        self.sheet.revise(|v| *v = to);
        self.layout.revise(|v| *v += 1);
    }
}

fn action(label: &str, callback: impl FnMut() + 'static) -> Box<dyn Widget> {
    let caption = match label {
        "Bold" => "B",
        "Italic" => "I",
        "Underline" => "U",
        "Align left" => "⇤",
        "Align center" => "↔",
        "Align right" => "⇥",
        "Currency" => "$",
        "Percent" => "%",
        "Merge cells" => "Merge",
        "Unmerge cells" => "Unmerge",
        "Sort ascending" => "A↓Z",
        "Sort descending" => "Z↓A",
        "All borders" => "▦",
        "Clear borders" => "□",
        _ => label,
    };
    Box::new(
        sapid::button()
            .theme(crate::components::ui::theme())
            .variant(sapid::ButtonVariant::Secondary)
            .size(sapid::Size::Small)
            .class("toolbar-btn")
            .name(label.to_owned())
            .label(caption.to_owned())
            .on_press(callback),
    )
}
fn toolbar_action(
    label: &'static str,
    callback: impl FnMut() + Clone + 'static,
) -> sapid::OverflowItem {
    sapid::overflow_item(label.replace(' ', "-").to_ascii_lowercase())
        .content(move || action(label, callback.clone()))
}

fn control(
    name: &str,
    value: impl Into<Vary<String>>,
    callback: impl FnMut(String) + 'static,
) -> sapid::TextField {
    sapid::text_field()
        .theme(crate::components::ui::theme())
        .name(name.to_owned())
        .value(value)
        .on_value_change(callback)
}

#[derive(Clone, Copy)]
enum ClipboardCommand {
    Copy,
    Cut,
    Paste,
}

fn clipboard_command(
    weak: glory::ui::WeakScope,
    ed: Editor,
    command: ClipboardCommand,
) -> impl FnMut() + Clone {
    move || {
        let Some(scope) = weak.upgrade() else {
            return;
        };
        #[cfg(target_arch = "wasm32")]
        match command {
            ClipboardCommand::Copy => clipboard_events::request_copy(&scope, ed, false),
            ClipboardCommand::Cut => clipboard_events::request_copy(&scope, ed, true),
            ClipboardCommand::Paste => clipboard_events::request_paste(&scope, ed),
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (scope, command);
            ed.status
                .revise(|value| *value = "System clipboard requires the browser".into());
        }
    }
}

fn format_control(
    scope: &Scope,
    ed: Editor,
    name: &str,
    default: &str,
    callback: impl FnMut(String) + 'static,
) -> impl Widget {
    let field = name.to_owned();
    let fallback = default.to_owned();
    let value = scope.bond(move || {
        let si = ed.sheet.get();
        let (r, c) = ed.active.get();
        ed.book.with(|b| {
            let style = b
                .snapshot
                .styles
                .get(&(si, r, c))
                .cloned()
                .unwrap_or_default();
            let value = match field.as_str() {
                "Font family" => style.font_family,
                "Font size" => style.font_size,
                "Text color (#rrggbb)" => style.text_color,
                "Fill color (#rrggbb)" => style.fill_color,
                "Number format" => style.number_format,
                "Column width" => ed.dimension(&b.content["sheets"][si], true, c).to_string(),
                "Row height" => ed.dimension(&b.content["sheets"][si], false, r).to_string(),
                _ => String::new(),
            };
            if value.is_empty() {
                fallback.clone()
            } else {
                value
            }
        })
    });
    col()
        .class(format!(
            "calc-format-field {}",
            match name {
                "Font size" => "font-size-field",
                "Font family" => "font-family-field",
                _ => "advanced-format-field",
            }
        ))
        .fill((
            text().class("toolbar-label").content(name.to_owned()),
            sapid::text_field()
                .theme(crate::components::ui::theme())
                .size(sapid::Size::Small)
                .name(name.to_owned())
                .value(value)
                .on_value_change(callback),
        ))
}

/// Browser workbook editor. Every mutation stays in application-owned Cages.
#[allow(non_snake_case)]
pub fn Calc(scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    let ed = Editor {
        book: scope.cage(Book::default()),
        calculation_epoch: scope.cage(0),
        sheet: scope.cage(0),
        active: scope.cage((0, 0)),
        anchor: scope.cage((0, 0)),
        end: scope.cage((0, 0)),
        extra: scope.cage(Vec::new()),
        extend: scope.cage(false),
        additive: scope.cage(false),
        filter: scope.cage(None),
        formula: scope.cage(String::new()),
        editing: scope.cage(None),
        fill_preview: scope.cage(None),
        resize_preview: scope.cage(None),
        status: scope.cage("Loading document…".to_owned()),
        layout: scope.cage(0),
    };
    #[cfg(target_arch = "wasm32")]
    {
        pointer_events::install(&scope, ed);
        clipboard_events::install(&scope, ed);
    }
    let loaded = scope.cage(false);
    let version = scope.cage(0i32);
    let remote_version = scope.cage(0i32);
    let submitted = Rc::new(RefCell::new(None::<serde_json::Value>));
    let title = scope.cage("Spreadsheet".to_owned());
    let connected = scope.cage(ConnectionState::Connecting);
    let collaborators = scope.cage(Vec::<String>::new());
    #[cfg(target_arch = "wasm32")]
    let session = scope.cage(None::<CollabHandle>);
    let token = auth.with(|a| a.token.clone().unwrap_or_default());
    #[cfg(target_arch = "wasm32")]
    let calculation =
        calculation_worker::Controller::install(&scope, ed, loaded, auth, token.clone());
    #[cfg(target_arch = "wasm32")]
    let self_id = auth.with(|a| {
        a.user
            .as_ref()
            .map(|u| u.id.to_string())
            .unwrap_or_default()
    });
    let request_id = id.clone();
    let load_token = token.clone();
    let _ = scope.spawn(async move {
        match api_get::<DocumentResponse>(&format!("/documents/{request_id}"), &load_token).await {
            Ok(doc) => {
                title.revise(|v| *v = doc.name);
                if doc.version >= version.get() {
                    ed.book.revise(|book| {
                        book.apply_snapshot(doc.content);
                    });
                    if ed.book.with(|book| book.conflict.is_some()) {
                        remote_version.revise(|value| *value = doc.version);
                        ed.status.revise(|value| {
                            *value =
                                "Conflict: local edits preserved while loading server state.".into()
                        });
                    } else {
                        version.revise(|value| *value = doc.version);
                        ed.status.revise(|value| {
                            *value = if ed.book.with(|book| book.dirty) {
                                "Unsaved changes"
                            } else {
                                "Saved"
                            }
                            .into()
                        });
                    }
                }
                loaded.revise(|v| *v = true);
                ed.layout.revise(|v| *v += 1);
            }
            Err(error) => ed
                .status
                .revise(|v| *v = format!("Could not load document: {error}")),
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        let weak = scope.downgrade();
        let author = self_id.clone();
        let submitted = submitted.clone();
        let handle = connect_with_reconnect(
            id.clone(),
            token.clone(),
            move |message| {
                if weak.upgrade().is_none() {
                    return;
                }
                match message {
                    CollabMessage::Operation(payload) => {
                        if payload["user_id"].as_str() == Some(author.as_str()) {
                            return;
                        }
                        ed.book.revise(|book| book.apply_remote(&payload));
                        if ed.book.with(|book| book.conflict.is_some()) {
                            ed.status.revise(|value| *value = "Conflict: local edits preserved. Reload to accept the server version or save a copy.".into());
                        }
                        ed.layout.revise(|v| *v += 1);
                    }
                    CollabMessage::DocumentState {
                        content,
                        version: server_version,
                    } => {
                        let server_version = i32::try_from(server_version).unwrap_or(i32::MAX);
                        if server_version <= version.get() {
                            return;
                        }
                        let own_commit = submitted.borrow().as_ref() == Some(&content);
                        if own_commit {
                            ed.book.revise(|book| book.observe_commit(content, None));
                            version.revise(|value| *value = server_version);
                        } else {
                            ed.book.revise(|book| {
                                book.apply_snapshot(content);
                            });
                            if ed.book.with(|book| book.conflict.is_some()) {
                                remote_version.revise(|value| *value = server_version);
                                ed.status.revise(|value|*value="Conflict: local edits preserved. Choose a version before saving.".into());
                            } else {
                                version.revise(|value| *value = server_version);
                                ed.status.revise(|value| {
                                    *value = if ed.book.with(|book| book.dirty) {
                                        "Unsaved changes"
                                    } else {
                                        "Saved"
                                    }
                                    .into()
                                });
                            }
                            loaded.revise(|value| *value = true);
                            ed.layout.revise(|value| *value += 1);
                        }
                    }
                    CollabMessage::UserJoined {
                        user_id, username, ..
                    } => collaborators.revise(|v| {
                        if !v
                            .iter()
                            .any(|entry| entry.starts_with(&format!("{user_id}:")))
                        {
                            v.push(format!("{user_id}:{username}"));
                        }
                    }),
                    CollabMessage::UserLeft { user_id } => collaborators
                        .revise(|v| v.retain(|s| !s.starts_with(&format!("{user_id}:")))),
                    CollabMessage::Error { message } => ed.status.revise(|v| *v = message),
                    _ => {}
                }
            },
            {
                let weak = scope.downgrade();
                move |state| {
                    if weak.upgrade().is_some() {
                        connected.revise(|v| *v = state)
                    }
                }
            },
        );
        session.revise(|v| *v = Some(handle.clone()));
        let _ = scope.on_cleanup(move || handle.close());
    }
    let timer = Rc::new(RefCell::new(None::<Timeout>));
    let saving = scope.cage(false);
    let failed = scope.cage(false);
    #[cfg(target_arch = "wasm32")]
    let waiting_for_calculation = scope.cage(false);
    #[cfg(target_arch = "wasm32")]
    let calculation_progress = calculation.progress;
    let save: Rc<dyn Fn()> = Rc::new({
        let weak = scope.downgrade();
        let id = id.clone();
        let token = token.clone();
        let submitted = submitted.clone();
        move || {
            if !loaded.get()
                || saving.get()
                || !ed.book.with(|b| b.dirty)
                || ed.book.with(|b| b.conflict.is_some())
                || !auth.with(|state| state.token.as_deref() == Some(token.as_str()))
            {
                return;
            }
            let Some(scope) = weak.upgrade() else {
                return;
            };
            #[cfg(target_arch = "wasm32")]
            {
                if !ed
                    .book
                    .with(|book| book.calculation_allows_save(calculation_progress.get()))
                {
                    waiting_for_calculation.revise(|waiting| *waiting = true);
                    return;
                }
                if waiting_for_calculation.get_untracked() {
                    waiting_for_calculation.revise(|waiting| *waiting = false);
                }
            }
            let (content, revision) = ed.book.with(|b| (b.serialize(), b.revision));
            let id = id.clone();
            let token = token.clone();
            let weak = weak.clone();
            let expected_version = version.get();
            *submitted.borrow_mut() = Some(content.clone());
            failed.revise(|value| *value = false);
            saving.revise(|v| *v = true);
            ed.status.revise(|v| *v = "Saving…".into());
            let _ = scope.spawn(async move {
                let outcome = save_document(&id, &content, expected_version, &token).await;
                if weak.upgrade().is_none() {
                    return;
                }
                let outcome = if auth.with(|state| state.token.as_deref().unwrap_or_default() == token.as_str()) {
                    outcome
                } else {
                    DocumentSave::Unconfirmed
                };
                match outcome {
                    DocumentSave::Saved(next_version) => {
                        if next_version >= version.get() && ed.book.with(|book| book.conflict.is_none()) {
                            ed.book.revise(|book| book.observe_commit(content, Some(revision)));
                            version.revise(|value| *value = next_version);
                            ed.status.revise(|value| {
                                *value = if ed.book.with(|book| book.dirty) { "Unsaved changes" } else { "Saved" }.into();
                            });
                        }
                    }
                    DocumentSave::Changed(snapshot) => {
                        let has_conflict = ed.book.with(|book| book.conflict.is_some());
                        if snapshot.version > version.get() && (!has_conflict || snapshot.version > remote_version.get()) {
                            ed.book.revise(|book| {
                                book.apply_snapshot(snapshot.content);
                            });
                            if ed.book.with(|book| book.conflict.is_some()) {
                                remote_version.revise(|value| *value = snapshot.version);
                            } else {
                                version.revise(|value| *value = snapshot.version);
                            }
                            ed.layout.revise(|value| *value += 1);
                        }
                        failed.revise(|value| *value = ed.book.with(|book| book.dirty));
                        ed.status.revise(|value| {
                            *value = ed.book.with(|book| {
                                if book.conflict.is_some() {
                                    "Conflict: local edits preserved. Choose a version before saving.".into()
                                } else if book.dirty {
                                    crate::i18n::t("editor.save_unconfirmed")
                                } else {
                                    "Saved".into()
                                }
                            });
                        });
                    }
                    DocumentSave::Unconfirmed => {
                        failed.revise(|value| *value = true);
                        ed.status.revise(|value| *value = crate::i18n::t("editor.save_unconfirmed"));
                    }
                    DocumentSave::Rejected { status, message } => {
                        let advanced = status == 409 && version.get() > expected_version
                            && ed.book.with(|book| book.conflict.is_none());
                        failed.revise(|value| *value = !advanced);
                        ed.status.revise(|value| {
                            *value = if advanced {
                                if ed.book.with(|book| book.dirty) {
                                    "Concurrent changes merged; saving latest edits.".into()
                                } else {
                                    "Saved".into()
                                }
                            } else {
                                format!("Save failed: {message}. Local changes retained; retry Save or reload.")
                            };
                        });
                    }
                }
                saving.revise(|v| *v = false);
            });
        }
    });
    #[cfg(target_arch = "wasm32")]
    scope.effect({
        let save = save.clone();
        move || {
            if !waiting_for_calculation.get() || saving.get() {
                return;
            }
            let progress = calculation_progress.get();
            let (valid, ready) = ed.book.with(|book| {
                (
                    book.dirty && book.conflict.is_none(),
                    book.calculation_allows_save(progress),
                )
            });
            if !valid || ready {
                waiting_for_calculation.revise(|waiting| *waiting = false);
                if valid {
                    save();
                }
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    cell_events::install(&scope, ed, save.clone());
    scope.effect({
        let timer = timer.clone();
        let save = save.clone();
        move || {
            let _revision = ed.book.with(|b| b.revision);
            let busy = saving.get();
            timer.borrow_mut().take();
            if loaded.get()
                && ed.book.with(|b| b.dirty && b.conflict.is_none())
                && !busy
                && !failed.get()
            {
                ed.status.revise(|v| *v = "Unsaved changes".into());
                let save = save.clone();
                *timer.borrow_mut() = Some(Timeout::new(1500, move || save()));
            }
        }
    });
    let _ = scope.on_cleanup({
        let timer = timer.clone();
        move || {
            timer.borrow_mut().take();
        }
    });
    let formula_node = scope.cage(None);
    let formula_reconcile = scope.cage(0_u64);
    scope.effect({
        let weak = scope.downgrade();
        move || {
            let node = formula_node.get();
            if formula_reconcile.get() == 0 {
                return;
            }
            if let (Some(scope), Some(node)) = (weak.upgrade(), node) {
                // Escape can restore the last rendered value within one host tick.
                // Reconcile the native draft after routing, even when the binding
                // sees equal values and therefore does not emit a value patch.
                if let Err(error) = scope.set_input_value(node, ed.formula.get_untracked()) {
                    scope.report_error(error);
                }
            }
        }
    });
    let address = scope.cage("A1".to_owned());
    scope.effect(move || {
        let range = ed.range();
        let active = ed.active.get();
        let value = if range.start == range.end {
            cell_ref(active.0, active.1)
        } else {
            format!(
                "{}:{}",
                cell_ref(range.start.0, range.start.1),
                cell_ref(range.end.0, range.end.1)
            )
        };
        address.revise(|v| *v = value);
    });
    let selection = scope.cage("A1:A1".to_owned());
    let sheet_name = scope.cage("Sheet1".to_owned());
    let last_sheet_name = scope.cage(None::<(usize, String)>);
    scope.effect(move || {
        let index = ed.sheet.get();
        let name = ed
            .book
            .with(|book| book.snapshot.sheets.get(index).cloned().unwrap_or_default());
        let current = (index, name);
        if last_sheet_name.get_untracked().as_ref() != Some(&current) {
            sheet_name.revise(|value| *value = current.1.clone());
            last_sheet_name.revise(|value| *value = Some(current));
        }
    });
    let find = scope.cage(String::new());
    let replace = scope.cage(String::new());
    let clipboard = scope.cage(String::new());
    let show_chart = scope.cage(false);
    let comments_open = scope.cage(false);
    let versions_open = scope.cage(false);
    let ai_open = scope.cage(false);
    let advanced_open = scope.cage(false);
    let clipboard_open = scope.cage(false);
    let find_open = scope.cage(false);
    let selection_open = scope.cage(false);
    let sheets_open = scope.cage(false);
    let metadata_name = scope.cage("Selection".to_owned());
    let threshold = scope.cage("0".to_owned());
    let filter_query = scope.cage(String::new());
    let chrome: Vec<Box<dyn Widget>> = vec![
        Box::new(
            link()
                .target("/dashboard")
                .fill(text().content("← Dashboard")),
        ),
        Box::new(text().class("calc-document-name").content(title)),
        Box::new(text().class("save-status").content(ed.status)),
        action("Save", {
            let save = save.clone();
            move || save()
        }),
        Box::new(when(
            scope.bond(move || ed.book.with(|book| book.conflict.is_some())),
            {
                let save = save.clone();
                move || {
                    let save = save.clone();
                    row().class("editor-conflict").fill((
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Use server version")
                            .on_press(move || {
                                let remote = ed.book.with(|book| book.conflict.clone());
                                if let Some(remote) = remote {
                                    ed.book.revise(|book| book.hydrate(remote));
                                    version.revise(|value| *value = remote_version.get());
                                    failed.revise(|value| *value = false);
                                    ed.status.revise(|value| *value = "Saved".into());
                                }
                            }),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Keep my version")
                            .on_press(move || {
                                version.revise(|value| *value = remote_version.get());
                                ed.book.revise(|book| {
                                    book.conflict = None;
                                    book.changed();
                                });
                                failed.revise(|value| *value = false);
                                save();
                            }),
                    ))
                }
            },
        )),
        action("Comments", move || comments_open.revise(|v| *v = !*v)),
        action("Version history", move || {
            versions_open.revise(|v| *v = !*v)
        }),
        action("AI assist", move || ai_open.revise(|v| *v = !*v)),
        Box::new(text().content(scope.bond(move || format!("{:?}", connected.get())))),
        Box::new(text().content(
            scope.bond(move || collaborators.with(|v| format!("{} collaborators", v.len()))),
        )),
    ];
    #[cfg(target_arch = "wasm32")]
    let chrome = {
        use calculation_worker::Progress;

        let mut chrome = chrome;
        let progress = calculation.progress;
        chrome.push(Box::new(
            text()
                .class("calc-formula-status")
                .content(scope.bond(move || progress.get().1.message())),
        ));
        chrome.push(Box::new(when(
            scope.bond(move || progress.get().1 == Progress::Pending),
            {
                let calculation = calculation.clone();
                move || {
                    let calculation = calculation.clone();
                    action(&crate::i18n::t("calc.formula_cancel"), move || {
                        calculation.cancel()
                    })
                }
            },
        )));
        chrome.push(Box::new(when(
            scope.bond(move || progress.get().1.allows_raw_save()),
            move || {
                let calculation = calculation.clone();
                action(&crate::i18n::t("calc.formula_retry"), move || {
                    calculation.retry()
                })
            },
        )));
        chrome
    };
    let toolbar: Vec<sapid::OverflowItem> = vec![
        toolbar_action("Bold", move || ed.format(|s| s.bold = !s.bold)),
        toolbar_action("Italic", move || ed.format(|s| s.italic = !s.italic)),
        toolbar_action("Underline", move || {
            ed.format(|s| s.underline = !s.underline)
        }),
        toolbar_action("Align left", move || ed.format(|s| s.align = "left".into())),
        toolbar_action("Align center", move || {
            ed.format(|s| s.align = "center".into())
        }),
        toolbar_action("Align right", move || {
            ed.format(|s| s.align = "right".into())
        }),
        toolbar_action("Currency", move || {
            ed.format(|s| s.number_format = "currency".into())
        }),
        toolbar_action("Percent", move || {
            ed.format(|s| s.number_format = "percent".into())
        }),
        toolbar_action("Merge cells", move || ed.merge(false)),
        toolbar_action("Unmerge cells", move || ed.merge(true)),
        toolbar_action("Insert row", move || ed.row_column(false, false)),
        toolbar_action("Delete row", move || ed.row_column(false, true)),
        toolbar_action("Insert column", move || ed.row_column(true, false)),
        toolbar_action("Delete column", move || ed.row_column(true, true)),
        toolbar_action("Sort ascending", move || ed.sort(false)),
        toolbar_action("Sort descending", move || ed.sort(true)),
        toolbar_action("Fill down", move || ed.fill(true)),
        toolbar_action("Fill right", move || ed.fill(false)),
        toolbar_action("Clear cells", move || {
            let si = ed.sheet.get();
            let cells = ed.selected();
            ed.book.revise(|b| {
                b.checkpoint();
                for (r, c) in cells {
                    b.snapshot.cells.remove(&(si, r, c));
                    b.snapshot.styles.remove(&(si, r, c));
                }
            });
            ed.formula.revise(String::clear);
        }),
        toolbar_action(
            "Copy selection",
            clipboard_command(scope.downgrade(), ed, ClipboardCommand::Copy),
        ),
        toolbar_action(
            "Cut selection",
            clipboard_command(scope.downgrade(), ed, ClipboardCommand::Cut),
        ),
        toolbar_action(
            "Paste selection",
            clipboard_command(scope.downgrade(), ed, ClipboardCommand::Paste),
        ),
        toolbar_action("Chart selection", move || {
            ed.metadata(|content, si, range| {
                append_array_entry(
                    &mut content["sheets"][si],
                    "charts",
                    serde_json::json!({
                        "id":uuid::Uuid::new_v4(),"title":"Chart","chart_type":"Bar",
                        "data_start":{"row":range.start.0,"col":range.start.1},
                        "data_end":{"row":range.end.0,"col":range.end.1},
                        "anchor_start":{"row":range.start.0,"col":range.end.1+2},
                        "anchor_end":{"row":range.start.0+12,"col":range.end.1+10}
                    }),
                );
            });
            show_chart.revise(|v| *v = !*v);
        }),
        toolbar_action("All borders", move || {
            ed.format(|s| {
                s.borders = CellBorders {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true,
                }
            })
        }),
        toolbar_action("Clear borders", move || {
            ed.format(|s| s.borders = CellBorders::default())
        }),
    ];
    let mut formats: Vec<Box<dyn Widget>> = vec![
        Box::new(format_control(&scope, ed, "Font size", "14", move |v| {
            if v.parse::<f64>().is_ok_and(f64::is_finite) {
                ed.format(|s| s.font_size = v.clone());
            }
        })),
        Box::new(format_control(
            &scope,
            ed,
            "Font family",
            "sans-serif",
            move |v| ed.format(|s| s.font_family = v.clone()),
        )),
        Box::new(format_control(
            &scope,
            ed,
            "Text color (#rrggbb)",
            "#172033",
            move |v| {
                if color(&v).is_some() {
                    ed.format(|s| s.text_color = v.clone());
                }
            },
        )),
        Box::new(format_control(
            &scope,
            ed,
            "Fill color (#rrggbb)",
            "#ffffff",
            move |v| {
                if color(&v).is_some() {
                    ed.format(|s| s.fill_color = v.clone());
                }
            },
        )),
        Box::new(format_control(
            &scope,
            ed,
            "Number format",
            "general",
            move |v| ed.format(|s| s.number_format = v.clone()),
        )),
        Box::new(format_control(
            &scope,
            ed,
            "Column width",
            "80",
            move |value| {
                let Some(width) = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && (24.0..=800.0).contains(v))
                else {
                    return;
                };
                let col = ed.active.get().1.to_string();
                ed.metadata(|content, si, _| {
                    if !content["sheets"][si]["column_widths"].is_object() {
                        content["sheets"][si]["column_widths"] = serde_json::json!({});
                    }
                    content["sheets"][si]["column_widths"][col] = serde_json::json!(width);
                });
            },
        )),
        Box::new(format_control(
            &scope,
            ed,
            "Row height",
            "24",
            move |value| {
                let Some(height) = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && (18.0..=400.0).contains(v))
                else {
                    return;
                };
                let row = ed.active.get().0.to_string();
                ed.metadata(|content, si, _| {
                    if !content["sheets"][si]["row_heights"].is_object() {
                        content["sheets"][si]["row_heights"] = serde_json::json!({});
                    }
                    content["sheets"][si]["row_heights"][row] = serde_json::json!(height);
                });
            },
        )),
    ];
    let extra_formats = formats.split_off(2);
    let export: Vec<Box<dyn Widget>> = ["xlsx", "csv", "pdf", "ods"]
        .into_iter()
        .map(|format| {
            let id = id.clone();
            let token = token.clone();
            let weak = scope.downgrade();
            action(&format!("Export {format}"), move || {
                let Some(scope) = weak.upgrade() else {
                    return;
                };
                let id = id.clone();
                let token = token.clone();
                let _ = scope.spawn(async move {
                    if let Err(error) = export_document(&id, format, &token).await {
                        ed.status.revise(|v| *v = error);
                    }
                });
            })
        })
        .collect();
    let file_save = save.clone();
    let menu_bar = row().class("menubar calc-menubar").fill((
        sapid::dropdown_menu("calc-file")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.file"))
            .item(
                sapid::menu_item("save")
                    .label(crate::i18n::t("menu.file.save"))
                    .on_activate(move || file_save()),
            )
            .item(
                sapid::menu_item("open")
                    .label(crate::i18n::t("menu.file.open"))
                    .on_activate(|| crate::components::ui::go("/files")),
            )
            .item(
                sapid::menu_item("print")
                    .label(crate::i18n::t("menu.file.print"))
                    .on_activate(|| {
                        #[cfg(target_arch = "wasm32")]
                        if let Some(window) = web_sys::window() {
                            let _ = window.print();
                        }
                    }),
            )
            .item(
                sapid::menu_item("close")
                    .label(crate::i18n::t("menu.file.close"))
                    .on_activate(|| crate::components::ui::go("/dashboard")),
            ),
        sapid::dropdown_menu("calc-edit")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.edit"))
            .item(
                sapid::menu_item("undo")
                    .label(crate::i18n::t("menu.edit.undo"))
                    .on_activate(move || ed.history(false)),
            )
            .item(
                sapid::menu_item("redo")
                    .label(crate::i18n::t("menu.edit.redo"))
                    .on_activate(move || ed.history(true)),
            )
            .item(
                sapid::menu_item("copy")
                    .label(crate::i18n::t("menu.edit.copy"))
                    .on_activate(clipboard_command(
                        scope.downgrade(),
                        ed,
                        ClipboardCommand::Copy,
                    )),
            )
            .item(
                sapid::menu_item("cut")
                    .label(crate::i18n::t("menu.edit.cut"))
                    .on_activate(clipboard_command(
                        scope.downgrade(),
                        ed,
                        ClipboardCommand::Cut,
                    )),
            )
            .item(
                sapid::menu_item("paste")
                    .label(crate::i18n::t("menu.edit.paste"))
                    .on_activate(clipboard_command(
                        scope.downgrade(),
                        ed,
                        ClipboardCommand::Paste,
                    )),
            )
            .item(
                sapid::menu_item("clipboard")
                    .label("Clipboard")
                    .on_activate(move || clipboard_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("find")
                    .label(crate::i18n::t("menu.edit.find"))
                    .on_activate(move || find_open.revise(|value| *value = !*value)),
            ),
        sapid::dropdown_menu("calc-view")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.view"))
            .item(
                sapid::menu_item("selection-tools")
                    .label("Selection tools")
                    .on_activate(move || selection_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("sheet-tools")
                    .label("Sheet tools")
                    .on_activate(move || sheets_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("freeze-rows")
                    .label(crate::i18n::t("menu.view.freeze_rows"))
                    .on_activate(move || {
                        ed.metadata(|content, si, range| {
                            let frozen = content["sheets"][si]["frozen_rows"].as_u64().unwrap_or(0);
                            content["sheets"][si]["frozen_rows"] =
                                serde_json::json!(if frozen > 0 { 0 } else { range.start.0 + 1 });
                        })
                    }),
            )
            .item(
                sapid::menu_item("freeze-cols")
                    .label(crate::i18n::t("menu.view.freeze_cols"))
                    .on_activate(move || {
                        ed.metadata(|content, si, range| {
                            let frozen = content["sheets"][si]["frozen_cols"].as_u64().unwrap_or(0);
                            content["sheets"][si]["frozen_cols"] =
                                serde_json::json!(if frozen > 0 { 0 } else { range.start.1 + 1 });
                        })
                    }),
            ),
        sapid::dropdown_menu("calc-insert")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.insert"))
            .item(
                sapid::menu_item("row")
                    .label(crate::i18n::t("menu.insert.row_above"))
                    .on_activate(move || ed.row_column(false, false)),
            )
            .item(
                sapid::menu_item("column")
                    .label(crate::i18n::t("menu.insert.col_left"))
                    .on_activate(move || ed.row_column(true, false)),
            )
            .item(
                sapid::menu_item("sheet")
                    .label(crate::i18n::t("menu.insert.sheet"))
                    .on_activate(move || {
                        let index = ed.book.revise(add_blank_sheet);
                        ed.sheet.revise(|value| *value = index);
                        ed.activate((0, 0));
                        ed.layout.revise(|value| *value += 1);
                    }),
            ),
        sapid::dropdown_menu("calc-format")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.format"))
            .item(
                sapid::menu_item("bold")
                    .label("Bold")
                    .on_activate(move || ed.format(|style| style.bold = !style.bold)),
            )
            .item(
                sapid::menu_item("italic")
                    .label("Italic")
                    .on_activate(move || ed.format(|style| style.italic = !style.italic)),
            )
            .item(
                sapid::menu_item("merge")
                    .label("Merge cells")
                    .on_activate(move || ed.merge(false)),
            )
            .item(
                sapid::menu_item("unmerge")
                    .label("Unmerge cells")
                    .on_activate(move || ed.merge(true)),
            ),
        sapid::dropdown_menu("calc-tools")
            .theme(crate::components::ui::theme())
            .label(crate::i18n::t("menubar.tools"))
            .item(
                sapid::menu_item("data")
                    .label("Data tools")
                    .on_activate(move || advanced_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("comments")
                    .label("Comments")
                    .on_activate(move || comments_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("history")
                    .label("Version history")
                    .on_activate(move || versions_open.revise(|value| *value = !*value)),
            )
            .item(
                sapid::menu_item("ai")
                    .label("AI assist")
                    .on_activate(move || ai_open.revise(|value| *value = !*value)),
            ),
    ));
    col().class("editor-layout calc-editor")
        .fill(crate::components::navbar::Navbar(scope.clone(),auth))
        .fill(row().class("editor-navbar").fill(chrome))
        .fill(menu_bar)
        .fill((
        row().class("calc-ribbon").fill((row().class("calc-format-toolbar").fill(formats),
        sapid::overflow_list("calc-toolbar")
            .theme(crate::components::ui::theme())
            .size(sapid::Size::Small)
            .class("calc-toolbar")
            .initial_visible(8)
            .gap(3.)
            .overflow_label("More tools")
            .items(toolbar))),
        row().class("metadata-toolbar").style(advanced_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            row().class("calc-advanced-formats").fill(extra_formats),
            control("Named range name",metadata_name,move|v|metadata_name.revise(|s|*s=v)),
            sapid::button().theme(crate::components::ui::theme()).label("Define named range").on_press(move||{
                let name=metadata_name.get();if name.trim().is_empty(){return;}
                ed.metadata(|content,si,range|{
                    let sheet_id=content["sheets"][si]["id"].clone();
                    append_array_entry(content,"named_ranges",serde_json::json!({"name":name,"sheet_id":sheet_id,
                        "start":{"row":range.start.0,"col":range.start.1},"end":{"row":range.end.0,"col":range.end.1}}));
                });
            }),
            control("Conditional threshold",threshold,move|v|threshold.revise(|s|*s=v)),
            sapid::button().theme(crate::components::ui::theme()).label("Highlight greater than").on_press(move||{
                let Ok(limit)=threshold.get().parse::<f64>()else{return;};
                ed.metadata(|content,si,range|{
                    let format=style_to_json(&CellStyle{fill_color:"#fff3bf".into(),..Default::default()});
                    append_array_entry(&mut content["sheets"][si],"conditional_formats",serde_json::json!({
                        "range_start":{"row":range.start.0,"col":range.start.1},"range_end":{"row":range.end.0,"col":range.end.1},
                        "rule":{"GreaterThan":limit},"format":format
                    }));
                });
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Validate numbers only").on_press(move||ed.metadata(|content,si,range|{
                append_array_entry(&mut content["sheets"][si],"data_validations",serde_json::json!({
                    "range_start":{"row":range.start.0,"col":range.start.1},"range_end":{"row":range.end.0,"col":range.end.1},
                    "rule":{"DecimalBetween":[-1000000000000000.0,1000000000000000.0]},"error_message":"Enter a number"
                }));
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Freeze rows").on_press(move||ed.metadata(|content,si,range|{
                let current=content["sheets"][si]["frozen_rows"].as_u64().unwrap_or(0);
                content["sheets"][si]["frozen_rows"]=serde_json::json!(if current>0{0}else{range.start.0+1});
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Freeze columns").on_press(move||ed.metadata(|content,si,range|{
                let current=content["sheets"][si]["frozen_cols"].as_u64().unwrap_or(0);
                content["sheets"][si]["frozen_cols"]=serde_json::json!(if current>0{0}else{range.start.1+1});
            })),
            sapid::button().theme(crate::components::ui::theme()).label("Pivot summary").on_press(move||{
                let si=ed.sheet.get();let range=MergedRange{start:ed.anchor.get(),end:ed.end.get()}.normalized();
                ed.book.revise(|b|{
                    b.checkpoint();b.content=b.serialize_raw();
                    let target=b.snapshot.sheets.len();let name=format!("Pivot{target}");
                    let values=pivot_sum_rows(&b.snapshot.cells,si,range.start,range.end);
                    b.snapshot.sheets.push(name.clone());
                    b.snapshot.cells.insert((target,0,0),"Group".into());b.snapshot.cells.insert((target,0,1),"Sum".into());
                    for(i,(label,value))in values.into_iter().take((NUM_ROWS-1)as usize).enumerate(){
                        b.snapshot.cells.insert((target,i as u32+1,0),label);b.snapshot.cells.insert((target,i as u32+1,1),value.to_string());
                    }
                    let target_id=uuid::Uuid::new_v4();let source=b.content["sheets"][si]["id"].clone();
                    append_array_entry(&mut b.content,"sheets",serde_json::json!({"id":target_id,"name":name,"cells":[]}));
                    append_array_entry(&mut b.content,"pivot_tables",serde_json::json!({"id":uuid::Uuid::new_v4(),"name":name,
                        "source_sheet_id":source,"source_start":{"row":range.start.0,"col":range.start.1},"source_end":{"row":range.end.0,"col":range.end.1},
                        "target_sheet_id":target_id,"target":{"row":0,"col":0},"row_field_col":range.start.1,
                        "value_field_col":(range.start.1+1).min(range.end.1),"aggregation":"Sum"}));
                    ed.sheet.revise(|v|*v=target);
                });ed.activate((0,0));ed.layout.revise(|v|*v+=1);
            }),
        )),
        row().class("formula-bar").fill((
            control("Cell address or named range", address, move |v| {
                address.revise(|s| *s = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).name("Go to cell").label("↵").on_press(move || {
                let address = address.get();
                if let Some(point) =
                    parse_cell_ref(&address).filter(|p| p.0 < NUM_ROWS && p.1 < NUM_COLS)
                {
                    ed.activate(point);
                } else if let Some((si, range)) =
                    ed.book.with(|b| resolve_named_range(&b.content, &address))
                {
                    ed.sheet.revise(|v| *v = si);
                    ed.activate(range.start);
                    ed.end.revise(|v| *v = range.end);
                    ed.layout.revise(|v| *v += 1);
                } else {
                    ed.status
                        .revise(|v| *v = "Unknown cell address or named range".into());
                }
            }),
            sapid::button().theme(crate::components::ui::theme())
                .class("formula-icon")
                .label("f(x)")
                .on_press({
                    let weak = scope.downgrade();
                    move || {
                        ed.formula.revise(|v| {
                            if !v.starts_with('=') {
                                v.insert(0, '=');
                            }
                        });
                        if let (Some(scope), Some(node)) = (weak.upgrade(), formula_node.get()) {
                            let _ = scope.action(glory::render::Action::Focus(node));
                        }
                    }
                }),
            input()
                .name("Formula")
                .class("calc-formula-input formula-input")
                .value(ed.formula)
                .node_ref(formula_node)
                .semantics(Some(Box::new(Semantics {
                    id: Some(SemanticId::new("calc-formula-input").expect("static id")),
                    ..Default::default()
                })))
                .on(EventKind::ValueChanged, move |event| {
                    if let glory::render::Event::ValueChanged { value, .. } = event {
                        ed.formula.revise(|v| *v = value);
                    }
                })
                .on(EventKind::Commit, move |event| {
                    if let glory::render::Event::Commit { value, .. } = event {
                        ed.commit(value);
                    }
                })
                .on(EventKind::Keyboard, move |event| {
                    if let glory::render::Event::Keyboard { command, .. } = event {
                        match command {
                            glory::render::KeyboardCommand::Submit => {
                                ed.commit(ed.formula.get());
                                let (r, c) = ed.active.get();
                                ed.activate(((r + 1).min(NUM_ROWS - 1), c));
                            }
                            glory::render::KeyboardCommand::Dismiss => {
                                ed.activate(ed.active.get());
                                formula_reconcile.revise(|revision| {
                                    *revision = revision.wrapping_add(1);
                                });
                            }
                            _ => {}
                        }
                    }
                }),
            sapid::button().theme(crate::components::ui::theme())
                .name("Apply formula").label("✓")
                .on_press(move || ed.commit(ed.formula.get())),
        )),
        row().class("clipboard-toolbar").style(clipboard_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            sapid::text_field().theme(crate::components::ui::theme()).name("Selection data (tab separated)").multiline(true).value(clipboard)
                .on_value_change(move|value|clipboard.revise(|v|*v=value)),
            sapid::button().theme(crate::components::ui::theme()).label("Load selection data").on_press(move||clipboard.revise(|v|*v=selection_text(ed))),
            sapid::button().theme(crate::components::ui::theme()).label("Paste selection data").on_press(move|| {
                #[cfg(target_arch = "wasm32")]
                clipboard_events::request_paste_text(ed, clipboard.get());
            }),
        )),
        row().class("selection-toolbar").style(selection_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            control("Selection range", selection, move |v| {
                selection.revise(|s| *s = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Select range").on_press(move || {
                let value = selection.get();
                let mut parts = value.split(':');
                if let Some(a) = parts.next().and_then(parse_cell_ref) {
                    let z = parts.next().and_then(parse_cell_ref).unwrap_or(a);
                    if a.0 < NUM_ROWS && z.0 < NUM_ROWS && a.1 < NUM_COLS && z.1 < NUM_COLS {
                        ed.activate(a);
                        ed.end.revise(|v| *v = z);
                    }
                }
            }),
            sapid::checkbox().theme(crate::components::ui::theme())
                .label("Extend selection")
                .checked(ed.extend)
                .on_checked_change({
                    move |value| {
                        ed.extend.revise(|v| *v = value);
                    }
                }),
            sapid::checkbox().theme(crate::components::ui::theme()).label("Add cells to selection").checked(ed.additive)
                .on_checked_change(move|value|ed.additive.revise(|v|*v=value)),
            control("Column filter",filter_query,move|value|filter_query.revise(|v|*v=value)),
            sapid::button().theme(crate::components::ui::theme()).label("Filter column").on_press(move||{
                if ed.book.with(|b|b.snapshot.merged_cells.get(&ed.sheet.get()).is_some_and(|v|!v.is_empty())) {
                    ed.status.revise(|v|*v="Unmerge cells before filtering rows".into());return;
                }
                ed.filter.revise(|v|*v=Some((ed.active.get().1,filter_query.get())));ed.layout.revise(|v|*v+=1);
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Clear filter").on_press(move||{ed.filter.revise(|v|*v=None);ed.layout.revise(|v|*v+=1);}),
        )),
        when(loaded, {
            let weak = scope.downgrade();
            move || {
                let weak = weak.clone();
                each(scope_bond(&weak, move || vec![ed.layout.get()]))
                    .key(|generation| *generation)
                    .children(move |_| {
                        widget(move|scope| grid_context(scope,ed).build(scope))
                    })
            }
        }),
        row().class("sheet-tabs").fill((
            each(
                scope.bond(move || {
                    ed.book.with(|b| {
                        b.snapshot
                            .sheets
                            .iter()
                            .enumerate()
                            .map(|(i, n)| (i, n.clone()))
                            .collect::<Vec<_>>()
                    })
                }))
                .key(|(i, _)| *i)
                .children(move |row| {
                    let sheet = row.value;
                    let index = sheet.get().0;
                    sapid::button().theme(crate::components::ui::theme())
                        .variant(sapid::ButtonVariant::Secondary).size(sapid::Size::Small)
                        .class(ed.sheet.map(move |active|if *active==index { "sheet-tab active".to_string() } else { "sheet-tab".to_string() }))
                        .label(scope_label(sheet)).on_press(move || {
                        ed.sheet.revise(|v| *v = index);
                        ed.activate((0, 0));
                        ed.layout.revise(|v| *v += 1);
                    })
                }),
            sapid::button().theme(crate::components::ui::theme()).name("Add sheet").label("+").class("sheet-tab-add").on_press(move || {
                let index = ed.book.revise(add_blank_sheet);
                ed.sheet.revise(|value| *value = index);
                ed.activate((0, 0));
                ed.layout.revise(|value| *value += 1);
            }),
            row().class("sheet-management").style(sheets_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            sapid::button().theme(crate::components::ui::theme()).label("Duplicate sheet").on_press(move||ed.duplicate_sheet()),
            sapid::button().theme(crate::components::ui::theme()).label("Move sheet left").on_press(move||ed.move_sheet(false)),
            sapid::button().theme(crate::components::ui::theme()).label("Move sheet right").on_press(move||ed.move_sheet(true)),
            control("Sheet name", sheet_name, move |v| {
                sheet_name.revise(|n| *n = v)
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Rename sheet").on_press(move || {
                let si = ed.sheet.get();
                let name = sheet_name.get();
                if !name.trim().is_empty() {
                    ed.book.revise(|b| {
                        b.checkpoint();
                        b.snapshot.sheets[si] = name;
                    });
                }
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Delete sheet").on_press(move || {
                let si = ed.sheet.get();
                ed.book.revise(|b| {
                    if b.snapshot.sheets.len() < 2 {
                        return;
                    }
                    b.checkpoint();
                    b.snapshot.sheets.remove(si);
                    b.snapshot.cells = std::mem::take(&mut b.snapshot.cells)
                        .into_iter()
                        .filter_map(|((s, r, c), v)| {
                            (s != si).then_some(((if s > si { s - 1 } else { s }, r, c), v))
                        })
                        .collect();
                    b.snapshot.styles = std::mem::take(&mut b.snapshot.styles)
                        .into_iter()
                        .filter_map(|((s, r, c), v)| {
                            (s != si).then_some(((if s > si { s - 1 } else { s }, r, c), v))
                        })
                        .collect();
                    delete_sheet_from_merged(&mut b.snapshot.merged_cells, si);
                    if let Some(sheets) = b.content.get_mut("sheets").and_then(|s| s.as_array_mut())
                        && si < sheets.len()
                    {
                        sheets.remove(si);
                    }
                });
                ed.sheet.revise(|v| *v = 0);
                ed.activate((0, 0));
                ed.layout.revise(|v| *v += 1);
            }),)),
        )),
        row().class("find-toolbar").style(find_open.map(|open|Style::new().display(if *open { Display::Flex } else { Display::None }))).fill((
            control("Find", find, move |v| find.revise(|s| *s = v)),
            control("Replace with", replace, move |v| replace.revise(|s| *s = v)),
            sapid::button().theme(crate::components::ui::theme()).label("Find next").on_press(move || {
                let query = find.get();
                if query.is_empty() {
                    return;
                }
                let si = ed.sheet.get();
                let active = ed.active.get();
                let mut found = ed.book.with(|b| {
                    b.snapshot
                        .cells
                        .iter()
                        .filter(|((s, _, _), v)| *s == si && v.contains(&query))
                        .map(|((_, r, c), _)| (*r, *c))
                        .collect::<Vec<_>>()
                });
                found.sort_unstable();
                if let Some(point) = found.iter().find(|p| **p > active).or(found.first()) {
                    ed.activate(*point);
                } else {
                    ed.status.revise(|v| *v = "No matches".into());
                }
            }),
            sapid::button().theme(crate::components::ui::theme()).label("Replace all").on_press(move || {
                let query = find.get();
                let replacement = replace.get();
                if !query.is_empty() {
                    ed.book.revise(|b| {
                        b.checkpoint();
                        for value in b.snapshot.cells.values_mut() {
                            *value = value.replace(&query, &replacement);
                        }
                    });
                }
            }),
        )),
        when(show_chart, move || widget(move|scope|chart_view(scope,ed).build(scope))),
        group().fill((crate::components::comments_pane::CommentsPane(scope.clone(),auth,id.clone(),comments_open),
        crate::components::version_history::VersionHistory(scope.clone(),auth,id.clone(),versions_open, version),
        when(ai_open,{let id=id.clone();move||{let id=id.clone();widget(move|scope|crate::components::ai_assist::AiAssist(scope.clone(),auth,id.clone()).build(scope))}}),)),
        row().class("status-bar").fill((
            text().content(scope.bond(move || {
                let _ = ed.calculation_epoch.get();
                let (r, c) = ed.active.get();
                let si = ed.sheet.get();
                let selected = ed.selected();
                ed.book.with(|b| {
                    let values: Vec<_> = selected
                        .iter()
                        .filter_map(|(r, c)| {
                            b.numeric(si, *r, *c)
                        })
                        .collect();
                    let sum: f64 = values.iter().sum();
                    let avg = if values.is_empty() {
                        0.0
                    } else {
                        sum / values.len() as f64
                    };
                    format!(
                        "Cell: {} · Sum: {} · Average: {} · Count: {}",
                        cell_ref(r, c),
                        format_number_default(sum),
                        format_number_default(avg),
                        values.len()
                    )
                })
            })),
            row().fill(export),
        )),
    ))
}

fn scope_bond<T: 'static>(weak: &glory::ui::WeakScope, read: impl Fn() -> T + 'static) -> Bond<T> {
    weak.upgrade().expect("live workbook scope").bond(read)
}
fn scope_label(sheet: Cage<(usize, String)>) -> Vary<String> {
    // Each owns this Cage; read through its runtime owner for live tab renames.
    sheet.map(|(_, name)| name.clone()).into()
}

fn selection_text(ed: Editor) -> String {
    let si = ed.sheet.get();
    let range = MergedRange {
        start: ed.anchor.get(),
        end: ed.end.get(),
    }
    .normalized();
    ed.book.with(|b| {
        (range.start.0..=range.end.0)
            .map(|r| {
                (range.start.1..=range.end.1)
                    .map(|c| {
                        b.snapshot
                            .cells
                            .get(&(si, r, c))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    Some(Color((n >> 16) as u8, (n >> 8) as u8, n as u8, 255))
}
fn add_blank_sheet(book: &mut Book) -> usize {
    book.checkpoint();
    book.content = book.serialize_raw();
    let index = book.snapshot.sheets.len();
    let mut suffix = index + 1;
    let name = loop {
        let name = format!("Sheet{suffix}");
        if !book.snapshot.sheets.contains(&name) {
            break name;
        }
        suffix += 1;
    };
    book.snapshot.sheets.push(name.clone());
    append_array_entry(
        &mut book.content,
        "sheets",
        serde_json::json!({
            "id": uuid::Uuid::new_v4(), "name": name, "cells": [],
            "column_widths": {}, "row_heights": {}, "frozen_rows": 0, "frozen_cols": 0,
            "conditional_formats": [], "data_validations": [], "charts": [], "merged_cells": []
        }),
    );
    index
}

fn sheet_axis_size(sheet: &serde_json::Value, column: bool, index: u32) -> f64 {
    let (key, default, maximum) = if column {
        ("column_widths", 80.0, 800.0)
    } else {
        ("row_heights", 24.0, 400.0)
    };
    sheet[key][index.to_string()]
        .as_f64()
        .filter(|value| value.is_finite())
        .unwrap_or(default)
        .clamp(24.0, maximum)
}

fn cell_style(style: &CellStyle) -> Style {
    let mut out = Style::new()
        .width(Length::Percent(100.0))
        .min_height(Length::Px(24.0));
    if style.bold {
        out.font_weight = Some(700.0);
    }
    if let Some(value) = color(&style.text_color) {
        out.color = Some(value);
    }
    if let Some(value) = color(&style.fill_color) {
        out.background = Some(value);
    }
    if let Some(value) = style
        .font_size
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
    {
        out.font_size = Some(value.clamp(6.0, 96.0));
    }
    if !style.font_family.is_empty() {
        out.font_family = Some(vec![Family::Named(style.font_family.clone())]);
    }
    out
}
fn grid_context(scope: &Scope, ed: Editor) -> impl Widget {
    let mut menu = sapid::context_menu("calc-context")
        .theme(crate::components::ui::theme())
        .size(sapid::Size::Small)
        .name("Cell actions")
        .label("Cell actions")
        .target_name("Spreadsheet actions")
        .target(grid(scope, ed));
    #[cfg(target_arch = "wasm32")]
    {
        for (key, label, cut) in [("copy", "Copy", false), ("cut", "Cut", true)] {
            let weak = scope.downgrade();
            menu = menu.item(sapid::menu_item(key).label(label).on_activate(move || {
                if let Some(scope) = weak.upgrade() {
                    clipboard_events::request_copy(&scope, ed, cut);
                }
            }));
        }
        let weak = scope.downgrade();
        menu = menu
            .item(
                sapid::menu_item("paste")
                    .label("Paste")
                    .on_activate(move || {
                        if let Some(scope) = weak.upgrade() {
                            clipboard_events::request_paste(&scope, ed);
                        }
                    }),
            )
            .separator("clipboard");
    }
    menu = menu
        .item(
            sapid::menu_item("undo")
                .label("Undo")
                .on_activate(move || ed.history(false)),
        )
        .item(
            sapid::menu_item("redo")
                .label("Redo")
                .on_activate(move || ed.history(true)),
        )
        .item(
            sapid::menu_item("clear")
                .label("Clear cells")
                .on_activate(move || {
                    let cells = ed.selected();
                    let si = ed.sheet.get();
                    ed.book.revise(|b| {
                        b.checkpoint();
                        for (r, c) in cells {
                            b.snapshot.cells.remove(&(si, r, c));
                        }
                    });
                    ed.read_active();
                }),
        )
        .separator("structure")
        .item(
            sapid::menu_item("insert-row")
                .label("Insert row")
                .on_activate(move || ed.row_column(false, false)),
        )
        .item(
            sapid::menu_item("insert-column")
                .label("Insert column")
                .on_activate(move || ed.row_column(true, false)),
        )
        .item(
            sapid::menu_item("delete-row")
                .label("Delete row")
                .on_activate(move || ed.row_column(false, true)),
        )
        .item(
            sapid::menu_item("delete-column")
                .label("Delete column")
                .on_activate(move || ed.row_column(true, true)),
        );
    view().class("calc-grid-workspace").fill(menu)
}

fn grid(scope: &Scope, ed: Editor) -> impl Widget {
    let si = ed.sheet.get();
    let ranges = ed.book.with(|b| {
        b.snapshot
            .merged_cells
            .get(&si)
            .cloned()
            .unwrap_or_default()
    });
    let mut head = sapid::table_row("head").cell(
        sapid::table_header_cell("corner")
            .class("corner-cell row-header")
            .content(move || {
                glory::ui::button()
                    .class("calc-grid-header calc-select-all")
                    .name("Select all cells")
                    .label("◢")
                    .on(EventKind::Activate, move |_| {
                        ed.select_range(MergedRange {
                            start: (0, 0),
                            end: (NUM_ROWS - 1, NUM_COLS - 1),
                        })
                    })
            }),
    );
    for c in 0..NUM_COLS {
        head = head.cell(
            sapid::table_header_cell(format!("h{c}"))
                .style(scope.bond(move || {
                    ed.book.with(|book| {
                        let sheet = &book.content["sheets"][si];
                        let width = ed.dimension(sheet, true, c);
                        let mut style = Style::new()
                            .width(Length::Px(width))
                            .min_width(Length::Px(width));
                        if (c as u64) < sheet["frozen_cols"].as_u64().unwrap_or(0) {
                            style.position = Some(Position::Sticky);
                            style.left = Some(Length::Px(
                                80.0 + (0..c)
                                    .map(|prior| ed.dimension(sheet, true, prior))
                                    .sum::<f64>(),
                            ));
                        }
                        style
                    })
                }))
                .class("col-header")
                .content(move || {
                    group().fill((
                        glory::ui::button()
                            .class("calc-grid-header calc-column-header")
                            .name(format!("Select column {}", col_letter(c)))
                            .label(col_letter(c).to_string())
                            .on(EventKind::Activate, move |_| {
                                ed.select_range(MergedRange {
                                    start: (0, c),
                                    end: (NUM_ROWS - 1, c),
                                })
                            }),
                        glory::ui::button()
                            .class("calc-column-resize")
                            .name(format!("Resize column {}", col_letter(c)))
                            .label("")
                            .focus(glory::render::FocusMode::Programmatic),
                    ))
                }),
        );
    }
    let mut nodes = HashMap::new();
    let mut body = sapid::table_body();
    for r in 0..NUM_ROWS {
        if r > 0
            && ed.filter.with(|filter| {
                filter.as_ref().is_some_and(|(col, query)| {
                    ed.book.with(|b| {
                        !b.snapshot
                            .cells
                            .get(&(si, r, *col))
                            .is_some_and(|v| v.to_lowercase().contains(&query.to_lowercase()))
                    })
                })
            })
        {
            continue;
        }
        let mut row = sapid::table_row(format!("r{r}")).cell(
            sapid::table_row_header(format!("rh{r}"))
                .class("row-header")
                .style(scope.bond(move || {
                    ed.book.with(|book| {
                        let sheet = &book.content["sheets"][si];
                        let mut style = Style::new();
                        if (r as u64) < sheet["frozen_rows"].as_u64().unwrap_or(0) {
                            let preceding_height: f64 = (0..r)
                                .filter(|prior| {
                                    *prior == 0
                                        || !ed.filter.with(|filter| {
                                            filter.as_ref().is_some_and(|(column, query)| {
                                                !book
                                                    .snapshot
                                                    .cells
                                                    .get(&(si, *prior, *column))
                                                    .is_some_and(|value| {
                                                        value
                                                            .to_lowercase()
                                                            .contains(&query.to_lowercase())
                                                    })
                                            })
                                        })
                                })
                                .map(|prior| ed.dimension(sheet, false, prior))
                                .sum();
                            style.position = Some(Position::Sticky);
                            style.top = Some(Length::Px(24.0 + preceding_height));
                        }
                        style
                    })
                }))
                .content(move || {
                    group().fill((
                        glory::ui::button()
                            .class("calc-grid-header calc-row-header")
                            .name(format!("Select row {}", r + 1))
                            .label((r + 1).to_string())
                            .on(EventKind::Activate, move |_| {
                                ed.select_range(MergedRange {
                                    start: (r, 0),
                                    end: (r, NUM_COLS - 1),
                                })
                            }),
                        glory::ui::button()
                            .class("calc-row-resize")
                            .name(format!("Resize row {}", r + 1))
                            .label("")
                            .focus(glory::render::FocusMode::Programmatic),
                    ))
                }),
        );
        for c in 0..NUM_COLS {
            let merged = ranges.iter().find(|range| range.contains((r, c)));
            if merged.is_some_and(|range| range.start != (r, c)) {
                continue;
            }
            let node = scope.cage(None);
            nodes.insert((r, c), node);
            let name = cell_ref(r, c);
            let cell_scope = scope.downgrade();
            let mut cell = sapid::table_cell(format!("c{c}"))
                .style(scope.bond(move || {
                    ed.book.with(|b| {
                        let sheet = &b.content["sheets"][si];
                        let width = ed.dimension(sheet, true, c);
                        let height = ed.dimension(sheet, false, r);
                        let mut style = Style::new()
                            .width(Length::Px(width))
                            .min_width(Length::Px(width))
                            .height(Length::Px(height));
                        if (r as u64) < sheet["frozen_rows"].as_u64().unwrap_or(0) {
                            style.position = Some(Position::Sticky);
                            let preceding_height: f64 = (0..r)
                                .filter(|prior| {
                                    *prior == 0
                                        || !ed.filter.with(|filter| {
                                            filter.as_ref().is_some_and(|(col, query)| {
                                                !b.snapshot
                                                    .cells
                                                    .get(&(si, *prior, *col))
                                                    .is_some_and(|value| {
                                                        value
                                                            .to_lowercase()
                                                            .contains(&query.to_lowercase())
                                                    })
                                            })
                                        })
                                })
                                .map(|prior| ed.dimension(sheet, false, prior))
                                .sum();
                            style.top = Some(Length::Px(24.0 + preceding_height));
                        }
                        if (c as u64) < sheet["frozen_cols"].as_u64().unwrap_or(0) {
                            style.position = Some(Position::Sticky);
                            let preceding_width: f64 =
                                (0..c).map(|prior| ed.dimension(sheet, true, prior)).sum();
                            style.left = Some(Length::Px(80.0 + preceding_width));
                        }
                        style
                    })
                }))
                .class(scope.bond(move || {
                    let active = ed.active.get() == (r, c);
                    let selected = in_selection((r, c), ed.anchor.get(), ed.end.get())
                        || ed.extra.with(|v| v.contains(&(r, c)));
                    let extra = ed.book.with(|b| {
                        let style = b
                            .snapshot
                            .styles
                            .get(&(si, r, c))
                            .cloned()
                            .unwrap_or_default();
                        format!(
                            "{}{}{}{}{}{}{}{}",
                            if style.bold { " cell-bold" } else { "" },
                            if style.italic { " cell-italic" } else { "" },
                            if style.underline {
                                " cell-underline"
                            } else {
                                ""
                            },
                            if style.borders.top { " border-t" } else { "" },
                            if style.borders.right { " border-r" } else { "" },
                            if style.borders.bottom {
                                " border-b"
                            } else {
                                ""
                            },
                            if style.borders.left { " border-l" } else { "" },
                            match style.align.as_str() {
                                "left" => " cell-align-left",
                                "center" => " cell-align-center",
                                "right" => " cell-align-right",
                                _ => "",
                            }
                        )
                    });
                    let range = ed.range();
                    let mut edges = String::new();
                    if selected {
                        if r == range.start.0 {
                            edges.push_str(" selection-top");
                        }
                        if r == range.end.0 {
                            edges.push_str(" selection-bottom");
                        }
                        if c == range.start.1 {
                            edges.push_str(" selection-left");
                        }
                        if c == range.end.1 {
                            edges.push_str(" selection-right");
                        }
                    }
                    if ed
                        .fill_preview
                        .get()
                        .is_some_and(|range| range.contains((r, c)))
                    {
                        edges.push_str(" calc-fill-preview");
                    }
                    format!(
                        "cell{}{}{}{}",
                        if active { " active" } else { "" },
                        if selected { " selected" } else { "" },
                        extra,
                        edges
                    )
                }))
                .content(move || {
                    let scope = cell_scope.upgrade().expect("live spreadsheet cell");
                    let value = scope.bond(move || {
                        let _ = ed.calculation_epoch.get();
                        ed.book.with(|b| {
                            let value = b.display(si, r, c);
                            let style = b
                                .snapshot
                                .styles
                                .get(&(si, r, c))
                                .cloned()
                                .unwrap_or_default();
                            format_number(&value, &style.number_format)
                        })
                    });
                    let input_name = format!("Edit {name}");
                    let name = name.clone();
                    let handle_visible = scope.bond(move || {
                        let range = ed.range();
                        ed.extra.with(Vec::is_empty)
                            && ed.editing.get().is_none()
                            && range.end == (r, c)
                            && !ed.book.with(|b| {
                                b.snapshot.merged_cells.get(&si).is_some_and(|ranges| {
                                    ranges.iter().any(|merged| merged.overlaps(range))
                                })
                            })
                    });
                    let content = when(
                        ed.editing.map(move |target| *target == Some((si, r, c))),
                        move || {
                            let input_name = input_name.clone();
                            widget(move |scope| {
                                let edit_node = scope.cage(None);
                                let weak = scope.downgrade();
                                scope.effect(move || {
                                    if let (Some(scope), Some(node)) =
                                        (weak.upgrade(), edit_node.get())
                                    {
                                        // Focus in the same host commit that mounts the input,
                                        // so fast typing never falls between two focus targets.
                                        let _ = scope.action(glory::render::Action::Focus(node));
                                        let end = ed.formula.get_untracked().encode_utf16().count();
                                        let _ = scope.set_input_selection(
                                            node,
                                            glory::render::TextSelection {
                                                start: end,
                                                end,
                                                ..Default::default()
                                            },
                                        );
                                    }
                                });
                                input()
                                    .name(input_name.clone())
                                    .node_ref(edit_node)
                                    .class("calc-cell-input")
                                    .value(ed.formula)
                                    .on(EventKind::ValueChanged, move |event| {
                                        if let glory::render::Event::ValueChanged { value, .. } =
                                            event
                                            && ed.editing.get() == Some((si, r, c))
                                        {
                                            ed.formula.revise(|v| *v = value);
                                        }
                                    })
                                    .on(EventKind::Commit, move |event| {
                                        if ed.editing.get() == Some((si, r, c))
                                            && let glory::render::Event::Commit { value, .. } =
                                                event
                                        {
                                            ed.formula.revise(|v| *v = value);
                                            ed.finish_edit(false);
                                        }
                                    })
                                    .build(scope)
                            })
                        },
                    )
                    .otherwise(move || {
                        glory::ui::button()
                            .label(value)
                            .name(name.clone())
                            .class("cell-button")
                            .node_ref(node)
                            .style(scope.bond(move || {
                                ed.book.with(|b| {
                                    let base = b
                                        .snapshot
                                        .styles
                                        .get(&(si, r, c))
                                        .cloned()
                                        .unwrap_or_default();
                                    let raw = b
                                        .snapshot
                                        .cells
                                        .get(&(si, r, c))
                                        .map(String::as_str)
                                        .unwrap_or("");
                                    let style = conditional_style_for(&b.content, si, r, c, raw)
                                        .unwrap_or(base);
                                    cell_style(&style)
                                })
                            }))
                            .on(EventKind::Activate, move |_| {
                                ed.select_point((r, c), false, false)
                            })
                    });
                    group().fill((
                        content,
                        when(handle_visible, || {
                            glory::ui::button()
                                .name("Drag to fill")
                                .label("+")
                                .class("calc-fill-handle")
                                .focus(glory::render::FocusMode::Programmatic)
                        }),
                    ))
                });
            if let Some(range) = merged {
                cell = cell
                    .row_span(range.row_span() as u16)
                    .column_span(range.col_span() as u16);
            }
            row = row.cell(cell);
        }
        body = body.row(row);
    }
    let weak = scope.downgrade();
    view()
        .role(Role::Grid)
        .semantics(Some(Box::new(Semantics {
            grid_navigation: Some(glory::render::Direction::Ltr),
            grid_multiple: true,
            ..Default::default()
        })))
        .on(EventKind::GridNavigate, move |event| {
            if let glory::render::Event::GridNavigate { command, .. } = event {
                let (r, c) = ed.active.get();
                let step = match command {
                    GridCommand::Left => Some((0, -1)),
                    GridCommand::Right => Some((0, 1)),
                    GridCommand::Up => Some((-1, 0)),
                    GridCommand::Down => Some((1, 0)),
                    _ => None,
                };
                let target = match command {
                    GridCommand::Left => (r, c.saturating_sub(1)),
                    GridCommand::Right => (r, (c + 1).min(NUM_COLS - 1)),
                    GridCommand::Up => (r.saturating_sub(1), c),
                    GridCommand::Down => ((r + 1).min(NUM_ROWS - 1), c),
                    GridCommand::FirstInRow => (r, 0),
                    GridCommand::LastInRow => (r, NUM_COLS - 1),
                    GridCommand::PreviousPage => (r.saturating_sub(10), c),
                    GridCommand::NextPage => ((r + 10).min(NUM_ROWS - 1), c),
                    GridCommand::PreviousLargePage => (0, c),
                    GridCommand::NextLargePage => (NUM_ROWS - 1, c),
                };
                if let Some((dr, dc)) = step {
                    ed.step(dr, dc, false);
                } else {
                    ed.activate(target);
                }
                if let (Some(scope), Some(node)) = (
                    weak.upgrade(),
                    nodes.get(&ed.active.get()).and_then(|n| n.get()),
                ) {
                    let _ = scope.action(glory::render::Action::Focus(node));
                }
            }
        })
        .class("sheet-scroll spreadsheet")
        .style(Style::new().overflow_x(Overflow::Auto))
        .fill(
            sapid::table("workbook")
                .theme(crate::components::ui::theme())
                .class("sheet-grid")
                .style(scope.bond(move || {
                    ed.book.with(|book| {
                        let sheet = &book.content["sheets"][si];
                        let width: f64 = 80.0
                            + (0..NUM_COLS)
                                .map(|column| ed.dimension(sheet, true, column))
                                .sum::<f64>();
                        Style::new().width(Length::Px(width))
                    })
                }))
                .name("Spreadsheet")
                .bordered(true)
                .head(sapid::table_head().row(head))
                .body(body),
        )
}
fn chart_view(scope: &Scope, ed: Editor) -> impl Widget {
    let data = scope.bond(move || {
        let range = MergedRange {
            start: ed.anchor.get(),
            end: ed.end.get(),
        }
        .normalized();
        let si = ed.sheet.get();
        let _ = ed.calculation_epoch.get();
        ed.book.with(|b| {
            let points: Vec<_> = (range.start.0..=range.end.0)
                .take(128)
                .map(|r| {
                    let label = b
                        .snapshot
                        .cells
                        .get(&(si, r, range.start.1))
                        .cloned()
                        .unwrap_or_else(|| (r + 1).to_string());
                    let label: String = label
                        .replace(['\n', '\r', '\0'], " ")
                        .chars()
                        .scan(0usize, |bytes, ch| {
                            *bytes += ch.len_utf8();
                            (*bytes <= 256).then_some(ch)
                        })
                        .collect();
                    let value = b.numeric(si, r, range.end.1).filter(|value| {
                        value.is_finite() && value.abs() <= sapid::charts::MAX_CHART_VALUE
                    });
                    (label, value)
                })
                .collect();
            sapid::charts::ChartData {
                categories: points
                    .iter()
                    .enumerate()
                    .map(|(i, (label, _))| {
                        sapid::charts::ChartCategory::new(
                            format!("r{i}"),
                            if label.is_empty() {
                                "Row".into()
                            } else {
                                label.clone()
                            },
                        )
                    })
                    .collect(),
                series: vec![sapid::charts::ChartSeries::new(
                    "values",
                    "Values",
                    points.into_iter().map(|(_, v)| v),
                )],
            }
        })
    });
    sapid::charts::bar_chart("selection-chart")
        .theme(crate::components::ui::theme())
        .name("Selection chart")
        .data(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_sheet_keeps_ids_stable_and_uses_an_available_name() {
        let mut book = Book::default();
        book.snapshot.sheets = vec!["Sheet1".into(), "Sheet3".into()];
        let index = add_blank_sheet(&mut book);
        assert_eq!(index, 2);
        assert_eq!(book.snapshot.sheets[index], "Sheet4");
        let ids = book.serialize()["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sheet| sheet["id"].clone())
            .collect::<Vec<_>>();
        assert!(ids.iter().all(|id| {
            id.as_str()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        }));
        book.snapshot
            .cells
            .insert((index, 0, 0), "New value".into());
        let after = book.serialize()["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sheet| sheet["id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, after);
        assert_eq!(book.undo.len(), 1);
        assert!(book.dirty);
    }

    #[test]
    fn frozen_dimensions_follow_resized_axes_and_clamp_invalid_sizes() {
        let sheet = serde_json::json!({"column_widths":{"0":125,"1":64,"2":-4},"row_heights":{"0":47,"1":29,"2":8}});
        let left = 80.0
            + (0..2)
                .map(|column| sheet_axis_size(&sheet, true, column))
                .sum::<f64>();
        let top = 24.0
            + (0..2)
                .map(|row| sheet_axis_size(&sheet, false, row))
                .sum::<f64>();
        assert_eq!(left, 269.0);
        assert_eq!(top, 100.0);
        assert_eq!(sheet_axis_size(&sheet, true, 2), 24.0);
        assert_eq!(sheet_axis_size(&sheet, false, 2), 24.0);
        assert_eq!(sheet_axis_size(&sheet, true, 3), 80.0);
        assert_eq!(sheet_axis_size(&sheet, false, 3), 24.0);
    }

    #[test]
    fn workbook_save_preserves_ids_metadata_and_format_only_cells() {
        let mut book = Book::default();
        book.hydrate(serde_json::json!({
            "name":"Budget", "named_ranges":[{"name":"Revenue"}],
            "sheets":[{"id":"stable-sheet", "name":"Sheet1", "cells":[],
                "column_widths":{"0":180}, "frozen_rows":2,
                "conditional_formats":[{"rule":"IsEmpty"}], "charts":[{"title":"Sales"}]}]
        }));
        book.snapshot.cells.insert((0, 0, 0), "=SUM(A2:A3)".into());
        book.snapshot.styles.insert(
            (0, 3, 2),
            CellStyle {
                bold: true,
                ..Default::default()
            },
        );
        let output = book.serialize();
        assert_eq!(output["sheets"][0]["id"], "stable-sheet");
        assert_eq!(output["sheets"][0]["column_widths"]["0"], 180);
        assert_eq!(output["sheets"][0]["frozen_rows"], 2);
        assert_eq!(output["sheets"][0]["charts"][0]["title"], "Sales");
        assert_eq!(output["named_ranges"][0]["name"], "Revenue");
        let (_, cells, styles, _) = hydrate_workbook(&output);
        assert_eq!(cells[&(0, 0, 0)], "=SUM(A2:A3)");
        assert!(styles[&(0, 3, 2)].bold);
    }

    #[test]
    fn merged_workbook_grid_mounts_through_semantic_controls() {
        let app = App::memory();
        app.mount(widget(move |scope| {
            let mut book = Book::default();
            book.snapshot.cells.insert((0, 0, 0), "=SUM(A2:A3)".into());
            book.snapshot.merged_cells.insert(
                0,
                vec![MergedRange {
                    start: (0, 0),
                    end: (1, 1),
                }],
            );
            let ed = Editor {
                book: scope.cage(book),
                calculation_epoch: scope.cage(0),
                sheet: scope.cage(0),
                active: scope.cage((0, 0)),
                anchor: scope.cage((0, 0)),
                end: scope.cage((0, 0)),
                extra: scope.cage(Vec::new()),
                extend: scope.cage(false),
                additive: scope.cage(false),
                filter: scope.cage(None),
                formula: scope.cage(String::new()),
                editing: scope.cage(None),
                fill_preview: scope.cage(None),
                resize_preview: scope.cage(None),
                status: scope.cage(String::new()),
                layout: scope.cage(0),
            };
            grid(scope, ed).build(scope)
        }))
        .expect("valid merged grid and keyboard boundary");
        assert!(app.stats().nodes > 1300);
        app.clear().expect("grid disposes its Owners");
        assert_eq!(app.stats().nodes, 0);
    }

    fn fill_test_editor(scope: &Scope, book: Book) -> Editor {
        Editor {
            book: scope.cage(book),
            calculation_epoch: scope.cage(0),
            sheet: scope.cage(0),
            active: scope.cage((0, 0)),
            anchor: scope.cage((0, 0)),
            end: scope.cage((0, 0)),
            extra: scope.cage(Vec::new()),
            extend: scope.cage(false),
            additive: scope.cage(false),
            filter: scope.cage(None),
            formula: scope.cage(String::new()),
            editing: scope.cage(None),
            fill_preview: scope.cage(None),
            resize_preview: scope.cage(None),
            status: scope.cage(String::new()),
            layout: scope.cage(0),
        }
    }

    #[test]
    fn drag_fill_copies_formula_and_styles_as_one_undo_transaction() {
        let app = App::memory();
        app.mount(widget(move |scope| {
            let mut book = Book {
                content: serde_json::json!({"sheets":[{"id":"stable-sheet","name":"Sheet1","cells":[],"column_widths":{"0":125}}]}),
                ..Default::default()
            };
            book.snapshot.cells.extend([
                ((0, 0, 0), "7".into()),
                ((0, 0, 1), "=A1+$D$1".into()),
                ((0, 0, 3), "100".into()),
                ((0, 1, 0), "99".into()),
                ((0, 1, 1), "old target".into()),
            ]);
            let source_style = CellStyle { bold: true, fill_color: "#dbeafe".into(), ..Default::default() };
            let formula_style = CellStyle { italic: true, align: "right".into(), ..Default::default() };
            book.snapshot.styles.insert((0, 0, 0), source_style.clone());
            book.snapshot.styles.insert((0, 0, 1), formula_style.clone());
            book.snapshot.styles.insert((0, 1, 0), CellStyle { underline: true, ..Default::default() });
            let original_snapshot = book.snapshot.clone();
            let original_content = book.content.clone();
            book.undo.push((original_snapshot.clone(), original_content.clone()));
            book.redo.push((original_snapshot.clone(), original_content.clone()));
            let ed = fill_test_editor(scope, book);
            let source = MergedRange { start: (0, 0), end: (0, 1) };
            let extent = MergedRange { start: (0, 0), end: (2, 1) };
            assert!(ed.drag_fill(source, extent));
            ed.book.with(|book| {
                assert_eq!(book.undo.len(), 2, "one drop creates exactly one checkpoint");
                assert!(book.redo.is_empty());
                assert_eq!(book.snapshot.cells[&(0, 1, 0)], "7");
                assert_eq!(book.snapshot.cells[&(0, 2, 1)], "=A3+$D$1");
                assert_eq!(book.snapshot.styles[&(0, 2, 0)], source_style);
                assert_eq!(book.snapshot.styles[&(0, 2, 1)], formula_style);
                assert_eq!(book.snapshot.cells[&(0, 0, 0)], "7");
                assert_eq!(book.snapshot.cells[&(0, 0, 1)], "=A1+$D$1");
                assert_eq!(book.snapshot.cells[&(0, 0, 3)], "100");
                assert!(book.dirty);
            });
            ed.history(false);
            ed.book.with(|book| {
                assert!(book.snapshot == original_snapshot, "undo restores all values and styles");
                assert_eq!(book.content, original_content);
                assert_eq!(book.undo.len(), 1);
                assert_eq!(book.redo.len(), 1);
            });
            view().build(scope)
        })).expect("fill transaction runs in an active editor Owner");
        app.clear().expect("dispose test editor");
    }

    #[test]
    fn drag_fill_validation_rejects_the_entire_batch_before_checkpointing() {
        let app = App::memory();
        app.mount(widget(move |scope| {
            let mut book = Book {
                content: serde_json::json!({"sheets":[{"id":"validated-sheet","name":"Sheet1","cells":[],"data_validations":[{
                "range_start":{"row":2,"col":0},"range_end":{"row":2,"col":0},
                "rule":{"WholeNumberBetween":[0,10]},"error_message":"Numbers 0 through 10 only"
            }]}]}),
                ..Default::default()
            };
            book.snapshot.cells.extend([
                ((0, 0, 0), "text seed".into()),
                ((0, 1, 0), "untouched first target".into()),
                ((0, 2, 0), "5".into()),
            ]);
            book.snapshot.styles.insert((0, 0, 0), CellStyle { bold: true, ..Default::default() });
            book.snapshot.styles.insert((0, 1, 0), CellStyle { italic: true, ..Default::default() });
            let original_snapshot = book.snapshot.clone();
            let original_content = book.content.clone();
            let revision = book.revision;
            let ed = fill_test_editor(scope, book);
            assert!(!ed.drag_fill(
                MergedRange { start: (0, 0), end: (0, 0) },
                MergedRange { start: (0, 0), end: (3, 0) },
            ));
            ed.book.with(|book| {
                assert!(book.snapshot == original_snapshot, "no earlier target or source is partially overwritten");
                assert_eq!(book.content, original_content);
                assert!(book.undo.is_empty());
                assert!(book.redo.is_empty());
                assert_eq!(book.revision, revision);
                assert!(!book.dirty);
            });
            assert_eq!(ed.status.get(), "Numbers 0 through 10 only");
            view().build(scope)
        })).expect("validation runs in an active editor Owner");
        app.clear().expect("dispose test editor");
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    fn book() -> Book {
        let mut workbook = syntra_doc_engine::Workbook::new("Review".into());
        let sheet = &mut workbook.sheets[0];
        sheet.set_cell(
            0,
            0,
            syntra_doc_engine::calc::CellValue::Text("00123".into()),
        );
        sheet.set_cell(0, 1, syntra_doc_engine::calc::CellValue::Boolean(true));
        sheet.set_cell(
            0,
            2,
            syntra_doc_engine::calc::CellValue::Date("2026-11-09".into()),
        );
        sheet.set_cell(
            0,
            3,
            syntra_doc_engine::calc::CellValue::Error("#N/A".into()),
        );
        let mut book = Book::default();
        book.hydrate(serde_json::to_value(workbook).unwrap());
        book
    }
    #[test]
    fn unrelated_edit_preserves_native_types_and_formula_cache() {
        let mut book = book();
        let before = workbook_cells(&book.content);
        book.checkpoint();
        book.snapshot.cells.insert((0, 1, 0), "7".into());
        book.snapshot.cells.insert((0, 1, 1), "=A2+1".into());
        let content = book.serialize();
        let after = workbook_cells(&content);
        for col in 0..4 {
            assert_eq!(before[&(0, 0, col)], after[&(0, 0, col)]);
        }
        assert_eq!(
            after[&(0, 1, 1)]["value"],
            serde_json::json!({"type":"Number","value":8.0})
        );
        assert_eq!(book.display(0, 1, 1), "8");
        book.content["sheets"][0]["cells"][0][1]["format"]["wrap_text"] = serde_json::json!(true);
        book.snapshot.styles.insert(
            (0, 0, 0),
            CellStyle {
                bold: true,
                ..Default::default()
            },
        );
        let styled = book.serialize();
        let styled_cells = workbook_cells(&styled);
        assert_eq!(styled_cells[&(0, 0, 0)]["format"]["wrap_text"], true);
        assert_eq!(styled_cells[&(0, 0, 0)]["format"]["bold"], true);
        book.checkpoint();
        book.snapshot.cells.insert((0, 1, 0), "9".into());
        assert_eq!(book.display(0, 1, 1), "10");
    }
    #[test]
    fn remote_other_cell_preserves_local_dirty_and_undo() {
        let mut book = book();
        let baseline = book.content.clone();
        book.checkpoint();
        book.snapshot.cells.insert((0, 1, 0), "local".into());
        let patch = serde_json::json!({"user_id":"other","op_type":{"type":"SetProperty","path":["workbook","sheets","0","cells"],"key":"2:0","value":{"formula":null,"value":{"type":"Text","value":"remote"}},"old_value":null}});
        book.apply_remote(&patch);
        assert!(book.dirty);
        assert_eq!(book.undo.len(), 1);
        assert_eq!(book.snapshot.cells[&(0, 1, 0)], "local");
        assert_eq!(book.snapshot.cells[&(0, 2, 0)], "remote");
        assert_ne!(book.content, baseline);
    }
    #[test]
    fn remote_replace_and_same_cell_do_not_destroy_pending_edits() {
        let mut book = book();
        book.checkpoint();
        book.snapshot.cells.insert((0, 0, 0), "local".into());
        let before = book.serialize();
        let remote = serde_json::json!({"user_id":"other","op_type":{"type":"Replace","path":["workbook"],"new_value":book.content}});
        book.apply_remote(&remote);
        assert!(book.conflict.is_some());
        assert!(book.dirty);
        assert_eq!(book.serialize(), before);
    }
}

#[cfg(test)]
mod snapshot_merge_regressions {
    use super::*;
    fn book() -> Book {
        let workbook = syntra_doc_engine::Workbook::new("Merge".into());
        let mut book = Book::default();
        book.hydrate(serde_json::to_value(workbook).unwrap());
        book
    }
    fn edit(book: &mut Book, row: u32, col: u32, value: &str) {
        book.checkpoint();
        book.snapshot.cells.insert((0, row, col), value.into());
    }
    fn peer(base: &serde_json::Value, row: u32, col: u32, value: &str) -> serde_json::Value {
        let mut book = Book::default();
        book.hydrate(base.clone());
        edit(&mut book, row, col, value);
        book.serialize()
    }
    #[test]
    fn disjoint_snapshots_keep_pending_cells_and_rebase_undo_redo() {
        let mut book = book();
        let base = book.content.clone();
        edit(&mut book, 0, 0, "local");
        book.redo = book.undo.clone();
        let remote = peer(&base, 0, 1, "remote");
        assert!(book.apply_snapshot(remote.clone()));
        assert!(book.dirty);
        assert!(book.conflict.is_none());
        assert_eq!(book.baseline, remote);
        assert_eq!(book.display(0, 0, 0), "local");
        assert_eq!(book.display(0, 0, 1), "remote");
        assert_eq!(book.undo.len(), 1);
        assert_eq!(book.redo.len(), 1);
        for (snapshot, content) in book.undo.iter().chain(&book.redo) {
            let checkpoint = Book {
                snapshot: snapshot.clone(),
                content: content.clone(),
                ..Default::default()
            };
            assert_eq!(
                checkpoint.display(0, 0, 1),
                "remote",
                "Undo/Redo cannot remove a collaborator's change"
            );
        }
        let (snapshot, content) = book.undo.pop().unwrap();
        book.snapshot = snapshot;
        book.content = content;
        book.changed();
        assert_eq!(book.display(0, 0, 0), "");
        assert_eq!(book.display(0, 0, 1), "remote");
        assert!(book.apply_snapshot(book.baseline.clone()));
        assert!(!book.dirty, "undoing back to server baseline is clean");
    }
    #[test]
    fn overlapping_snapshot_preserves_local_value_history_and_baseline() {
        let mut book = book();
        let base = book.content.clone();
        edit(&mut book, 0, 0, "local");
        let remote = peer(&base, 0, 0, "different remote");
        assert!(!book.apply_snapshot(remote.clone()));
        assert_eq!(book.display(0, 0, 0), "local");
        assert_eq!(book.baseline, base);
        assert_eq!(book.conflict, Some(remote));
        assert!(book.dirty);
        assert_eq!(book.undo.len(), 1);
    }
    #[test]
    fn derived_formula_cache_does_not_create_a_false_cell_conflict() {
        let mut engine = syntra_doc_engine::Workbook::new("Formula".into());
        engine.sheets[0].set_cell(0, 0, syntra_doc_engine::calc::CellValue::Number(1.0));
        engine.sheets[0].set_formula(0, 1, "=A1+1".into());
        engine.sheets[0].recalculate();
        let base = serde_json::to_value(engine).unwrap();
        let mut book = Book::default();
        book.hydrate(base.clone());
        edit(&mut book, 0, 2, "local");
        let remote = peer(&base, 0, 0, "7");
        assert!(book.apply_snapshot(remote));
        assert!(book.dirty);
        assert_eq!(book.display(0, 0, 1), "8");
        assert_eq!(book.display(0, 0, 2), "local");
        assert_eq!(
            workbook_cells(&book.serialize())[&(0, 0, 1)]["value"],
            serde_json::json!({"type":"Number","value":8.0})
        );
    }
    #[test]
    fn successive_snapshots_advance_common_baseline_without_losing_local_edits() {
        let mut book = book();
        let base = book.content.clone();
        edit(&mut book, 0, 0, "local");
        let first = peer(&base, 0, 1, "first remote");
        assert!(book.apply_snapshot(first.clone()));
        let second = peer(&first, 0, 2, "second remote");
        assert!(book.apply_snapshot(second.clone()));
        assert_eq!(book.baseline, second);
        assert!(book.dirty);
        assert!(book.conflict.is_none());
        assert_eq!(book.display(0, 0, 0), "local");
        assert_eq!(book.display(0, 0, 1), "first remote");
        assert_eq!(book.display(0, 0, 2), "second remote");
    }
    #[test]
    fn structural_or_metadata_changes_conflict_with_concurrent_cell_edits() {
        let mut book = book();
        let base = book.content.clone();
        edit(&mut book, 0, 0, "local");
        let mut remote = peer(&base, 0, 1, "shifted");
        remote["collaboration_structure"] = serde_json::json!("row inserted");
        assert!(!book.apply_snapshot(remote));
        assert_eq!(book.display(0, 0, 0), "local");
        let mut book = Book::default();
        book.hydrate(base.clone());
        book.checkpoint();
        book.content["sheets"][0]["column_widths"]["0"] = serde_json::json!(180);
        assert!(
            !book.apply_snapshot(peer(&base, 0, 1, "remote")),
            "metadata editing must not change the common baseline"
        );
        assert_eq!(book.content["sheets"][0]["column_widths"]["0"], 180);
    }
    #[test]
    fn own_commit_keeps_newer_typing_and_sets_the_next_merge_baseline() {
        let mut book = book();
        edit(&mut book, 0, 0, "submitted");
        let revision = book.revision;
        let submitted = book.serialize();
        edit(&mut book, 0, 0, "typed while saving");
        book.observe_commit(submitted.clone(), Some(revision));
        assert!(book.dirty);
        assert_eq!(book.display(0, 0, 0), "typed while saving");
        assert_eq!(book.baseline, submitted);
        assert!(book.apply_snapshot(peer(&submitted, 0, 1, "remote")));
        assert_eq!(book.display(0, 0, 0), "typed while saving");
        assert_eq!(book.display(0, 0, 1), "remote");
    }
}

#[cfg(test)]
mod new_cell_format_regressions {
    use super::*;
    fn empty_book() -> Book {
        let mut book = Book::default();
        book.hydrate(
            serde_json::to_value(syntra_doc_engine::Workbook::new("New styled cells".into()))
                .unwrap(),
        );
        book
    }
    fn strict_cell(content: &serde_json::Value) -> syntra_doc_engine::calc::Cell {
        let book: syntra_doc_engine::Workbook = serde_json::from_value(content.clone())
            .expect("formatted new cells must deserialize as a complete native workbook");
        book.sheets[0]
            .get_cell(0, 0)
            .expect("formatted input must not be skipped")
            .clone()
    }
    #[test]
    fn bold_on_a_new_number_keeps_typed_display_and_round_trip() {
        let mut book = empty_book();
        book.checkpoint();
        book.snapshot.cells.insert((0, 0, 0), "5".into());
        book.checkpoint();
        book.snapshot.styles.insert(
            (0, 0, 0),
            CellStyle {
                bold: true,
                ..Default::default()
            },
        );
        let raw = book.serialize_raw();
        let cell = strict_cell(&raw);
        assert!(matches!(
            cell.value,
            syntra_doc_engine::calc::CellValue::Number(5.0)
        ));
        assert!(cell.format.bold);
        assert_eq!(book.display(0, 0, 0), "5");
        book.hydrate(book.serialize());
        assert_eq!(book.display(0, 0, 0), "5");
        assert!(strict_cell(&book.content).format.bold);
    }
    #[test]
    fn format_before_inline_input_keeps_text_and_native_font_fields() {
        let mut book = empty_book();
        book.checkpoint();
        book.snapshot.styles.insert(
            (0, 0, 0),
            CellStyle {
                font_family: "Arial".into(),
                font_size: "18".into(),
                underline: true,
                ..Default::default()
            },
        );
        let empty = book.serialize_raw();
        let cell = strict_cell(&empty);
        assert!(matches!(
            cell.value,
            syntra_doc_engine::calc::CellValue::Empty
        ));
        book.checkpoint();
        book.snapshot
            .cells
            .insert((0, 0, 0), "Saved via shortcut".into());
        assert_eq!(book.display(0, 0, 0), "Saved via shortcut");
        let persisted = book.serialize();
        let cell = strict_cell(&persisted);
        assert!(
            matches!(cell.value,syntra_doc_engine::calc::CellValue::Text(ref text) if text=="Saved via shortcut")
        );
        assert_eq!(cell.format.font_family.as_deref(), Some("Arial"));
        assert_eq!(cell.format.font_size, Some(18.0));
        assert!(cell.format.underline);
        book.hydrate(persisted);
        assert_eq!(book.display(0, 0, 0), "Saved via shortcut");
    }
    #[test]
    fn delta_does_not_insert_null_for_unchanged_native_fields() {
        let mut format = serde_json::json!({});
        let before = style_to_value(&CellStyle::default());
        let after = style_to_value(&CellStyle {
            bold: true,
            ..Default::default()
        });
        merge_visible_delta(&mut format, &before, &after);
        assert_eq!(format, serde_json::json!({"bold":true}));
        let parsed: syntra_doc_engine::calc::CellFormat = serde_json::from_value(format).unwrap();
        assert!(parsed.bold);
        assert!(!parsed.italic);
    }
    #[test]
    fn legacy_malformed_visual_format_does_not_hide_values_or_break_formulas() {
        let mut workbook = syntra_doc_engine::Workbook::new("Legacy".into());
        workbook.sheets[0].set_cell(0, 0, syntra_doc_engine::calc::CellValue::Number(5.0));
        workbook.sheets[0].set_formula(0, 1, "=A1+2".into());
        let mut native = serde_json::to_value(workbook).unwrap();
        let legacy_format = serde_json::json!({"bold":null,"horizontal_align":"legacy-unknown"});
        for pair in native["sheets"][0]["cells"].as_array_mut().unwrap() {
            pair[1]["format"] = legacy_format.clone();
        }
        let mut book = Book::default();
        book.hydrate(native);
        assert_eq!(book.display(0, 0, 0), "5");
        assert_eq!(book.display(0, 0, 1), "7");
        book.checkpoint();
        book.snapshot.cells.insert((0, 0, 0), "9".into());
        assert_eq!(book.display(0, 0, 1), "11");
        let persisted = workbook_cells(&book.serialize());
        for col in 0..2 {
            assert_eq!(
                persisted[&(0, 0, col)]["format"],
                legacy_format,
                "calculation must not overwrite the original format"
            );
        }
        assert_eq!(
            persisted[&(0, 0, 1)]["value"],
            serde_json::json!({"type":"Number","value":11.0})
        );
    }
    #[test]
    fn malformed_typed_value_is_a_visible_error_instead_of_an_empty_cell() {
        let mut workbook = syntra_doc_engine::Workbook::new("Invalid value".into());
        workbook.sheets[0].set_cell(0, 0, syntra_doc_engine::calc::CellValue::Number(5.0));
        let mut native = serde_json::to_value(workbook).unwrap();
        native["sheets"][0]["cells"][0][1]["value"] =
            serde_json::json!({"type":"Number","value":"invalid"});
        let mut book = Book::default();
        book.hydrate(native);
        assert!(book.display(0, 0, 0).contains("#VALUE!"));
    }
}
