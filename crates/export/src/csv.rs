//! CSV export and import for Calc spreadsheets.
//!
//! Converts between the doc-engine [`Sheet`] model and RFC 4180-style CSV text.

use syntra_doc_engine::calc::{Cell, CellValue, Sheet};

/// Export a [`Sheet`] to a CSV string.
///
/// The output always uses CRLF line endings and quotes any field that
/// contains a comma, double-quote, or newline character.
pub fn sheet_to_csv(sheet: &Sheet) -> String {
    // Determine the bounding rectangle of the sheet.
    let (max_row, max_col) = bounding_rect(sheet);

    let mut csv = String::with_capacity(1024);

    for row in 0..=max_row {
        for col in 0..=max_col {
            if col > 0 {
                csv.push(',');
            }
            let text = sheet
                .get_cell(row, col)
                .map(cell_display_value)
                .unwrap_or_default();
            csv.push_str(&csv_escape(&text));
        }
        csv.push_str("\r\n");
    }

    csv
}

/// Import a CSV string into a new [`Sheet`].
///
/// Each line in `csv_data` becomes a row. Fields are split on commas,
/// respecting double-quoted values. The resulting sheet uses the supplied
/// `sheet_name`.
pub fn csv_to_sheet(csv_data: &str, sheet_name: &str) -> Sheet {
    let mut sheet = Sheet::new(sheet_name.to_owned());

    for (row_idx, line) in csv_lines(csv_data).iter().enumerate() {
        for (col_idx, field) in line.iter().enumerate() {
            let value = parse_cell_value(field);
            // Skip empty values to keep the HashMap sparse.
            if matches!(value, CellValue::Empty) {
                continue;
            }
            sheet.set_cell(row_idx as u32, col_idx as u32, value);
        }
    }

    sheet
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return `(max_row, max_col)` based on the keys in `sheet.cells`.
/// Returns `(0, 0)` for an empty sheet so we produce at least one empty row.
fn bounding_rect(sheet: &Sheet) -> (u32, u32) {
    if sheet.cells.is_empty() {
        return (0, 0);
    }
    let max_row = sheet.cells.keys().map(|r| r.row).max().unwrap_or(0);
    let max_col = sheet.cells.keys().map(|r| r.col).max().unwrap_or(0);
    (max_row, max_col)
}

/// Render a cell's display value as a plain string.
fn cell_display_value(cell: &Cell) -> String {
    match &cell.value {
        CellValue::Empty => String::new(),
        CellValue::Text(s) => s.clone(),
        CellValue::Number(n) => {
            // Emit integers without a decimal point.
            if *n == (*n as i64) as f64 {
                format!("{}", *n as i64)
            } else {
                format!("{n}")
            }
        }
        CellValue::Boolean(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        CellValue::Date(d) => d.clone(),
        CellValue::Error(e) => e.clone(),
    }
}

/// Escape a field value for CSV.  If the field contains a comma,
/// double-quote, newline, or carriage return it is wrapped in double-quotes
/// and any embedded double-quotes are doubled.
fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        field.to_string()
    }
}

/// Parse CSV text into a `Vec<Vec<String>>` (rows of fields).
///
/// Handles double-quoted fields (including embedded commas, newlines, and
/// escaped double-quotes).
fn csv_lines(data: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = data.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                // Check for escaped quote ("").
                if chars.peek() == Some(&'"') {
                    chars.next(); // consume the second quote
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else {
            match ch {
                '"' => {
                    in_quotes = true;
                }
                ',' => {
                    current_row.push(std::mem::take(&mut field));
                }
                '\r' => {
                    // Treat \r\n and bare \r as row terminators.
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    current_row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut current_row));
                }
                '\n' => {
                    current_row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut current_row));
                }
                _ => {
                    field.push(ch);
                }
            }
        }
    }

    // Flush the last field/row if the input does not end with a newline.
    if !field.is_empty() || !current_row.is_empty() {
        current_row.push(field);
        rows.push(current_row);
    }

    rows
}

/// Attempt to parse a CSV field into a strongly typed [`CellValue`].
fn parse_cell_value(field: &str) -> CellValue {
    let trimmed = field.trim();

    if trimmed.is_empty() {
        return CellValue::Empty;
    }

    // Boolean.
    match trimmed.to_ascii_uppercase().as_str() {
        "TRUE" => return CellValue::Boolean(true),
        "FALSE" => return CellValue::Boolean(false),
        _ => {}
    }

    // Number.
    if let Ok(n) = trimmed.parse::<f64>() {
        return CellValue::Number(n);
    }

    // Fallback: text.
    CellValue::Text(field.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple() {
        let csv_input = "Name,Age,Active\r\nAlice,30,TRUE\r\nBob,25,FALSE\r\n";
        let sheet = csv_to_sheet(csv_input, "People");
        assert_eq!(sheet.name, "People");

        // Verify some cells.
        let alice = sheet.get_cell(1, 0).expect("Alice");
        assert!(matches!(&alice.value, CellValue::Text(s) if s == "Alice"));

        let age = sheet.get_cell(1, 1).expect("30");
        assert!(matches!(&age.value, CellValue::Number(n) if (*n - 30.0).abs() < f64::EPSILON));

        let active = sheet.get_cell(1, 2).expect("TRUE");
        assert!(matches!(&active.value, CellValue::Boolean(true)));

        // Export back to CSV.
        let csv_output = sheet_to_csv(&sheet);
        assert!(csv_output.contains("Alice"));
        assert!(csv_output.contains("30"));
        assert!(csv_output.contains("TRUE"));
    }

    #[test]
    fn csv_escape_commas_and_quotes() {
        assert_eq!(csv_escape("hello"), "hello");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn parse_cell_values() {
        assert!(matches!(parse_cell_value(""), CellValue::Empty));
        assert!(
            matches!(parse_cell_value("42"), CellValue::Number(n) if (n - 42.0).abs() < f64::EPSILON)
        );
        assert!(matches!(parse_cell_value("true"), CellValue::Boolean(true)));
        assert!(matches!(parse_cell_value("hello"), CellValue::Text(_)));
    }

    #[test]
    fn empty_sheet_produces_single_empty_row() {
        let sheet = Sheet::new("Empty".to_owned());
        let csv = sheet_to_csv(&sheet);
        assert_eq!(csv, "\r\n");
    }

    #[test]
    fn quoted_fields_with_newlines() {
        let input = "a,\"b\nc\",d\n";
        let rows = csv_lines(input);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec!["a", "b\nc", "d"]);
    }
}
