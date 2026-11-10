//! Three-way merge of persisted workbook snapshots. Cell coordinates are stable
//! only while both sides retain the baseline structure and metadata.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
type CellKey = (usize, u32, u32);
type Cells = BTreeMap<CellKey, Value>;

fn state(document: &Value) -> Option<(Value, Cells)> {
    let mut metadata = document.clone();
    let sheets = metadata.get_mut("sheets")?.as_array_mut()?;
    let mut cells = Cells::new();
    let mut ids = BTreeSet::new();
    for (si, sheet) in sheets.iter_mut().enumerate() {
        if !ids.insert(sheet.get("id")?.as_str()?.to_owned()) {
            return None;
        }
        for pair in sheet.get("cells")?.as_array()? {
            let pair = pair.as_array()?;
            if pair.len() != 2 || !pair[1].is_object() {
                return None;
            }
            let row = u32::try_from(pair[0].get("row")?.as_u64()?).ok()?;
            let col = u32::try_from(pair[0].get("col")?.as_u64()?).ok()?;
            if cells.insert((si, row, col), pair[1].clone()).is_some() {
                return None;
            }
        }
        sheet["cells"] = json!([]);
    }
    Some((metadata, cells))
}
pub(super) fn semantic(cell: Option<&Value>) -> Option<Value> {
    let mut value = cell?.clone();
    if let Some(formula) = value.get("formula").and_then(Value::as_str) {
        let formula = formula.trim_start_matches('=').to_owned();
        value.as_object_mut()?.remove("value");
        value["formula"] = json!(formula);
    }
    Some(value)
}
fn equal(left: &Cells, right: &Cells) -> bool {
    left.keys()
        .chain(right.keys())
        .all(|key| semantic(left.get(key)) == semantic(right.get(key)))
}
fn write_cells(mut document: Value, cells: Cells) -> Option<Value> {
    for sheet in document.get_mut("sheets")?.as_array_mut()? {
        sheet["cells"] = json!([]);
    }
    for ((si, row, col), value) in cells {
        document
            .get_mut("sheets")?
            .as_array_mut()?
            .get_mut(si)?
            .get_mut("cells")?
            .as_array_mut()?
            .push(json!([{"row":row,"col":col},value]));
    }
    Some(document)
}

pub(super) struct Merge {
    pub content: Value,
    pub pending: bool,
    base_metadata: Value,
    remote_changes: Vec<(CellKey, Option<Value>)>,
}
impl Merge {
    /// Historical local edits must never undo a newly received remote cell.
    pub fn rebase_history(&self, document: &Value) -> Option<Value> {
        if self.remote_changes.is_empty() {
            return Some(document.clone());
        }
        let (metadata, mut cells) = state(document)?;
        if metadata != self.base_metadata {
            return None;
        }
        for (key, value) in &self.remote_changes {
            if let Some(value) = value {
                cells.insert(*key, value.clone());
            } else {
                cells.remove(key);
            }
        }
        write_cells(document.clone(), cells)
    }
}

pub(super) fn merge(base: &Value, local: &Value, remote: &Value) -> Option<Merge> {
    let (base_metadata, base_cells) = state(base)?;
    let (local_metadata, local_cells) = state(local)?;
    let (remote_metadata, remote_cells) = state(remote)?;
    let local_changed = local_metadata != base_metadata || !equal(&base_cells, &local_cells);
    let remote_changed = remote_metadata != base_metadata || !equal(&base_cells, &remote_cells);
    if !local_changed {
        return Some(Merge {
            content: remote.clone(),
            pending: false,
            base_metadata,
            remote_changes: Vec::new(),
        });
    }
    if !remote_changed {
        return Some(Merge {
            content: local.clone(),
            pending: true,
            base_metadata,
            remote_changes: Vec::new(),
        });
    }
    // A row/column shift, sheet reorder, merge, chart, named range or metadata
    // edit cannot be safely interpreted using only cell coordinates.
    if base_metadata != local_metadata || base_metadata != remote_metadata {
        return None;
    }
    let keys: BTreeSet<_> = base_cells
        .keys()
        .chain(local_cells.keys())
        .chain(remote_cells.keys())
        .copied()
        .collect();
    let mut merged = remote_cells.clone();
    let mut changes = Vec::new();
    for key in keys {
        let base = semantic(base_cells.get(&key));
        let local = semantic(local_cells.get(&key));
        let remote = semantic(remote_cells.get(&key));
        let ours = local != base;
        let theirs = remote != base;
        if ours && theirs && local != remote {
            return None;
        }
        if theirs {
            changes.push((key, remote_cells.get(&key).cloned()));
        }
        if ours {
            if let Some(cell) = local_cells.get(&key) {
                merged.insert(key, cell.clone());
            } else {
                merged.remove(&key);
            }
        }
    }
    let pending = !equal(&merged, &remote_cells);
    Some(Merge {
        content: write_cells(remote.clone(), merged)?,
        pending,
        base_metadata,
        remote_changes: changes,
    })
}
