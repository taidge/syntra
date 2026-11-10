use contrix_doc_engine::{CellRef, CellValue, Sheet};
use serde::{Deserialize, Serialize};

use super::{
    CalcSnapshot, CalculatedCells, CellKey, HashMap, cell_input_text, workbook_cell_entries,
};

#[derive(Deserialize, Serialize)]
pub(super) struct CalculationCell {
    pub(super) value: CellValue,
    #[serde(default)]
    pub(super) formula: Option<String>,
}

impl CalculationCell {
    pub(super) fn edited(text: &str) -> Self {
        let (formula, value) = if text.starts_with('=') {
            (Some(text.into()), CellValue::Empty)
        } else if text.is_empty() {
            (None, CellValue::Empty)
        } else if let Some(value) = text.parse::<f64>().ok().filter(|value| value.is_finite()) {
            (None, CellValue::Number(value))
        } else {
            (None, CellValue::Text(text.into()))
        };
        Self { value, formula }
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct CalculationInput(Vec<Vec<(CellRef, CalculationCell)>>);

pub(super) type FormulaResults = Vec<(CellKey, CellValue)>;

impl CalculationInput {
    pub(super) fn from_workbook(snapshot: &CalcSnapshot, content: &serde_json::Value) -> Self {
        let originals: HashMap<_, _> = workbook_cell_entries(content).collect();
        let mut sheets: Vec<_> = (0..snapshot.sheets.len()).map(|_| Vec::new()).collect();
        let cells = snapshot
            .cells
            .iter()
            .map(|(key, text)| (key, text.as_str()));
        let styled = snapshot
            .styles
            .keys()
            .filter(|key| !snapshot.cells.contains_key(*key))
            .map(|key| (key, ""));
        for (&(si, row, col), text) in cells.chain(styled) {
            let Some(sheet) = sheets.get_mut(si) else {
                continue;
            };
            let mut cell = match originals.get(&(si, row, col)).copied() {
                Some(original) if cell_input_text(original) == text => {
                    CalculationCell::deserialize(original).unwrap_or_else(|_| CalculationCell {
                        value: CellValue::Error(
                            "#VALUE! Invalid stored cell value or formula".into(),
                        ),
                        formula: None,
                    })
                }
                _ => CalculationCell::edited(text),
            };
            if cell.formula.is_some() {
                cell.value = CellValue::Empty;
            }
            sheet.push((CellRef::new(row, col), cell));
        }
        Self(sheets)
    }

    pub(super) fn constants(&self) -> CalculatedCells {
        self.0
            .iter()
            .enumerate()
            .flat_map(|(si, cells)| {
                cells
                    .iter()
                    .filter(|(_, cell)| cell.formula.is_none())
                    .map(move |(point, cell)| ((si, point.row, point.col), cell.value.clone()))
            })
            .collect()
    }

    pub(super) fn calculate(&self, mut remaining: usize) -> Option<FormulaResults> {
        let mut values = Vec::new();
        for (si, cells) in self.0.iter().enumerate() {
            if cells.iter().all(|(_, cell)| cell.formula.is_none()) {
                continue;
            }
            let mut sheet = Sheet::new(String::new());
            sheet.cells.extend(cells.iter().map(|(point, cell)| {
                (
                    point.clone(),
                    contrix_doc_engine::calc::Cell {
                        value: cell.value.clone(),
                        formula: cell.formula.clone(),
                        format: Default::default(),
                        comment: None,
                    },
                )
            }));
            sheet.recalculate();
            for (point, cell) in sheet.cells {
                if cell.formula.is_some() {
                    remaining = remaining.checked_sub(
                        value_bytes(&cell.value)
                            .saturating_add(std::mem::size_of::<(CellKey, CellValue)>()),
                    )?;
                    values.push(((si, point.row, point.col), cell.value));
                }
            }
        }
        Some(values)
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(super) fn needs_worker(&self) -> bool {
        let cells = self.0.iter().flatten();
        if !cells.clone().any(|(_, cell)| cell.formula.is_some()) {
            return false;
        }
        let mut remaining = 4_096usize;
        for (_, cell) in cells {
            let Some(formula_cost) = cell.formula.as_deref().map_or(Some(0), inline_formula_cost)
            else {
                return true;
            };
            let cost = 16usize
                .saturating_add(value_bytes(&cell.value))
                .saturating_add(formula_cost);
            let Some(next) = remaining.checked_sub(cost) else {
                return true;
            };
            remaining = next;
        }
        false
    }
}

fn value_bytes(value: &CellValue) -> usize {
    match value {
        CellValue::Text(text) | CellValue::Date(text) | CellValue::Error(text) => text.len(),
        _ => 0,
    }
}

/// Admit only small work inline. Each range may be walked in the graph pass
/// and again during evaluation, even if it has no populated cells.
#[cfg(any(test, target_arch = "wasm32"))]
fn inline_formula_cost(formula: &str) -> Option<usize> {
    if formula.len() > 4_096
        || formula.contains('&')
        || formula.to_ascii_uppercase().contains("CONCATENATE")
    {
        return None;
    }
    let mut cost = formula.len();
    let is_delimiter = |c: char| !c.is_ascii_alphanumeric() && c != '$';
    for (offset, _) in formula.match_indices(':') {
        let left = formula[..offset].trim_end().rsplit(is_delimiter).next()?;
        let right = formula[offset + 1..]
            .trim_start()
            .split(is_delimiter)
            .next()?;
        let start = CellRef::from_a1(&left.replace('$', ""))?;
        let end = CellRef::from_a1(&right.replace('$', ""))?;
        let rows = u64::from(start.row.abs_diff(end.row)) + 1;
        let columns = u64::from(start.col.abs_diff(end.col)) + 1;
        let visits = usize::try_from(rows.checked_mul(columns)?.checked_mul(4)?).ok()?;
        cost = cost.checked_add(visits)?;
    }
    Some(cost)
}

#[cfg(target_arch = "wasm32")]
pub(super) const MAX_WIRE_BYTES: usize =
    contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES + 4 * 1024 * 1024;

#[cfg(target_arch = "wasm32")]
pub(super) fn encode(value: &impl Serialize) -> serde_json::Result<Vec<u8>> {
    // Cap the serialized payload while writing, including escaped strings.
    struct BoundedBuffer(Vec<u8>);
    impl std::io::Write for BoundedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_WIRE_BYTES - self.0.len() {
                return Err(std::io::Error::other(
                    "calculation message exceeds its size limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut output = BoundedBuffer(Vec::new());
    serde_json::to_writer(&mut output, value)?;
    Ok(output.0)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    (bytes.len() <= MAX_WIRE_BYTES)
        .then(|| serde_json::from_slice(bytes).ok())
        .flatten()
}

/// The module worker imports this entry point without starting the Glory app.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn calculate_workbook_formulas(bytes: &[u8]) -> Result<Vec<u8>, wasm_bindgen::JsValue> {
    let input: CalculationInput = decode(bytes).ok_or("Invalid formula calculation input")?;
    let results = input
        .calculate(MAX_WIRE_BYTES)
        .ok_or("Formula calculation output is too large")?;
    encode(&results).map_err(|_| "Formula calculation output is too large".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_admission_accounts_for_sparse_ranges_and_expanding_text() {
        let input = |formula: &str| {
            CalculationInput(vec![vec![(
                CellRef::new(0, 0),
                CalculationCell {
                    value: CellValue::Empty,
                    formula: Some(formula.to_owned()),
                },
            )]])
        };
        assert!(!input("=SUM($B$1:$B$10)+2").needs_worker());
        assert!(input("=SUM(B1:B50000)").needs_worker());
        assert!(input("=SUM(B1:B600)+SUM(C1:C600)").needs_worker());
        assert!(input("=B1&B1").needs_worker());
        assert!(input("=concatenate(B1,B1)").needs_worker());
    }
}
