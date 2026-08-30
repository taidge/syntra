//! XLSX export for Calc workbooks.
//!
//! Converts a [`Workbook`] into a `.xlsx` byte vector (a ZIP archive
//! containing Open XML Spreadsheet files).  The output is compatible with
//! Microsoft Excel, LibreOffice Calc, and other spreadsheet applications that
//! support the OOXML format.

use std::collections::HashMap;
use std::io::{Cursor, Write};

use calamine::{Data, Reader, Xlsx};
use contrix_doc_engine::calc::{
    Cell, CellRef, CellValue, ConditionalFormat, ConditionalRule, DataValidation, MergedRange,
    NamedRange, PivotTable, Sheet, SheetChart, SheetChartType, ValidationRule, Workbook,
    WorkbookMetadata,
};
use serde::{Deserialize, Serialize};
use zip::write::{SimpleFileOptions, ZipWriter};

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
                        .map(|(reference, cell)| (reference.clone(), cell.clone()))
                        .collect(),
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
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();
    let chart_count: usize = wb.sheets.iter().map(|sheet| sheet.charts.len()).sum();

    // Build the shared strings table across all sheets.
    let (shared_strings, ss_index) = build_shared_strings(wb);

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/customXml/contrix-workbook.json" ContentType="application/json"/>
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
    wbrels.push_str("\n</Relationships>");
    write!(zip, "{}", wbrels).map_err(|e| e.to_string())?;

    // xl/styles.xml — base style plus differential styles used by
    // conditional formatting.
    zip.start_file("xl/styles.xml", options)
        .map_err(|e| e.to_string())?;
    let differential_formats = wb
        .sheets
        .iter()
        .flat_map(|sheet| sheet.conditional_formats.iter())
        .map(|conditional| differential_format_xml(&conditional.format))
        .collect::<String>();
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
  <dxfs count="{}">{}</dxfs>
</styleSheet>"#,
        wb.sheets
            .iter()
            .map(|sheet| sheet.conditional_formats.len())
            .sum::<usize>(),
        differential_formats,
    )
    .map_err(|e| e.to_string())?;

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
        let sheet_xml = sheet_to_xml(
            sheet,
            &ss_index,
            differential_offset,
            (!sheet.charts.is_empty()).then_some(i + 1),
        );
        write!(zip, "{}", sheet_xml).map_err(|e| e.to_string())?;
        differential_offset += sheet.conditional_formats.len();

        if !sheet.charts.is_empty() {
            zip.start_file(
                format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1),
                options,
            )
            .map_err(|e| e.to_string())?;
            write!(
                zip,
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdDrawing\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"../drawings/drawing{}.xml\"/></Relationships>",
                i + 1
            )
            .map_err(|e| e.to_string())?;

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
                write!(zip, "{}", sheet_chart_xml(chart, sheet)).map_err(|e| e.to_string())?;
            }
        }
    }

    zip.start_file("customXml/contrix-workbook.json", options)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut zip, &NativeWorkbook::from(wb)).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    crate::native_package::seal(buf.into_inner(), "customXml/contrix-workbook.json")
}

/// Build a shared strings table from all text cell values across all sheets.
///
/// Returns the ordered list of unique strings and a lookup map from string to
/// its index in the table.
fn build_shared_strings(wb: &Workbook) -> (Vec<String>, HashMap<String, usize>) {
    let mut strings: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for sheet in &wb.sheets {
        for cell in sheet.cells.values() {
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
    differential_offset: usize,
    drawing_number: Option<usize>,
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
    xml.push_str("</sheetView></sheetViews><sheetData>");

    if !sheet.cells.is_empty() {
        // Determine bounding rectangle.
        let max_row = sheet.cells.keys().map(|r| r.row).max().unwrap_or(0);
        let max_col = sheet.cells.keys().map(|r| r.col).max().unwrap_or(0);

        for row in 0..=max_row {
            // Collect cells in this row.
            let mut row_cells: Vec<(u32, &contrix_doc_engine::calc::Cell)> = Vec::new();
            for col in 0..=max_col {
                if let Some(cell) = sheet.get_cell(row, col) {
                    row_cells.push((col, cell));
                }
            }
            if row_cells.is_empty() {
                continue;
            }

            xml.push_str(&format!("<row r=\"{}\">", row + 1)); // 1-based row number

            for (col, cell) in &row_cells {
                let cell_ref = CellRef::new(row, *col).to_a1();

                match (&cell.value, &cell.formula) {
                    // Formula cell — emit <f> and optionally a cached <v>.
                    (_, Some(formula)) => {
                        xml.push_str(&format!("<c r=\"{}\">", cell_ref));
                        xml.push_str(&format!("<f>{}</f>", escape_xml(formula)));
                        // If there is a cached value, emit it.
                        match &cell.value {
                            CellValue::Number(n) => {
                                xml.push_str(&format!("<v>{}</v>", n));
                            }
                            CellValue::Text(s) => {
                                // For formula cells with text results, inline the
                                // string rather than using the shared strings table.
                                xml.push_str(&format!("<v>{}</v>", escape_xml(s)));
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
                    (CellValue::Empty, None) => {}
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

fn differential_format_xml(format: &contrix_doc_engine::calc::CellFormat) -> String {
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
            format!("<formula>\"{}\"</formula>", escape_xml(value)),
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

fn data_validation_xml(validation: &contrix_doc_engine::calc::DataValidation) -> String {
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

fn sheet_chart_xml(chart: &SheetChart, sheet: &Sheet) -> String {
    let chart_tag = match chart.chart_type {
        SheetChartType::Bar => "barChart",
        SheetChartType::Line => "lineChart",
        SheetChartType::Pie => "pieChart",
        SheetChartType::Area => "areaChart",
    };
    let range = format!(
        "'{}'!{}:{}",
        sheet.name.replace('\'', "''"),
        chart.data_start.to_a1(),
        chart.data_end.to_a1()
    );
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:layout/><c:{chart_tag}>"#,
        escape_xml(&chart.title)
    );
    if matches!(chart.chart_type, SheetChartType::Bar) {
        xml.push_str("<c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>");
    }
    xml.push_str(&format!(
        "<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx><c:v>{}</c:v></c:tx><c:val><c:numRef><c:f>{}</c:f></c:numRef></c:val></c:ser>",
        escape_xml(&chart.title),
        escape_xml(&range)
    ));
    xml.push_str(&format!(
        "</c:{chart_tag}></c:plotArea><c:plotVisOnly val=\"1\"/></c:chart></c:chartSpace>"
    ));
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
    if let Some(model) = crate::native_package::read(bytes, "customXml/contrix-workbook.json")
        && let Ok(workbook) = serde_json::from_value::<NativeWorkbook>(model)
    {
        return Ok(workbook.into());
    }
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
        let mut sheet = Sheet::new(name.clone());
        let start = range.start().unwrap_or((0, 0));
        for (row, col, value) in range.cells() {
            let abs_row = start.0 + row as u32;
            let abs_col = start.1 + col as u32;
            if let Some(cv) = data_to_cell_value(value) {
                sheet.set_cell(abs_row, abs_col, cv);
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
    use contrix_doc_engine::calc::{CellRef, CellValue, MergedRange, Workbook};
    use std::io::Read;
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
        let (strings, index) = build_shared_strings(&wb);
        assert_eq!(strings.len(), 2);
        assert_eq!(index.len(), 2);
        assert!(index.contains_key("same"));
        assert!(index.contains_key("different"));
    }

    #[test]
    fn spreadsheet_tools_are_emitted_and_round_trip() {
        use contrix_doc_engine::calc::{
            ConditionalFormat, DataValidation, NamedRange, SheetChart, SheetChartType,
            ValidationRule,
        };
        let mut wb = Workbook::new("Tools".into());
        let sheet_id = wb.sheets[0].id;
        wb.sheets[0].frozen_rows = 1;
        wb.sheets[0].frozen_cols = 1;
        wb.sheets[0].set_cell(0, 0, CellValue::Number(12.0));
        let format = contrix_doc_engine::calc::CellFormat {
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
