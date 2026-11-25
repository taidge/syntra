//! XLSX export for Calc workbooks.
//!
//! Converts a [`Workbook`] into a `.xlsx` byte vector (a ZIP archive
//! containing Open XML Spreadsheet files).  The output is compatible with
//! Microsoft Excel, LibreOffice Calc, and other spreadsheet applications that
//! support the OOXML format.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Write};

use calamine::{Data, Reader, Xlsx};
use serde::{Deserialize, Serialize};
use syntra_doc_engine::calc::{
    Cell, CellRef, CellValue, ConditionalFormat, ConditionalRule, DataValidation, MergedRange,
    NamedRange, PivotTable, Sheet, SheetChart, SheetChartType, ValidationRule, Workbook,
    WorkbookMetadata,
};
use zip::write::{SimpleFileOptions, ZipWriter};

#[path = "xlsx_import.rs"]
mod external_import;
#[path = "xlsx_pivots.rs"]
mod pivots;
#[path = "xlsx_styles.rs"]
mod styles;

/// Actual cell iteration and coordinate-lookup events in XLSX export.
///
/// A visit counts one source-map entry consumed by the named pass, or one
/// chart/pivot coordinate lookup including an absent coordinate. Repeated
/// passes count separately. Native snapshot visits count the source cells
/// cloned for budget validation and for the native payload. Generic traversal
/// of those cloned payloads by Serde, sorting, XML text processing, metadata,
/// ZIP compression and allocation are outside these counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct XlsxExportStats {
    pub native_snapshot_cell_visits: usize,
    pub coordinate_cell_visits: usize,
    pub shared_string_cell_visits: usize,
    pub style_cell_visits: usize,
    pub worksheet_grouping_cell_visits: usize,
    pub worksheet_cell_visits: usize,
    pub chart_cell_lookups: usize,
    pub pivot_cell_lookups: usize,
    /// Number of actual worksheet `<c>` elements emitted, including explicitly
    /// stored empty cells. Absent coordinates emit no element.
    pub emitted_cells: usize,
}

impl XlsxExportStats {
    /// Sum of the measured source-cell pass events and coordinate lookups.
    pub fn source_cell_visits(&self) -> usize {
        self.native_snapshot_cell_visits
            + self.coordinate_cell_visits
            + self.shared_string_cell_visits
            + self.style_cell_visits
            + self.worksheet_grouping_cell_visits
            + self.worksheet_cell_visits
            + self.chart_cell_lookups
            + self.pivot_cell_lookups
    }
}

#[derive(Serialize, Deserialize)]
struct NativeWorkbook {
    id: uuid::Uuid,
    title: String,
    sheets: Vec<NativeSheet>,
    named_ranges: Vec<NamedRange>,
    pivot_tables: Vec<PivotTable>,
    metadata: WorkbookMetadata,
}

#[derive(Serialize, Deserialize)]
struct NativeSheet {
    id: uuid::Uuid,
    name: String,
    cells: Vec<(CellRef, Cell)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_column_width: Option<f32>,
    column_widths: HashMap<u32, f32>,
    row_heights: HashMap<u32, f32>,
    frozen_rows: u32,
    frozen_cols: u32,
    merged_cells: Vec<MergedRange>,
    conditional_formats: Vec<ConditionalFormat>,
    data_validations: Vec<DataValidation>,
    charts: Vec<SheetChart>,
}

impl From<&Workbook> for NativeWorkbook {
    fn from(workbook: &Workbook) -> Self {
        Self::from_with_stats(workbook, &mut XlsxExportStats::default())
    }
}

impl NativeWorkbook {
    fn from_with_stats(workbook: &Workbook, stats: &mut XlsxExportStats) -> Self {
        Self {
            id: workbook.id,
            title: workbook.title.clone(),
            sheets: workbook
                .sheets
                .iter()
                .map(|sheet| NativeSheet {
                    id: sheet.id,
                    name: sheet.name.clone(),
                    cells: sheet
                        .cells
                        .iter()
                        .map(|(reference, cell)| {
                            stats.native_snapshot_cell_visits += 1;
                            (reference.clone(), cell.clone())
                        })
                        .collect(),
                    default_column_width: sheet.default_column_width,
                    column_widths: sheet.column_widths.clone(),
                    row_heights: sheet.row_heights.clone(),
                    frozen_rows: sheet.frozen_rows,
                    frozen_cols: sheet.frozen_cols,
                    merged_cells: sheet.merged_cells.clone(),
                    conditional_formats: sheet.conditional_formats.clone(),
                    data_validations: sheet.data_validations.clone(),
                    charts: sheet.charts.clone(),
                })
                .collect(),
            named_ranges: workbook.named_ranges.clone(),
            pivot_tables: workbook.pivot_tables.clone(),
            metadata: workbook.metadata.clone(),
        }
    }
}

impl From<NativeWorkbook> for Workbook {
    fn from(workbook: NativeWorkbook) -> Self {
        Self {
            id: workbook.id,
            title: workbook.title,
            sheets: workbook
                .sheets
                .into_iter()
                .map(|sheet| Sheet {
                    id: sheet.id,
                    name: sheet.name,
                    cells: sheet.cells.into_iter().collect(),
                    default_column_width: sheet.default_column_width,
                    column_widths: sheet.column_widths,
                    row_heights: sheet.row_heights,
                    frozen_rows: sheet.frozen_rows,
                    frozen_cols: sheet.frozen_cols,
                    merged_cells: sheet.merged_cells,
                    conditional_formats: sheet.conditional_formats,
                    data_validations: sheet.data_validations,
                    charts: sheet.charts,
                })
                .collect(),
            named_ranges: workbook.named_ranges,
            pivot_tables: workbook.pivot_tables,
            metadata: workbook.metadata,
        }
    }
}

/// Convert a [`Workbook`] to a `.xlsx` byte vector (ZIP archive with Open XML).
pub fn workbook_to_xlsx(wb: &Workbook) -> Result<Vec<u8>, String> {
    workbook_to_xlsx_with_stats(wb).map(|(bytes, _)| bytes)
}

/// Export through the same production serializer and return measured cell
/// events. Counts describe the passes documented by [`XlsxExportStats`],
/// without imposing a latency, memory or total-CPU guarantee.
pub fn workbook_to_xlsx_with_stats(wb: &Workbook) -> Result<(Vec<u8>, XlsxExportStats), String> {
    let mut stats = XlsxExportStats::default();
    crate::ooxml_import::model_budget(&NativeWorkbook::from_with_stats(wb, &mut stats))?;
    if wb
        .sheets
        .iter()
        .flat_map(|sheet| sheet.cells.keys())
        .inspect(|_| stats.coordinate_cell_visits += 1)
        .any(|reference| reference.row >= 1_048_576 || reference.col >= 16_384)
    {
        return Err("Cell coordinates exceed XLSX worksheet dimensions".into());
    }
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();
    let mut points_remaining = syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES / 200;
    for chart in wb.sheets.iter().flat_map(|s| &s.charts) {
        let range_points = |start: &CellRef, end: &CellRef| -> usize {
            if start.row > end.row
                || start.col > end.col
                || end.row >= 1_048_576
                || end.col >= 16_384
            {
                return usize::MAX;
            }
            (u64::from(end.row - start.row + 1) * u64::from(end.col - start.col + 1))
                .try_into()
                .unwrap_or(usize::MAX)
        };
        let count = if !chart.series_sources.is_empty() {
            chart.series_sources.iter().fold(0usize, |sum, s| {
                sum.saturating_add(range_points(&s.value_start, &s.value_end))
                    .saturating_add(
                        s.label_start
                            .as_ref()
                            .zip(s.label_end.as_ref())
                            .map(|(a, b)| range_points(a, b))
                            .unwrap_or(0),
                    )
            })
        } else if let Some(data) = &chart.cached_data {
            data.datasets.iter().fold(data.labels.len(), |sum, s| {
                sum.saturating_add(s.values.len())
            })
        } else {
            range_points(&chart.data_start, &chart.data_end).saturating_mul(2)
        };
        points_remaining = points_remaining
            .checked_sub(count)
            .ok_or("Chart point expansion budget exceeded")?;
    }
    let pivot_parts = pivots::build(wb, &mut stats)?;
    let chart_count: usize = wb.sheets.iter().map(|sheet| sheet.charts.len()).sum();

    // Build the shared strings table across all sheets.
    let (shared_strings, ss_index) = build_shared_strings(wb, &mut stats);
    let styles = styles::Styles::new(wb, &mut stats);

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/customXml/syntra-workbook.json" ContentType="application/json"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
    );
    for (i, _) in wb.sheets.iter().enumerate() {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/xl/worksheets/sheet{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
            i + 1
        ));
        if !wb.sheets[i].charts.is_empty() {
            ct.push_str(&format!(
                "\n  <Override PartName=\"/xl/drawings/drawing{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>",
                i + 1
            ));
        }
    }
    for chart_number in 1..=chart_count {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/xl/charts/chart{chart_number}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>"
        ));
    }
    ct.push_str(&pivots::content_types(&pivot_parts));
    ct.push_str("\n</Types>");
    write!(zip, "{}", ct).map_err(|e| e.to_string())?;

    // _rels/.rels
    zip.start_file("_rels/.rels", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#
    )
    .map_err(|e| e.to_string())?;

    // xl/workbook.xml
    zip.start_file("xl/workbook.xml", options)
        .map_err(|e| e.to_string())?;
    let mut wbxml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>"#,
    );
    for (i, sheet) in wb.sheets.iter().enumerate() {
        wbxml.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            escape_xml(&sheet.name),
            i + 1,
            i + 1
        ));
    }
    wbxml.push_str("</sheets>");
    if !wb.named_ranges.is_empty() {
        wbxml.push_str("<definedNames>");
        for range in &wb.named_ranges {
            if let Some(sheet) = wb.sheets.iter().find(|sheet| sheet.id == range.sheet_id) {
                wbxml.push_str(&format!(
                    "<definedName name=\"{}\">'{}'!{}:{}</definedName>",
                    escape_xml(&range.name),
                    escape_xml(&sheet.name.replace('\'', "''")),
                    range.start.to_a1(),
                    range.end.to_a1()
                ));
            }
        }
        wbxml.push_str("</definedNames>");
    }
    wbxml.push_str(&pivots::workbook_caches(&pivot_parts));
    wbxml.push_str("</workbook>");
    write!(zip, "{}", wbxml).map_err(|e| e.to_string())?;

    // xl/_rels/workbook.xml.rels
    zip.start_file("xl/_rels/workbook.xml.rels", options)
        .map_err(|e| e.to_string())?;
    let mut wbrels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (i, _) in wb.sheets.iter().enumerate() {
        wbrels.push_str(&format!(
            "\n  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
            i + 1,
            i + 1
        ));
    }
    let styles_rid = wb.sheets.len() + 1;
    let ss_rid = wb.sheets.len() + 2;
    wbrels.push_str(&format!(
        "\n  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
        styles_rid
    ));
    wbrels.push_str(&format!(
        "\n  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/>",
        ss_rid
    ));
    wbrels.push_str(&pivots::workbook_rels(&pivot_parts));
    wbrels.push_str("\n</Relationships>");
    write!(zip, "{}", wbrels).map_err(|e| e.to_string())?;

    zip.start_file("xl/styles.xml", options)
        .map_err(|e| e.to_string())?;
    write!(zip, "{}", styles.xml(wb)).map_err(|e| e.to_string())?;

    // xl/sharedStrings.xml
    zip.start_file("xl/sharedStrings.xml", options)
        .map_err(|e| e.to_string())?;
    let mut ssxml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#,
        shared_strings.len(),
        shared_strings.len()
    );
    for s in &shared_strings {
        ssxml.push_str(&format!("<si><t>{}</t></si>", escape_xml(s)));
    }
    ssxml.push_str("</sst>");
    write!(zip, "{}", ssxml).map_err(|e| e.to_string())?;

    // xl/worksheets/sheetN.xml — one per sheet
    let mut differential_offset = 0usize;
    let mut chart_offset = 0usize;
    for (i, sheet) in wb.sheets.iter().enumerate() {
        zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), options)
            .map_err(|e| e.to_string())?;
        let mut sheet_xml = sheet_to_xml(
            sheet,
            &ss_index,
            &styles,
            differential_offset,
            (!sheet.charts.is_empty()).then_some(i + 1),
            &mut stats,
        );
        sheet_xml = sheet_xml.replace(
            "</worksheet>",
            &format!("{}</worksheet>", pivots::sheet_refs(&pivot_parts, i)),
        );
        write!(zip, "{}", sheet_xml).map_err(|e| e.to_string())?;
        differential_offset += sheet.conditional_formats.len();

        let pivot_rels = pivots::sheet_rels(&pivot_parts, i);
        if !sheet.charts.is_empty() || !pivot_rels.is_empty() {
            zip.start_file(
                format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1),
                options,
            )
            .map_err(|e| e.to_string())?;
            let drawing = if !sheet.charts.is_empty() {
                format!(
                    "<Relationship Id=\"rIdDrawing\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing{}.xml\"/>",
                    i + 1
                )
            } else {
                String::new()
            };
            write!(zip,"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{drawing}{pivot_rels}</Relationships>").map_err(|e|e.to_string())?;
        }
        if !sheet.charts.is_empty() {
            zip.start_file(format!("xl/drawings/drawing{}.xml", i + 1), options)
                .map_err(|e| e.to_string())?;
            write!(zip, "{}", drawing_xml(sheet, chart_offset)).map_err(|e| e.to_string())?;

            zip.start_file(
                format!("xl/drawings/_rels/drawing{}.xml.rels", i + 1),
                options,
            )
            .map_err(|e| e.to_string())?;
            write!(zip, "{}", drawing_relationships_xml(sheet, chart_offset))
                .map_err(|e| e.to_string())?;

            for chart in &sheet.charts {
                chart_offset += 1;
                zip.start_file(format!("xl/charts/chart{chart_offset}.xml"), options)
                    .map_err(|e| e.to_string())?;
                write!(zip, "{}", sheet_chart_xml(chart, sheet, &mut stats))
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    for pivot in pivot_parts {
        let number = pivot.index;
        for (path, content) in [
            (
                format!("xl/pivotTables/pivotTable{number}.xml"),
                pivot.definition,
            ),
            (
                format!("xl/pivotCache/pivotCacheDefinition{number}.xml"),
                pivot.cache,
            ),
            (
                format!("xl/pivotCache/pivotCacheRecords{number}.xml"),
                pivot.records,
            ),
            (
                format!("xl/pivotTables/_rels/pivotTable{number}.xml.rels"),
                format!(
                    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdCache\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition\" Target=\"../pivotCache/pivotCacheDefinition{number}.xml\"/></Relationships>"
                ),
            ),
            (
                format!("xl/pivotCache/_rels/pivotCacheDefinition{number}.xml.rels"),
                format!(
                    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdRecords\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheRecords\" Target=\"pivotCacheRecords{number}.xml\"/></Relationships>"
                ),
            ),
        ] {
            zip.start_file(path, options).map_err(|e| e.to_string())?;
            zip.write_all(content.as_bytes())
                .map_err(|e| e.to_string())?;
        }
    }
    zip.start_file("customXml/syntra-workbook.json", options)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut zip, &NativeWorkbook::from_with_stats(wb, &mut stats))
        .map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    let bytes = crate::native_package::seal(buf.into_inner(), "customXml/syntra-workbook.json")?;
    Ok((bytes, stats))
}

/// Build a shared strings table from all text cell values across all sheets.
///
/// Returns the ordered list of unique strings and a lookup map from string to
/// its index in the table.
fn build_shared_strings(
    wb: &Workbook,
    stats: &mut XlsxExportStats,
) -> (Vec<String>, HashMap<String, usize>) {
    let mut strings: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for sheet in &wb.sheets {
        for cell in sheet.cells.values() {
            stats.shared_string_cell_visits += 1;
            if let CellValue::Text(ref s) = cell.value
                && !index.contains_key(s)
            {
                let idx = strings.len();
                strings.push(s.clone());
                index.insert(s.clone(), idx);
            }
        }
    }

    (strings, index)
}

/// Generate the XML content for a single worksheet.
fn sheet_to_xml(
    sheet: &Sheet,
    ss_index: &HashMap<String, usize>,
    styles: &styles::Styles,
    differential_offset: usize,
    drawing_number: Option<usize>,
    stats: &mut XlsxExportStats,
) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
"#,
    );
    xml.push_str("<sheetViews><sheetView workbookViewId=\"0\">");
    if sheet.frozen_rows > 0 || sheet.frozen_cols > 0 {
        let top_left = CellRef::new(sheet.frozen_rows, sheet.frozen_cols).to_a1();
        let pane = if sheet.frozen_rows > 0 && sheet.frozen_cols > 0 {
            "bottomRight"
        } else if sheet.frozen_rows > 0 {
            "bottomLeft"
        } else {
            "topRight"
        };
        xml.push_str(&format!(
            "<pane xSplit=\"{}\" ySplit=\"{}\" topLeftCell=\"{top_left}\" activePane=\"{pane}\" state=\"frozen\"/>",
            sheet.frozen_cols, sheet.frozen_rows
        ));
    }
    xml.push_str("</sheetView></sheetViews>");
    if let Some(width) = sheet
        .default_column_width
        .filter(|w| w.is_finite() && *w >= 0.)
    {
        xml.push_str(&format!(
            "<sheetFormatPr defaultRowHeight=\"15\" defaultColWidth=\"{}\"/>",
            width / 7.
        ));
    }
    if !sheet.column_widths.is_empty() {
        xml.push_str("<cols>");
        let mut widths: Vec<_> = sheet.column_widths.iter().collect();
        widths.sort_by_key(|(col, _)| **col);
        for (col, width) in widths {
            if width.is_finite() && *col < 16_384 {
                xml.push_str(&format!(
                    "<col min=\"{}\" max=\"{}\" width=\"{}\" customWidth=\"1\"/>",
                    col + 1,
                    col + 1,
                    (width / 7.0).max(0.0)
                ));
            }
        }
        xml.push_str("</cols>");
    }
    xml.push_str("<sheetData>");

    if !sheet.cells.is_empty() {
        // Enumerate occupied coordinates: a distant single cell is still O(1).
        let mut rows: BTreeMap<u32, Vec<(u32, &Cell)>> = BTreeMap::new();
        for (reference, cell) in &sheet.cells {
            stats.worksheet_grouping_cell_visits += 1;
            rows.entry(reference.row)
                .or_default()
                .push((reference.col, cell));
        }
        for (row, mut row_cells) in rows {
            row_cells.sort_by_key(|(col, _)| *col);
            let height = sheet
                .row_heights
                .get(&row)
                .filter(|height| height.is_finite())
                .map(|height| format!(" ht=\"{}\" customHeight=\"1\"", height * 0.75))
                .unwrap_or_default();
            xml.push_str(&format!("<row r=\"{}\"{height}>", row + 1)); // 1-based row number

            for (col, cell) in &row_cells {
                stats.worksheet_cell_visits += 1;
                let cell_ref = CellRef::new(row, *col).to_a1();

                let cell_start = xml.len();
                match (&cell.value, &cell.formula) {
                    // Formula cell — emit <f> and optionally a cached <v>.
                    (_, Some(formula)) => {
                        let value_type = match &cell.value {
                            CellValue::Text(_) | CellValue::Date(_) => " t=\"str\"",
                            CellValue::Boolean(_) => " t=\"b\"",
                            CellValue::Error(_) => " t=\"e\"",
                            _ => "",
                        };
                        xml.push_str(&format!("<c r=\"{}\"{value_type}>", cell_ref));
                        xml.push_str(&format!(
                            "<f>{}</f>",
                            escape_xml(formula.trim_start_matches('='))
                        ));
                        // If there is a cached value, emit it.
                        match &cell.value {
                            CellValue::Number(n) => {
                                xml.push_str(&format!("<v>{}</v>", n));
                            }
                            CellValue::Text(s) | CellValue::Date(s) | CellValue::Error(s) => {
                                // For formula cells with text results, inline the
                                // string rather than using the shared strings table.
                                xml.push_str(&format!("<v>{}</v>", escape_xml(s)));
                            }
                            CellValue::Boolean(value) => {
                                xml.push_str(if *value { "<v>1</v>" } else { "<v>0</v>" })
                            }
                            _ => {}
                        }
                        xml.push_str("</c>");
                    }
                    // Number
                    (CellValue::Number(n), None) => {
                        xml.push_str(&format!("<c r=\"{}\" t=\"n\"><v>{}</v></c>", cell_ref, n));
                    }
                    // Text — reference shared string
                    (CellValue::Text(s), None) => {
                        if let Some(idx) = ss_index.get(s) {
                            xml.push_str(&format!(
                                "<c r=\"{}\" t=\"s\"><v>{}</v></c>",
                                cell_ref, idx
                            ));
                        }
                    }
                    // Boolean
                    (CellValue::Boolean(b), None) => {
                        xml.push_str(&format!(
                            "<c r=\"{}\" t=\"b\"><v>{}</v></c>",
                            cell_ref,
                            if *b { 1 } else { 0 }
                        ));
                    }
                    // Date — store as inline string
                    (CellValue::Date(d), None) => {
                        xml.push_str(&format!(
                            "<c r=\"{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                            cell_ref,
                            escape_xml(d)
                        ));
                    }
                    // Error — store as error type
                    (CellValue::Error(e), None) => {
                        xml.push_str(&format!(
                            "<c r=\"{}\" t=\"e\"><v>{}</v></c>",
                            cell_ref,
                            escape_xml(e)
                        ));
                    }
                    // Empty
                    (CellValue::Empty, None) => xml.push_str(&format!("<c r=\"{cell_ref}\"/>")),
                }
                if xml.len() > cell_start {
                    stats.emitted_cells += 1;
                    let style = styles.index(&cell.format);
                    xml.insert_str(cell_start + 2, &format!(" s=\"{style}\""));
                }
            }

            xml.push_str("</row>");
        }
    }

    xml.push_str("</sheetData>");
    if !sheet.merged_cells.is_empty() {
        xml.push_str(&format!(
            "<mergeCells count=\"{}\">",
            sheet.merged_cells.len()
        ));
        for range in &sheet.merged_cells {
            let start = range.start.to_a1();
            let end = range.end.to_a1();
            xml.push_str(&format!("<mergeCell ref=\"{start}:{end}\"/>"));
        }
        xml.push_str("</mergeCells>");
    }
    for (index, conditional) in sheet.conditional_formats.iter().enumerate() {
        let range = format!(
            "{}:{}",
            conditional.range_start.to_a1(),
            conditional.range_end.to_a1()
        );
        let (rule_type, operator, formulas) = conditional_rule_xml(&conditional.rule);
        xml.push_str(&format!(
            "<conditionalFormatting sqref=\"{range}\"><cfRule type=\"{rule_type}\" dxfId=\"{}\" priority=\"{}\"{operator}>{formulas}</cfRule></conditionalFormatting>",
            differential_offset + index,
            differential_offset + index + 1
        ));
    }
    if !sheet.data_validations.is_empty() {
        xml.push_str(&format!(
            "<dataValidations count=\"{}\">",
            sheet.data_validations.len()
        ));
        for validation in &sheet.data_validations {
            xml.push_str(&data_validation_xml(validation));
        }
        xml.push_str("</dataValidations>");
    }
    if drawing_number.is_some() {
        xml.push_str("<drawing r:id=\"rIdDrawing\"/>");
    }
    xml.push_str("</worksheet>");
    xml
}

fn differential_format_xml(format: &syntra_doc_engine::calc::CellFormat) -> String {
    let mut xml = String::from("<dxf>");
    if format.bold
        || format.italic
        || format.font_family.is_some()
        || format.font_size.is_some()
        || format.text_color.is_some()
    {
        xml.push_str("<font>");
        if format.bold {
            xml.push_str("<b/>");
        }
        if format.italic {
            xml.push_str("<i/>");
        }
        if let Some(font) = &format.font_family {
            xml.push_str(&format!("<name val=\"{}\"/>", escape_xml(font)));
        }
        if let Some(size) = format.font_size {
            xml.push_str(&format!("<sz val=\"{size}\"/>"));
        }
        if let Some(color) = &format.text_color {
            xml.push_str(&format!(
                "<color rgb=\"FF{}\"/>",
                escape_xml(color.trim_start_matches('#'))
            ));
        }
        xml.push_str("</font>");
    }
    if let Some(color) = &format.bg_color {
        xml.push_str(&format!(
            "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{}\"/><bgColor indexed=\"64\"/></patternFill></fill>",
            escape_xml(color.trim_start_matches('#'))
        ));
    }
    xml.push_str("</dxf>");
    xml
}

fn conditional_rule_xml(rule: &ConditionalRule) -> (&'static str, String, String) {
    match rule {
        ConditionalRule::GreaterThan(value) => (
            "cellIs",
            " operator=\"greaterThan\"".into(),
            format!("<formula>{value}</formula>"),
        ),
        ConditionalRule::LessThan(value) => (
            "cellIs",
            " operator=\"lessThan\"".into(),
            format!("<formula>{value}</formula>"),
        ),
        ConditionalRule::Between(low, high) => (
            "cellIs",
            " operator=\"between\"".into(),
            format!("<formula>{low}</formula><formula>{high}</formula>"),
        ),
        ConditionalRule::EqualTo(value) => (
            "cellIs",
            " operator=\"equal\"".into(),
            if value.parse::<f64>().is_ok() {
                format!("<formula>{}</formula>", escape_xml(value))
            } else {
                format!("<formula>\"{}\"</formula>", escape_xml(value))
            },
        ),
        ConditionalRule::ContainsText(value) => (
            "containsText",
            format!(" text=\"{}\"", escape_xml(value)),
            format!(
                "<formula>NOT(ISERROR(SEARCH(\"{}\",A1)))</formula>",
                escape_xml(value)
            ),
        ),
        ConditionalRule::IsEmpty => (
            "containsBlanks",
            String::new(),
            "<formula>LEN(TRIM(A1))=0</formula>".into(),
        ),
        ConditionalRule::IsNotEmpty => (
            "notContainsBlanks",
            String::new(),
            "<formula>LEN(TRIM(A1))&gt;0</formula>".into(),
        ),
    }
}

fn data_validation_xml(validation: &syntra_doc_engine::calc::DataValidation) -> String {
    let range = format!(
        "{}:{}",
        validation.range_start.to_a1(),
        validation.range_end.to_a1()
    );
    let (kind, operator, formula1, formula2) = match &validation.rule {
        ValidationRule::WholeNumberBetween(low, high) => {
            ("whole", "between", low.to_string(), Some(high.to_string()))
        }
        ValidationRule::DecimalBetween(low, high) => (
            "decimal",
            "between",
            low.to_string(),
            Some(high.to_string()),
        ),
        ValidationRule::List(values) => (
            "list",
            "between",
            format!("\"{}\"", values.join(",").replace('"', "\"\"")),
            None,
        ),
        ValidationRule::DateBetween(low, high) => {
            ("date", "between", low.clone(), Some(high.clone()))
        }
        ValidationRule::TextLengthBetween(low, high) => (
            "textLength",
            "between",
            low.to_string(),
            Some(high.to_string()),
        ),
        ValidationRule::CustomFormula(formula) => ("custom", "between", formula.clone(), None),
    };
    let mut xml = format!(
        "<dataValidation type=\"{kind}\" operator=\"{operator}\" allowBlank=\"1\" showInputMessage=\"1\" showErrorMessage=\"1\" sqref=\"{range}\""
    );
    if let Some(message) = &validation.input_message {
        xml.push_str(&format!(" prompt=\"{}\"", escape_xml(message)));
    }
    if let Some(message) = &validation.error_message {
        xml.push_str(&format!(" error=\"{}\"", escape_xml(message)));
    }
    xml.push_str(&format!("><formula1>{}</formula1>", escape_xml(&formula1)));
    if let Some(formula) = formula2 {
        xml.push_str(&format!("<formula2>{}</formula2>", escape_xml(&formula)));
    }
    xml.push_str("</dataValidation>");
    xml
}

fn drawing_xml(sheet: &Sheet, chart_offset: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
    );
    for (index, chart) in sheet.charts.iter().enumerate() {
        xml.push_str(&format!(
            "<xdr:twoCellAnchor><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"{}\" name=\"{}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart r:id=\"rIdChart{}\"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor>",
            chart.anchor_start.col,
            chart.anchor_start.row,
            chart.anchor_end.col,
            chart.anchor_end.row,
            index + 2,
            escape_xml(&chart.title),
            chart_offset + index + 1,
        ));
    }
    xml.push_str("</xdr:wsDr>");
    xml
}

fn drawing_relationships_xml(sheet: &Sheet, chart_offset: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for index in 0..sheet.charts.len() {
        let chart_number = chart_offset + index + 1;
        xml.push_str(&format!(
            "<Relationship Id=\"rIdChart{chart_number}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"../charts/chart{chart_number}.xml\"/>"
        ));
    }
    xml.push_str("</Relationships>");
    xml
}

fn sheet_chart_xml(chart: &SheetChart, sheet: &Sheet, stats: &mut XlsxExportStats) -> String {
    use syntra_doc_engine::slides::{ChartData, ChartType, Dataset, SlideChart};
    fn values(
        sheet: &Sheet,
        start: &CellRef,
        end: &CellRef,
        stats: &mut XlsxExportStats,
    ) -> Vec<CellValue> {
        let mut values = Vec::new();
        for row in start.row..=end.row {
            for col in start.col..=end.col {
                if values.len() >= 100_000 {
                    return values;
                }
                stats.chart_cell_lookups += 1;
                values.push(
                    sheet
                        .get_cell(row, col)
                        .map(|c| c.value.clone())
                        .unwrap_or(CellValue::Empty),
                );
            }
        }
        values
    }
    let mut data = chart.cached_data.clone().unwrap_or_else(|| ChartData {
        labels: Vec::new(),
        datasets: vec![Dataset {
            name: chart.title.clone(),
            values: Vec::new(),
            color: None,
        }],
    });
    if chart.cached_data.is_none() {
        data.datasets[0].values = values(sheet, &chart.data_start, &chart.data_end, stats)
            .into_iter()
            .map(|v| if let CellValue::Number(n) = v { n } else { 0.0 })
            .collect();
        data.labels = (1..=data.datasets[0].values.len())
            .map(|i| i.to_string())
            .collect();
    }
    for (dataset, source) in data.datasets.iter_mut().zip(&chart.series_sources) {
        dataset.values = values(sheet, &source.value_start, &source.value_end, stats)
            .into_iter()
            .map(|v| if let CellValue::Number(n) = v { n } else { 0.0 })
            .collect();
        if let (Some(start), Some(end)) = (&source.label_start, &source.label_end) {
            data.labels = values(sheet, start, end, stats)
                .into_iter()
                .map(|v| match v {
                    CellValue::Text(v) | CellValue::Date(v) | CellValue::Error(v) => v,
                    CellValue::Number(v) => v.to_string(),
                    CellValue::Boolean(v) => if v { "TRUE" } else { "FALSE" }.into(),
                    CellValue::Empty => String::new(),
                })
                .collect();
        }
    }
    let kind = match chart.chart_type {
        SheetChartType::Bar => ChartType::Bar,
        SheetChartType::Line => ChartType::Line,
        SheetChartType::Pie => ChartType::Pie,
        SheetChartType::Area => ChartType::Area,
        SheetChartType::Doughnut => ChartType::Doughnut,
        SheetChartType::Scatter => ChartType::Scatter,
    };
    let slide = SlideChart {
        id: chart.id,
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
        chart_type: kind,
        data,
    };
    let mut xml = crate::pptx::chart_part_xml(&slide);
    // Keep the worksheet title in a standard DrawingML chart title.
    let title = format!(
        "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx></c:title>",
        escape_xml(&chart.title)
    );
    xml = xml
        .replacen("<c:chart>", &format!("<c:chart>{title}"), 1)
        .replace("<c:autoTitleDeleted val=\"1\"/>", "");
    fn reference(sheet: &Sheet, start: &CellRef, end: &CellRef) -> String {
        escape_xml(&format!(
            "'{}'!{}:{}",
            sheet.name.replace('\'', "''"),
            start.to_a1(),
            end.to_a1()
        ))
    }
    // Keep standard range bindings as well as cached values, so Office refreshes
    // an imported chart after the user changes cells outside syntra.
    let mut offset = 0;
    for source in &chart.series_sources {
        let Some(start) = xml[offset..].find("<c:ser>").map(|i| i + offset) else {
            break;
        };
        let Some(end) = xml[start..].find("</c:ser>").map(|i| i + start + 8) else {
            break;
        };
        let mut series = xml[start..end].to_string();
        let value_ref = reference(sheet, &source.value_start, &source.value_end);
        // Last numeric literal is the Y/value cache (scatter X is earlier).
        if let Some(begin) = series.rfind("<c:numLit>")
            && let Some(close) = series[begin..].find("</c:numLit>").map(|i| i + begin)
        {
            series.replace_range(close..close + 11, "</c:numCache></c:numRef>");
            series.replace_range(
                begin..begin + 10,
                &format!("<c:numRef><c:f>{value_ref}</c:f><c:numCache>"),
            );
        }
        if let (Some(a), Some(b)) = (&source.label_start, &source.label_end) {
            let range = reference(sheet, a, b);
            if series.contains("<c:strLit>") {
                series = series
                    .replacen(
                        "<c:strLit>",
                        &format!("<c:strRef><c:f>{range}</c:f><c:strCache>"),
                        1,
                    )
                    .replacen("</c:strLit>", "</c:strCache></c:strRef>", 1);
            }
        }
        let length = series.len();
        xml.replace_range(start..end, &series);
        offset = start + length;
    }
    xml
}

/// Escape characters that are special in XML.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Parse a `.xlsx` byte vector into a [`Workbook`].
pub fn xlsx_to_workbook(bytes: &[u8]) -> Result<Workbook, String> {
    crate::ooxml::validate(bytes)?;
    let native_paths = [
        "customXml/syntra-workbook.json",
        "customXml/\x63ontrix-workbook.json",
    ];
    if let Some(model) = crate::native_package::read(bytes, &native_paths)
        && let Ok(workbook) = serde_json::from_value::<NativeWorkbook>(model)
    {
        return Ok(workbook.into());
    }
    external_import::preflight_shared_strings(bytes)?;
    let cursor = Cursor::new(bytes.to_vec());
    let mut xlsx: Xlsx<_> = Xlsx::new(cursor).map_err(|e| e.to_string())?;
    let sheet_names = xlsx.sheet_names().to_vec();
    let mut merged_by_sheet: HashMap<String, Vec<calamine::Dimensions>> = HashMap::new();
    for name in &sheet_names {
        let regions = xlsx
            .merge_cells_by_sheet_name(name)
            .map_err(|e| e.to_string())?;
        merged_by_sheet.insert(name.clone(), regions);
    }

    let title = sheet_names
        .first()
        .cloned()
        .unwrap_or_else(|| "Workbook".to_string());

    let mut wb = Workbook {
        id: uuid::Uuid::new_v4(),
        title,
        sheets: Vec::new(),
        named_ranges: Vec::new(),
        pivot_tables: Vec::new(),
        metadata: Default::default(),
    };

    for name in &sheet_names {
        let range = xlsx.worksheet_range(name).map_err(|e| e.to_string())?;
        let formulas = xlsx.worksheet_formula(name).map_err(|e| e.to_string())?;
        let mut sheet = Sheet::new(name.clone());
        let start = range.start().unwrap_or((0, 0));
        for (row, col, value) in range.cells() {
            let abs_row = start.0 + row as u32;
            let abs_col = start.1 + col as u32;
            if let Some(cv) = data_to_cell_value(value) {
                sheet.set_cell(abs_row, abs_col, cv);
            }
        }
        let start = formulas.start().unwrap_or((0, 0));
        for (row, col, formula) in formulas.cells() {
            if !formula.is_empty() {
                sheet.set_formula(
                    start.0 + row as u32,
                    start.1 + col as u32,
                    format!("={}", formula.trim_start_matches('=')),
                );
            }
        }
        if let Some(regions) = merged_by_sheet.get(name) {
            sheet.merged_cells = regions
                .iter()
                .map(|dimensions| MergedRange {
                    start: CellRef::new(dimensions.start.0, dimensions.start.1),
                    end: CellRef::new(dimensions.end.0, dimensions.end.1),
                })
                .collect();
        }
        wb.sheets.push(sheet);
    }

    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1".to_string()));
    }

    external_import::enrich(bytes, &mut wb)?;
    crate::ooxml_import::model_budget(&NativeWorkbook::from(&wb))?;
    Ok(wb)
}

fn data_to_cell_value(data: &Data) -> Option<CellValue> {
    match data {
        Data::Empty => None,
        Data::String(s) => Some(CellValue::Text(s.clone())),
        Data::Float(f) => Some(CellValue::Number(*f)),
        Data::Int(i) => Some(CellValue::Number(*i as f64)),
        Data::Bool(b) => Some(CellValue::Boolean(*b)),
        Data::DateTime(dt) => Some(CellValue::Text(dt.to_string())),
        Data::DateTimeIso(s) => Some(CellValue::Text(s.clone())),
        Data::DurationIso(s) => Some(CellValue::Text(s.clone())),
        Data::Error(e) => Some(CellValue::Error(format!("{:?}", e))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use syntra_doc_engine::calc::{CellRef, CellValue, MergedRange, Workbook};
    use zip::ZipArchive;

    #[test]
    fn test_xlsx_generation() {
        let mut wb = Workbook::new("Test".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Hello".into()));
        wb.sheets[0].set_cell(0, 1, CellValue::Number(42.0));
        let bytes = workbook_to_xlsx(&wb).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_xlsx_with_boolean() {
        let mut wb = Workbook::new("Bool Test".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Boolean(true));
        wb.sheets[0].set_cell(1, 0, CellValue::Boolean(false));
        let bytes = workbook_to_xlsx(&wb).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_xlsx_with_formula() {
        let mut wb = Workbook::new("Formula Test".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Number(10.0));
        wb.sheets[0].set_cell(1, 0, CellValue::Number(20.0));
        wb.sheets[0].set_formula(2, 0, "SUM(A1:A2)".to_string());
        wb.sheets[0].set_cell(2, 0, CellValue::Number(30.0));
        let bytes = workbook_to_xlsx(&wb).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_xlsx_multiple_sheets() {
        let mut wb = Workbook::new("Multi Sheet".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Sheet1 Data".into()));
        wb.add_sheet("Sheet2".to_string());
        wb.sheets[1].set_cell(0, 0, CellValue::Text("Sheet2 Data".into()));
        let bytes = workbook_to_xlsx(&wb).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_xlsx_contains_expected_files() {
        let mut wb = Workbook::new("Files Check".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Test".into()));
        let bytes = workbook_to_xlsx(&wb).unwrap();

        let reader = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let expected_files = [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/styles.xml",
            "xl/sharedStrings.xml",
            "xl/worksheets/sheet1.xml",
        ];
        for name in expected_files {
            assert!(
                archive.by_name(name).is_ok(),
                "Missing expected file: {}",
                name
            );
        }

        // Verify sharedStrings contains our text
        let mut ss_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
            &mut ss_xml,
        )
        .unwrap();
        assert!(ss_xml.contains("Test"));
    }

    #[test]
    fn test_xlsx_empty_workbook() {
        let wb = Workbook::new("Empty".into());
        let bytes = workbook_to_xlsx(&wb).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_xlsx_roundtrip_multi_sheet() {
        let mut wb = Workbook::new("RT".into());
        wb.sheets[0].name = "Alpha".into();
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Hello".into()));
        wb.sheets[0].set_cell(0, 1, CellValue::Number(42.0));
        wb.sheets[0].set_cell(1, 0, CellValue::Number(314.0 / 100.0));
        wb.add_sheet("Beta".to_string());
        wb.sheets[1].set_cell(0, 0, CellValue::Text("World".into()));
        wb.sheets[1].set_cell(2, 3, CellValue::Number(99.0));

        let bytes = workbook_to_xlsx(&wb).unwrap();
        let parsed = xlsx_to_workbook(&bytes).unwrap();

        assert_eq!(parsed.sheets.len(), 2);
        assert_eq!(parsed.sheets[0].name, "Alpha");
        assert_eq!(parsed.sheets[1].name, "Beta");

        match &parsed.sheets[0].get_cell(0, 0).unwrap().value {
            CellValue::Text(s) => assert_eq!(s, "Hello"),
            v => panic!("expected text, got {:?}", v),
        }
        match parsed.sheets[0].get_cell(0, 1).unwrap().value {
            CellValue::Number(n) => assert_eq!(n, 42.0),
            ref v => panic!("expected number, got {:?}", v),
        }
        match parsed.sheets[0].get_cell(1, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - (314.0 / 100.0)).abs() < 1e-9),
            ref v => panic!("expected number, got {:?}", v),
        }
        match &parsed.sheets[1].get_cell(0, 0).unwrap().value {
            CellValue::Text(s) => assert_eq!(s, "World"),
            v => panic!("expected text, got {:?}", v),
        }
        match parsed.sheets[1].get_cell(2, 3).unwrap().value {
            CellValue::Number(n) => assert_eq!(n, 99.0),
            ref v => panic!("expected number, got {:?}", v),
        }
    }

    #[test]
    fn test_xlsx_merged_cells_round_trip() {
        let mut wb = Workbook::new("Merged".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Heading".into()));
        wb.sheets[0].merged_cells.push(MergedRange {
            start: CellRef::new(0, 0),
            end: CellRef::new(1, 2),
        });

        let bytes = workbook_to_xlsx(&wb).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut sheet_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
            &mut sheet_xml,
        )
        .unwrap();
        assert!(sheet_xml.contains(r#"<mergeCell ref="A1:C2"/>"#));
        drop(archive);

        let parsed = xlsx_to_workbook(&bytes).unwrap();
        assert_eq!(parsed.sheets[0].merged_cells.len(), 1);
        let merged = &parsed.sheets[0].merged_cells[0];
        assert_eq!(merged.start, CellRef::new(0, 0));
        assert_eq!(merged.end, CellRef::new(1, 2));
    }

    #[test]
    fn test_xlsx_to_workbook_from_bytes() {
        let mut wb = Workbook::new("Bytes".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("Foo".into()));
        wb.sheets[0].set_cell(0, 1, CellValue::Number(7.0));
        wb.sheets[0].set_cell(1, 1, CellValue::Boolean(true));

        let bytes: Vec<u8> = workbook_to_xlsx(&wb).unwrap();
        let parsed = xlsx_to_workbook(bytes.as_slice()).unwrap();

        assert_eq!(parsed.sheets.len(), 1);
        let s = &parsed.sheets[0];
        match &s.get_cell(0, 0).unwrap().value {
            CellValue::Text(t) => assert_eq!(t, "Foo"),
            v => panic!("expected text, got {:?}", v),
        }
        match s.get_cell(0, 1).unwrap().value {
            CellValue::Number(n) => assert_eq!(n, 7.0),
            ref v => panic!("expected number, got {:?}", v),
        }
    }

    #[test]
    fn test_shared_strings_dedup() {
        let mut wb = Workbook::new("Dedup".into());
        wb.sheets[0].set_cell(0, 0, CellValue::Text("same".into()));
        wb.sheets[0].set_cell(1, 0, CellValue::Text("same".into()));
        wb.sheets[0].set_cell(2, 0, CellValue::Text("different".into()));
        let (strings, index) = build_shared_strings(&wb, &mut XlsxExportStats::default());
        assert_eq!(strings.len(), 2);
        assert_eq!(index.len(), 2);
        assert!(index.contains_key("same"));
        assert!(index.contains_key("different"));
    }

    #[test]
    fn spreadsheet_tools_are_emitted_and_round_trip() {
        use syntra_doc_engine::calc::{
            ConditionalFormat, DataValidation, NamedRange, SheetChart, SheetChartType,
            ValidationRule,
        };
        let mut wb = Workbook::new("Tools".into());
        let sheet_id = wb.sheets[0].id;
        wb.sheets[0].frozen_rows = 1;
        wb.sheets[0].frozen_cols = 1;
        wb.sheets[0].set_cell(0, 0, CellValue::Number(12.0));
        let format = syntra_doc_engine::calc::CellFormat {
            bg_color: Some("#FEE2E2".into()),
            ..Default::default()
        };
        wb.sheets[0].conditional_formats.push(ConditionalFormat {
            range_start: CellRef::new(0, 0),
            range_end: CellRef::new(9, 0),
            rule: ConditionalRule::GreaterThan(10.0),
            format,
        });
        wb.sheets[0].data_validations.push(DataValidation {
            range_start: CellRef::new(0, 1),
            range_end: CellRef::new(9, 1),
            rule: ValidationRule::List(vec!["Open".into(), "Closed".into()]),
            input_message: Some("Choose a status".into()),
            error_message: Some("Invalid status".into()),
        });
        wb.sheets[0].charts.push(SheetChart {
            id: uuid::Uuid::new_v4(),
            title: "Values".into(),
            chart_type: SheetChartType::Bar,
            data_start: CellRef::new(0, 0),
            data_end: CellRef::new(9, 0),
            anchor_start: CellRef::new(1, 3),
            anchor_end: CellRef::new(12, 10),
            cached_data: None,
            series_sources: Vec::new(),
        });
        wb.named_ranges.push(NamedRange {
            name: "Values".into(),
            sheet_id,
            start: CellRef::new(0, 0),
            end: CellRef::new(9, 0),
        });

        let bytes = workbook_to_xlsx(&wb).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        assert!(archive.by_name("xl/charts/chart1.xml").is_ok());
        assert!(archive.by_name("xl/drawings/drawing1.xml").is_ok());
        let mut sheet_xml = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet_xml)
            .unwrap();
        assert!(sheet_xml.contains("state=\"frozen\""));
        assert!(sheet_xml.contains("<conditionalFormatting"));
        assert!(sheet_xml.contains("<dataValidations"));
        drop(archive);

        let imported = xlsx_to_workbook(&bytes).unwrap();
        assert_eq!(imported.named_ranges.len(), 1);
        assert_eq!(imported.sheets[0].conditional_formats.len(), 1);
        assert_eq!(imported.sheets[0].data_validations.len(), 1);
        assert_eq!(imported.sheets[0].charts.len(), 1);
    }
}
