//! Standard cache/table parts for the single-row-field pivot model.
use super::{XlsxExportStats, escape_xml as escape};
use contrix_doc_engine::calc::*;
use std::collections::{BTreeMap, BTreeSet};
const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub(super) struct PivotParts {
    pub index: usize,
    pub target_sheet: usize,
    pub definition: String,
    pub cache: String,
    pub records: String,
}
fn value_xml(value: &CellValue) -> String {
    match value {
        CellValue::Empty => "<m/>".into(),
        CellValue::Number(n) if n.is_finite() => format!("<n v=\"{n}\"/>"),
        CellValue::Boolean(b) => format!("<b v=\"{}\"/>", u8::from(*b)),
        CellValue::Date(d) => format!(
            "<d v=\"{}\"/>",
            escape(&if d.len() == 10 {
                format!("{d}T00:00:00")
            } else {
                d.clone()
            })
        ),
        CellValue::Error(e) => format!("<e v=\"{}\"/>", escape(e)),
        CellValue::Text(text) => format!("<s v=\"{}\"/>", escape(text)),
        _ => "<m/>".into(),
    }
}
pub(super) fn build(
    workbook: &Workbook,
    stats: &mut XlsxExportStats,
) -> Result<Vec<PivotParts>, String> {
    let mut result = Vec::new();
    let mut remaining = contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES * 2;
    for (index, pivot) in workbook.pivot_tables.iter().enumerate() {
        let source = workbook
            .sheets
            .iter()
            .find(|s| s.id == pivot.source_sheet_id)
            .ok_or("Pivot source sheet is missing")?;
        let target_sheet = workbook
            .sheets
            .iter()
            .position(|s| s.id == pivot.target_sheet_id)
            .ok_or("Pivot target sheet is missing")?;
        // SpreadsheetML source rectangles include a header and pivot locations
        // include their heading row. Do not silently consume a data row when
        // a native-only specification provides no room for those headings.
        if pivot.source_start.row == 0 || pivot.target.row == 0 {
            return Err("Standard Office pivot export requires a source header row and target heading row before the data; adjust the pivot source/target rows".into());
        }
        let start = &pivot.source_start;
        let end = &pivot.source_end;
        if start.row > end.row
            || start.col > end.col
            || end.row >= 1_048_576
            || end.col >= 16_384
            || pivot.row_field_col < start.col
            || pivot.row_field_col > end.col
            || pivot.value_field_col < start.col
            || pivot.value_field_col > end.col
        {
            return Err("Invalid pivot source rectangle or fields".into());
        }
        let width = end.col - start.col + 1;
        let height = end.row - start.row + 1;
        if u64::from(width) * u64::from(height) > 1_000_000 {
            return Err("Pivot cache expansion budget exceeded".into());
        }
        let row_field = pivot.row_field_col - start.col;
        let value_field = pivot.value_field_col - start.col;
        let mut shared: Vec<Vec<String>> = (0..width).map(|_| Vec::new()).collect();
        let mut lookup: Vec<BTreeMap<String, usize>> =
            (0..width).map(|_| BTreeMap::new()).collect();
        let mut records = format!("<pivotCacheRecords xmlns=\"{NS}\" count=\"{height}\">");
        let mut row_items = Vec::new();
        let mut seen_rows = BTreeSet::new();
        for row in start.row..=end.row {
            records.push_str("<r>");
            for col in start.col..=end.col {
                stats.pivot_cell_lookups += 1;
                let cell = source
                    .get_cell(row, col)
                    .map(|c| &c.value)
                    .unwrap_or(&CellValue::Empty);
                let item = value_xml(cell);
                let field = (col - start.col) as usize;
                let key = if let Some(key) = lookup[field].get(&item) {
                    *key
                } else {
                    let key = shared[field].len();
                    lookup[field].insert(item.clone(), key);
                    shared[field].push(item);
                    key
                };
                records.push_str(&format!("<x v=\"{key}\"/>"));
                if field == row_field as usize && seen_rows.insert(key) {
                    row_items.push(key);
                }
            }
            records.push_str("</r>");
        }
        records.push_str("</pivotCacheRecords>");
        let number = index + 1;
        let header = start.row.saturating_sub(1);
        let source_ref = format!(
            "{}:{}",
            CellRef::new(header, start.col).to_a1(),
            end.to_a1()
        );
        let mut cache = format!(
            "<pivotCacheDefinition xmlns=\"{NS}\" xmlns:r=\"{REL}\" r:id=\"rIdRecords\" refreshOnLoad=\"1\" recordCount=\"{height}\"><cacheSource type=\"worksheet\"><worksheetSource ref=\"{source_ref}\" sheet=\"{}\"/></cacheSource><cacheFields count=\"{width}\">",
            escape(&source.name)
        );
        let mut names = Vec::new();
        let mut seen_names = BTreeSet::new();
        for col in start.col..=end.col {
            stats.pivot_cell_lookups += 1;
            let field = (col - start.col) as usize;
            let name = match source.get_cell(header, col).map(|c| &c.value) {
                Some(CellValue::Text(name)) if !name.is_empty() => name.clone(),
                _ => format!("Column{}", col + 1),
            };
            let mut unique = name.clone();
            let mut suffix = 2;
            while !seen_names.insert(unique.clone()) {
                unique = format!("{name}_{suffix}");
                suffix += 1;
            }
            let name = unique;
            names.push(name.clone());
            cache.push_str(&format!("<cacheField name=\"{}\" numFmtId=\"0\"><sharedItems count=\"{}\">{}</sharedItems></cacheField>",escape(&name),shared[field].len(),shared[field].join("")));
        }
        cache.push_str("</cacheFields></pivotCacheDefinition>");
        let header_row = pivot.target.row.saturating_sub(1);
        let first_data = pivot.target.row - header_row;
        let end_row = pivot
            .target
            .row
            .checked_add(row_items.len().saturating_sub(1) as u32)
            .ok_or("Pivot target overflow")?;
        let end_col = pivot
            .target
            .col
            .checked_add(1)
            .filter(|col| *col < 16_384)
            .ok_or("Pivot target column overflow")?;
        if end_row >= 1_048_576 {
            return Err("Pivot target exceeds worksheet dimensions".into());
        }
        let target_ref = format!(
            "{}:{}",
            CellRef::new(header_row, pivot.target.col).to_a1(),
            CellRef::new(end_row, end_col).to_a1()
        );
        let mut definition = format!(
            "<pivotTableDefinition xmlns=\"{NS}\" name=\"{}\" cacheId=\"{index}\" dataCaption=\"Values\" rowGrandTotals=\"0\" colGrandTotals=\"0\" compact=\"0\" compactData=\"0\" gridDropZones=\"1\"><location ref=\"{target_ref}\" firstHeaderRow=\"0\" firstDataRow=\"{first_data}\" firstDataCol=\"1\"/><pivotFields count=\"{width}\">",
            escape(&pivot.name)
        );
        for field in 0..width {
            if field == row_field {
                definition.push_str(&format!("<pivotField axis=\"axisRow\" showAll=\"0\" defaultSubtotal=\"0\"><items count=\"{}\">",row_items.len()));
                for item in &row_items {
                    definition.push_str(&format!("<item x=\"{item}\"/>"));
                }
                definition.push_str("</items></pivotField>");
            } else if field == value_field {
                definition.push_str("<pivotField dataField=\"1\" showAll=\"0\"/>");
            } else {
                definition.push_str("<pivotField showAll=\"0\"/>");
            }
        }
        let subtotal = match pivot.aggregation {
            PivotAggregation::Sum => "sum",
            PivotAggregation::Count => "count",
            PivotAggregation::Average => "average",
        };
        definition.push_str(&format!("</pivotFields><rowFields count=\"1\"><field x=\"{row_field}\"/></rowFields><rowItems count=\"{}\">",row_items.len()));
        for index in 0..row_items.len() {
            definition.push_str(&format!("<i><x v=\"{index}\"/></i>"));
        }
        definition.push_str(&format!("</rowItems><dataFields count=\"1\"><dataField name=\"{} of {}\" fld=\"{value_field}\" subtotal=\"{subtotal}\"/></dataFields><pivotTableStyleInfo name=\"PivotStyleLight16\" showRowHeaders=\"1\" showColHeaders=\"1\" showRowStripes=\"0\" showColStripes=\"0\" showLastColumn=\"1\"/></pivotTableDefinition>",subtotal,escape(&names[value_field as usize])));
        let bytes = definition
            .len()
            .saturating_add(cache.len())
            .saturating_add(records.len());
        remaining = remaining
            .checked_sub(bytes)
            .ok_or("Total pivot cache serialization budget exceeded")?;
        result.push(PivotParts {
            index: number,
            target_sheet,
            definition,
            cache,
            records,
        });
    }
    Ok(result)
}
pub(super) fn content_types(parts: &[PivotParts]) -> String {
    parts.iter().flat_map(|p|[
        format!("<Override PartName=\"/xl/pivotTables/pivotTable{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml\"/>",p.index),
        format!("<Override PartName=\"/xl/pivotCache/pivotCacheDefinition{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml\"/>",p.index),
        format!("<Override PartName=\"/xl/pivotCache/pivotCacheRecords{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheRecords+xml\"/>",p.index),
    ]).collect::<Vec<_>>().join("")
}
pub(super) fn workbook_caches(parts: &[PivotParts]) -> String {
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "<pivotCaches>{}</pivotCaches>",
        parts
            .iter()
            .map(|p| format!(
                "<pivotCache cacheId=\"{}\" r:id=\"rIdPivotCache{}\"/>",
                p.index - 1,
                p.index
            ))
            .collect::<Vec<_>>()
            .join("")
    )
}
pub(super) fn workbook_rels(parts: &[PivotParts]) -> String {
    parts.iter().map(|p|format!("<Relationship Id=\"rIdPivotCache{}\" Type=\"{REL}/pivotCacheDefinition\" Target=\"pivotCache/pivotCacheDefinition{}.xml\"/>",p.index,p.index)).collect::<Vec<_>>().join("")
}
pub(super) fn sheet_refs(parts: &[PivotParts], sheet: usize) -> String {
    let selected = parts
        .iter()
        .filter(|p| p.target_sheet == sheet)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return String::new();
    }
    format!(
        "<pivotTableParts count=\"{}\">{}</pivotTableParts>",
        selected.len(),
        selected
            .iter()
            .map(|p| format!("<pivotTablePart r:id=\"rIdPivot{}\"/>", p.index))
            .collect::<Vec<_>>()
            .join("")
    )
}
pub(super) fn sheet_rels(parts: &[PivotParts], sheet: usize) -> String {
    parts.iter().filter(|p|p.target_sheet==sheet).map(|p|format!("<Relationship Id=\"rIdPivot{}\" Type=\"{REL}/pivotTable\" Target=\"../pivotTables/pivotTable{}.xml\"/>",p.index,p.index)).collect::<Vec<_>>().join("")
}
