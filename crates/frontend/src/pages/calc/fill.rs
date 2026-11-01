//! Pure fill planning. The editor validates and checkpoints once before applying.
use super::{CalcSnapshot, CellStyle, MergedRange, NUM_COLS, NUM_ROWS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FillError {
    OutOfBounds,
    InvalidExtent,
    MergedCells,
    NonFiniteSeries,
}
impl std::fmt::Display for FillError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(match self {
            Self::OutOfBounds => "Fill range is outside this sheet",
            Self::InvalidExtent => "Extend the selected range in one direction",
            Self::MergedCells => "Unmerge cells before dragging the fill handle",
            Self::NonFiniteSeries => "Fill series exceeds the finite numeric range",
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FillChange {
    pub point: (u32, u32),
    /// None clears a destination whose corresponding seed is empty.
    pub value: Option<String>,
    /// None clears any destination formatting when the seed has no style.
    pub style: Option<CellStyle>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FillPatch {
    pub range: MergedRange,
    pub changes: Vec<FillChange>,
    sheet: usize,
}
impl FillPatch {
    /// No history or transport writes here: the caller owns the transaction.
    pub fn apply_to(&self, snapshot: &mut CalcSnapshot) {
        for change in &self.changes {
            let key = (self.sheet, change.point.0, change.point.1);
            if let Some(value) = &change.value {
                snapshot.cells.insert(key, value.clone());
            } else {
                snapshot.cells.remove(&key);
            }
            if let Some(style) = &change.style {
                snapshot.styles.insert(key, style.clone());
            } else {
                snapshot.styles.remove(&key);
            }
        }
    }
}

/// `extent` includes the original source and extends along exactly one axis.
/// Each source column/row has its own sequence; styles always tile with seeds.
pub(super) fn plan_fill(
    snapshot: &CalcSnapshot,
    sheet: usize,
    source: MergedRange,
    extent: MergedRange,
) -> Result<FillPatch, FillError> {
    let source = source.normalized();
    let extent = extent.normalized();
    if sheet >= snapshot.sheets.len()
        || extent.end.0 >= NUM_ROWS
        || extent.end.1 >= NUM_COLS
        || source.end.0 >= NUM_ROWS
        || source.end.1 >= NUM_COLS
    {
        return Err(FillError::OutOfBounds);
    }
    if !extent.contains(source.start) || !extent.contains(source.end) {
        return Err(FillError::InvalidExtent);
    }
    let vertical = extent.start.1 == source.start.1 && extent.end.1 == source.end.1;
    let horizontal = extent.start.0 == source.start.0 && extent.end.0 == source.end.0;
    if !vertical && !horizontal {
        return Err(FillError::InvalidExtent);
    }
    if snapshot
        .merged_cells
        .get(&sheet)
        .is_some_and(|ranges| ranges.iter().any(|range| range.overlaps(extent)))
    {
        return Err(FillError::MergedCells);
    }
    let mut changes = Vec::new();
    for row in extent.start.0..=extent.end.0 {
        for col in extent.start.1..=extent.end.1 {
            if source.contains((row, col)) {
                continue;
            }
            let seed_row = source.start.0
                + (i64::from(row) - i64::from(source.start.0))
                    .rem_euclid(i64::from(source.row_span())) as u32;
            let seed_col = source.start.1
                + (i64::from(col) - i64::from(source.start.1))
                    .rem_euclid(i64::from(source.col_span())) as u32;
            let seed_key = (sheet, seed_row, seed_col);
            let mut value = snapshot.cells.get(&seed_key).cloned();
            if let Some(raw) = &value
                && raw.starts_with('=')
            {
                value = Some(shift_formula(
                    raw,
                    row as i32 - seed_row as i32,
                    col as i32 - seed_col as i32,
                ));
            } else if (vertical && source.row_span() == 2) || (horizontal && source.col_span() == 2)
            {
                let (first, second, offset) = if vertical {
                    (
                        (sheet, source.start.0, col),
                        (sheet, source.start.0 + 1, col),
                        i64::from(row) - i64::from(source.start.0),
                    )
                } else {
                    (
                        (sheet, row, source.start.1),
                        (sheet, row, source.start.1 + 1),
                        i64::from(col) - i64::from(source.start.1),
                    )
                };
                let number = |key| {
                    snapshot
                        .cells
                        .get(&key)
                        .and_then(|value| value.parse::<f64>().ok())
                        .filter(|value| value.is_finite())
                };
                if let (Some(a), Some(b)) = (number(first), number(second)) {
                    let next = a + (b - a) * offset as f64;
                    if !next.is_finite() {
                        return Err(FillError::NonFiniteSeries);
                    }
                    value = Some(if next == 0.0 {
                        "0".into()
                    } else {
                        next.to_string()
                    });
                }
            }
            // A blank seed behaves as a blank, even if an imported map stores "".
            value = value.filter(|value| !value.is_empty());
            let style = snapshot.styles.get(&seed_key).cloned();
            let key = (sheet, row, col);
            if value.as_ref() != snapshot.cells.get(&key)
                || style.as_ref() != snapshot.styles.get(&key)
            {
                changes.push(FillChange {
                    point: (row, col),
                    value,
                    style,
                });
            }
        }
    }
    Ok(FillPatch {
        range: extent,
        changes,
        sheet,
    })
}

fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '.' | '\\')
}

fn column_name(mut column: i64, lowercase: bool) -> String {
    let mut name = Vec::new();
    while column > 0 {
        column -= 1;
        name.push((if lowercase { b'a' } else { b'A' }) + (column % 26) as u8);
        column /= 26;
    }
    name.into_iter().rev().map(char::from).collect()
}

fn shift_axis_range(formula: &str, start: usize, dr: i32, dc: i32) -> Option<(usize, String)> {
    let bytes = formula.as_bytes();
    let first = start + usize::from(bytes.get(start) == Some(&b'$'));
    let column = bytes.get(first)?.is_ascii_alphabetic();
    let parse = |start: usize| -> Option<(usize, bool, i64, bool)> {
        let absolute = bytes.get(start) == Some(&b'$');
        let value_start = start + usize::from(absolute);
        let mut end = value_start;
        while bytes.get(end).is_some_and(|ch| {
            if column {
                ch.is_ascii_alphabetic()
            } else {
                ch.is_ascii_digit()
            }
        }) {
            end += 1;
        }
        if end == value_start {
            return None;
        }
        let value = if column {
            bytes[value_start..end]
                .iter()
                .try_fold(0_i64, |value, ch| {
                    value
                        .checked_mul(26)?
                        .checked_add(i64::from(ch.to_ascii_uppercase() - b'A' + 1))
                })?
        } else {
            formula[value_start..end].parse().ok()?
        };
        let max = if column { 16_384 } else { 1_048_576 };
        (1..=max).contains(&value).then_some((
            end,
            absolute,
            value,
            bytes[value_start..end].iter().all(u8::is_ascii_lowercase),
        ))
    };
    let (middle, first_absolute, first, first_lowercase) = parse(start)?;
    if bytes.get(middle) != Some(&b':') {
        return None;
    }
    let (end, second_absolute, second, second_lowercase) = parse(middle + 1)?;
    if formula[end..]
        .chars()
        .next()
        .is_some_and(|ch| identifier_char(ch) || matches!(ch, '!' | '$'))
    {
        return None;
    }
    let format = |value: i64, absolute: bool, lowercase: bool| {
        let max = if column { 16_384 } else { 1_048_576 };
        let value = value
            + if absolute {
                0
            } else {
                i64::from(if column { dc } else { dr })
            };
        if !(1..=max).contains(&value) {
            return "#REF!".to_string();
        }
        format!(
            "{}{}",
            if absolute { "$" } else { "" },
            if column {
                column_name(value, lowercase)
            } else {
                value.to_string()
            }
        )
    };
    Some((
        end,
        format!(
            "{}:{}",
            format(first, first_absolute, first_lowercase),
            format(second, second_absolute, second_lowercase)
        ),
    ))
}

/// Shift A1 references without changing literals, function names or sheet names.
/// References beyond Excel's finite address space become #REF!, not wrapped IDs.
pub(super) fn shift_formula(formula: &str, dr: i32, dc: i32) -> String {
    let bytes = formula.as_bytes();
    let mut output = String::with_capacity(formula.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let first = bytes[cursor];
        if matches!(first, b'"' | b'\'') {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() {
                if bytes[cursor] == first {
                    cursor += 1;
                    if bytes.get(cursor) == Some(&first) {
                        cursor += 1;
                        continue;
                    }
                    break;
                }
                cursor += 1;
            }
            output.push_str(&formula[start..cursor]);
            continue;
        }
        if first == b'[' {
            let start = cursor;
            let mut depth = 1;
            cursor += 1;
            while cursor < bytes.len() && depth > 0 {
                match bytes[cursor] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                cursor += 1;
            }
            output.push_str(&formula[start..cursor]);
            continue;
        }
        let previous = formula[..cursor].chars().next_back();
        if previous.is_none_or(|ch| !identifier_char(ch) && ch != '$')
            && let Some((end, shifted)) = shift_axis_range(formula, cursor, dr, dc)
        {
            output.push_str(&shifted);
            cursor = end;
            continue;
        }
        if (first == b'$' || first.is_ascii_alphabetic())
            && previous.is_none_or(|ch| !identifier_char(ch) && ch != '$')
        {
            let start = cursor;
            let mut end = start;
            let absolute_col = bytes[end] == b'$';
            end += usize::from(absolute_col);
            let letters = end;
            while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                end += 1;
            }
            let letters_end = end;
            let absolute_row = bytes.get(end) == Some(&b'$');
            end += usize::from(absolute_row);
            let digits = end;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            let next = formula[end..].chars().next();
            let is_function = formula[end..].trim_start().starts_with('(');
            if letters_end > letters
                && end > digits
                && next.is_none_or(|ch| !identifier_char(ch) && !matches!(ch, '!' | '$'))
                && !is_function
            {
                let column = bytes[letters..letters_end].iter().try_fold(0_i64, |n, ch| {
                    n.checked_mul(26)?
                        .checked_add(i64::from(ch.to_ascii_uppercase() - b'A' + 1))
                });
                let row = formula[digits..end].parse::<i64>().ok();
                if let (Some(column @ 1..=16_384), Some(row @ 1..=1_048_576)) = (column, row) {
                    let new_col = column + if absolute_col { 0 } else { i64::from(dc) };
                    let new_row = row + if absolute_row { 0 } else { i64::from(dr) };
                    if !(1..=16_384).contains(&new_col) || !(1..=1_048_576).contains(&new_row) {
                        output.push_str("#REF!");
                    } else {
                        if absolute_col {
                            output.push('$');
                        }
                        let lowercase = bytes[letters..letters_end]
                            .iter()
                            .all(u8::is_ascii_lowercase);
                        output.push_str(&column_name(new_col, lowercase));
                        if absolute_row {
                            output.push('$');
                        }
                        output.push_str(&new_row.to_string());
                    }
                    cursor = end;
                    continue;
                }
            }
        }
        let ch = formula[cursor..]
            .chars()
            .next()
            .expect("character boundary");
        output.push(ch);
        cursor += ch.len_utf8();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn book() -> CalcSnapshot {
        CalcSnapshot {
            cells: HashMap::new(),
            styles: HashMap::new(),
            merged_cells: HashMap::new(),
            sheets: vec!["Sheet1".into()],
        }
    }
    fn range(start: (u32, u32), end: (u32, u32)) -> MergedRange {
        MergedRange { start, end }
    }

    #[test]
    fn repeats_single_seed_in_all_four_directions_without_touching_source() {
        let source = range((2, 2), (2, 2));
        for extent in [
            range((0, 2), (2, 2)),
            range((2, 2), (5, 2)),
            range((2, 0), (2, 2)),
            range((2, 2), (2, 5)),
        ] {
            let mut snapshot = book();
            snapshot.cells.insert((0, 2, 2), "7".into());
            let style = CellStyle {
                italic: true,
                number_format: "0.00".into(),
                ..Default::default()
            };
            snapshot.styles.insert((0, 2, 2), style.clone());
            let patch = plan_fill(&snapshot, 0, source, extent).unwrap();
            assert!(
                !patch
                    .changes
                    .iter()
                    .any(|change| source.contains(change.point))
            );
            patch.apply_to(&mut snapshot);
            for row in extent.start.0..=extent.end.0 {
                for col in extent.start.1..=extent.end.1 {
                    assert_eq!(snapshot.cells[&(0, row, col)], "7");
                    assert_eq!(snapshot.styles[&(0, row, col)], style);
                }
            }
        }
    }

    #[test]
    fn extends_numeric_series_per_column_forward_and_backward() {
        let mut snapshot = book();
        for (row, col, value) in [(2, 1, "10"), (3, 1, "13"), (2, 2, "5"), (3, 2, "3")] {
            snapshot.cells.insert((0, row, col), value.into());
        }
        let patch = plan_fill(&snapshot, 0, range((2, 1), (3, 2)), range((0, 1), (5, 2))).unwrap();
        patch.apply_to(&mut snapshot);
        assert_eq!(snapshot.cells[&(0, 0, 1)], "4");
        assert_eq!(snapshot.cells[&(0, 5, 1)], "19");
        assert_eq!(snapshot.cells[&(0, 0, 2)], "9");
        assert_eq!(snapshot.cells[&(0, 5, 2)], "-1");
    }

    #[test]
    fn extends_numeric_series_per_row_left_and_right() {
        let mut snapshot = book();
        for (row, col, value) in [(1, 2, "2"), (1, 3, "4"), (2, 2, "10"), (2, 3, "9")] {
            snapshot.cells.insert((0, row, col), value.into());
        }
        let patch = plan_fill(&snapshot, 0, range((1, 2), (2, 3)), range((1, 0), (2, 5))).unwrap();
        patch.apply_to(&mut snapshot);
        assert_eq!(snapshot.cells[&(0, 1, 0)], "-2");
        assert_eq!(snapshot.cells[&(0, 1, 5)], "8");
        assert_eq!(snapshot.cells[&(0, 2, 0)], "12");
        assert_eq!(snapshot.cells[&(0, 2, 5)], "7");
    }

    #[test]
    fn tiles_values_styles_and_empty_seeds_over_existing_destination() {
        let mut snapshot = book();
        snapshot.cells.insert((0, 2, 1), "first".into());
        snapshot.cells.insert((0, 3, 1), "second".into());
        snapshot.cells.insert((0, 4, 1), "third".into());
        snapshot.cells.insert((0, 0, 2), "stale".into());
        snapshot.styles.insert(
            (0, 0, 2),
            CellStyle {
                bold: true,
                ..Default::default()
            },
        );
        snapshot.styles.insert(
            (0, 3, 1),
            CellStyle {
                underline: true,
                ..Default::default()
            },
        );
        let patch = plan_fill(&snapshot, 0, range((2, 1), (4, 2)), range((0, 1), (6, 2))).unwrap();
        patch.apply_to(&mut snapshot);
        assert_eq!(snapshot.cells[&(0, 0, 1)], "second");
        assert_eq!(snapshot.cells[&(0, 5, 1)], "first");
        assert_eq!(snapshot.cells[&(0, 6, 1)], "second");
        assert!(snapshot.styles[&(0, 0, 1)].underline);
        assert!(!snapshot.cells.contains_key(&(0, 0, 2)));
        assert!(!snapshot.styles.contains_key(&(0, 0, 2)));
    }

    #[test]
    fn tiled_formulas_use_each_corresponding_seed_origin() {
        let mut snapshot = book();
        snapshot.cells.insert((0, 1, 1), "=A1+$A$1".into());
        snapshot.cells.insert((0, 2, 1), "=SUM(A2:B2)".into());
        let patch = plan_fill(&snapshot, 0, range((1, 1), (2, 1)), range((1, 1), (4, 1))).unwrap();
        patch.apply_to(&mut snapshot);
        assert_eq!(snapshot.cells[&(0, 3, 1)], "=A3+$A$1");
        assert_eq!(snapshot.cells[&(0, 4, 1)], "=SUM(A4:B4)");
    }

    #[test]
    fn formula_shift_preserves_absolute_axes_ranges_literals_and_sheet_names() {
        assert_eq!(
            shift_formula("=SUM(A1:B2)+$C3+D$4+$E$5+\"A1\"+\"say \"\"B2\"\"\"", 2, 1),
            "=SUM(B3:C4)+$C5+E$4+$E$5+\"A1\"+\"say \"\"B2\"\"\""
        );
        assert_eq!(
            shift_formula(
                "='A1 sheet'!B2+Sheet1!C3+LOG10(A1)+A1_name+Table1[A1]+a1",
                1,
                1
            ),
            "='A1 sheet'!C3+Sheet1!D4+LOG10(B2)+A1_name+Table1[A1]+b2"
        );
        assert_eq!(
            shift_formula("=A1+$A1+A$1+$A$1", -1, -1),
            "=#REF!+#REF!+#REF!+$A$1"
        );
        assert_eq!(shift_formula("=XFD1048576", 1, 1), "=#REF!");
    }

    #[test]
    fn formula_shift_handles_whole_axes_and_does_not_touch_identifiers_or_unicode() {
        assert_eq!(
            shift_formula("=SUM(A:C,$B:$D,1:3,$2:$4)+\"A:A\"", 2, 1),
            "=SUM(B:D,$B:$D,3:5,$2:$4)+\"A:A\""
        );
        assert_eq!(
            shift_formula("=SUM(A:A,1:1)", -1, -1),
            "=SUM(#REF!:#REF!,#REF!:#REF!)"
        );
        assert_eq!(
            shift_formula(
                "='A1''s 表'!A1+范围A1+name.A1+LOG10(A1)+[Book.xlsx]Sheet1!B2",
                1,
                1
            ),
            "='A1''s 表'!B2+范围A1+name.A1+LOG10(B2)+[Book.xlsx]Sheet1!C3"
        );
    }

    #[test]
    fn non_finite_series_has_no_partial_patch_or_source_mutation() {
        let mut snapshot = book();
        snapshot.cells.insert((0, 0, 0), "-1e308".into());
        snapshot.cells.insert((0, 1, 0), "1e308".into());
        let original = snapshot.clone();
        assert_eq!(
            plan_fill(&snapshot, 0, range((0, 0), (1, 0)), range((0, 0), (3, 0))),
            Err(FillError::NonFiniteSeries)
        );
        assert!(snapshot == original);
    }

    #[test]
    fn rejects_diagonal_out_of_bounds_and_merged_extents_without_mutating() {
        let mut snapshot = book();
        snapshot.cells.insert((0, 0, 0), "seed".into());
        let original = snapshot.clone();
        assert_eq!(
            plan_fill(&snapshot, 0, range((0, 0), (0, 0)), range((0, 0), (2, 2))),
            Err(FillError::InvalidExtent)
        );
        assert_eq!(
            plan_fill(
                &snapshot,
                0,
                range((0, 0), (0, 0)),
                range((0, 0), (NUM_ROWS, 0))
            ),
            Err(FillError::OutOfBounds)
        );
        assert!(snapshot == original);
        snapshot.merged_cells.insert(0, vec![range((1, 0), (2, 0))]);
        assert_eq!(
            plan_fill(&snapshot, 0, range((0, 0), (0, 0)), range((0, 0), (3, 0))),
            Err(FillError::MergedCells)
        );
    }

    #[test]
    fn identical_extent_is_an_empty_patch() {
        let snapshot = book();
        assert!(
            plan_fill(&snapshot, 0, range((1, 1), (2, 2)), range((1, 1), (2, 2)))
                .unwrap()
                .changes
                .is_empty()
        );
    }
}
