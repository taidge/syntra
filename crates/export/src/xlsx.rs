//! XLSX export for Calc workbooks.
//!
//! Converts a [`Workbook`] into a `.xlsx` byte vector (a ZIP archive
//! containing Open XML Spreadsheet files).  The output is compatible with
//! Microsoft Excel, LibreOffice Calc, and other spreadsheet applications that
//! support the OOXML format.

use std::collections::HashMap;
use std::io::{Cursor, Write};

use calamine::{Data, Reader, Xlsx};
use contrix_doc_engine::calc::{CellRef, CellValue, Sheet, Workbook};
use zip::write::{SimpleFileOptions, ZipWriter};

/// Convert a [`Workbook`] to a `.xlsx` byte vector (ZIP archive with Open XML).
pub fn workbook_to_xlsx(wb: &Workbook) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();

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
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
    );
    for (i, _) in wb.sheets.iter().enumerate() {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/xl/worksheets/sheet{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
            i + 1
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
    wbxml.push_str("</sheets></workbook>");
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

    // xl/styles.xml — minimal styles
    zip.start_file("xl/styles.xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
</styleSheet>"#
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
    for (i, sheet) in wb.sheets.iter().enumerate() {
        zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), options)
            .map_err(|e| e.to_string())?;
        let sheet_xml = sheet_to_xml(sheet, &ss_index);
        write!(zip, "{}", sheet_xml).map_err(|e| e.to_string())?;
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
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
            if let CellValue::Text(ref s) = cell.value {
                if !index.contains_key(s) {
                    let idx = strings.len();
                    strings.push(s.clone());
                    index.insert(s.clone(), idx);
                }
            }
        }
    }

    (strings, index)
}

/// Generate the XML content for a single worksheet.
fn sheet_to_xml(sheet: &Sheet, ss_index: &HashMap<String, usize>) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheetData>"#,
    );

    if sheet.cells.is_empty() {
        xml.push_str("</sheetData></worksheet>");
        return xml;
    }

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
                        xml.push_str(&format!("<c r=\"{}\" t=\"s\"><v>{}</v></c>", cell_ref, idx));
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

    xml.push_str("</sheetData></worksheet>");
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
    let cursor = Cursor::new(bytes.to_vec());
    let mut xlsx: Xlsx<_> = Xlsx::new(cursor).map_err(|e| e.to_string())?;
    let sheet_names = xlsx.sheet_names().to_vec();

    let title = sheet_names
        .first()
        .cloned()
        .unwrap_or_else(|| "Workbook".to_string());

    let mut wb = Workbook {
        id: uuid::Uuid::new_v4(),
        title,
        sheets: Vec::new(),
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
    use contrix_doc_engine::calc::{CellValue, Workbook};

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
        wb.sheets[0].set_cell(1, 0, CellValue::Number(3.14));
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
            CellValue::Number(n) => assert!((n - 3.14).abs() < 1e-9),
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
}
