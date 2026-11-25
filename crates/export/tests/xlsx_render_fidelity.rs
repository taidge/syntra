//! Independent office fixture: values, styles, drawing positions and the print
//! path must survive import and ordinary XLSX export without a native cache.
use std::io::{Cursor, Read, Write};
use syntra_doc_engine::calc::{BorderStyle, CellValue, HAlign, format_cell_value};
use syntra_export::{pdf, xlsx};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const FIXTURE: &[u8] = include_bytes!("fixtures/render-audit.xlsx");

#[test]
fn external_dates_formats_and_drawing_anchors_survive_standard_round_trip() {
    let original = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    assert_eq!(original.metadata.xlsx_import_revision, 2);
    let sheet = &original.sheets[0];
    for (row, col) in [(4, 1), (4, 2), (9, 4)] {
        assert_eq!(
            sheet.get_cell(row, col).unwrap().format.horizontal_align,
            HAlign::Right
        );
    }
    let date = sheet.get_cell(4, 1).unwrap();
    assert!(matches!(date.value, CellValue::Date(_)));
    assert_eq!(
        format_cell_value(&date.value, date.format.number_format.as_deref()).text,
        "2026-11-01"
    );
    let currency = sheet.get_cell(6, 2).unwrap();
    let display = format_cell_value(&currency.value, currency.format.number_format.as_deref());
    assert_eq!(display.text, "(¥300.00)");
    assert_eq!(display.color.as_deref(), Some("#FF0000"));
    assert_eq!(sheet.images.len(), 1);
    assert!(
        sheet.images[0]
            .data_url
            .starts_with("data:image/png;base64,")
    );
    assert!((sheet.images[0].drawing.width - 260.).abs() < 1.);
    assert!((sheet.images[0].drawing.height - 140.).abs() < 1.);
    assert_eq!(sheet.charts.len(), 1);
    assert!(sheet.charts[0].drawing.is_some());
    let encoded = xlsx::workbook_to_xlsx(&original).unwrap();
    let mut source = ZipArchive::new(Cursor::new(encoded)).unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut part = source.by_index(index).unwrap();
        let name = part.name().unwrap().into_owned();
        if name.starts_with("customXml/") {
            continue;
        }
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        let mut data = Vec::new();
        part.read_to_end(&mut data).unwrap();
        zip.write_all(&data).unwrap();
    }
    let standard = zip.finish().unwrap().into_inner();
    let restored = xlsx::xlsx_to_workbook(&standard).unwrap();
    let restored = &restored.sheets[0];
    assert_eq!(restored.images.len(), 1);
    assert_eq!(restored.images[0].data_url, sheet.images[0].data_url);
    assert_eq!(restored.charts.len(), 1);
    let before = sheet.chart_bounds(&sheet.charts[0]);
    let after = restored.chart_bounds(&restored.charts[0]);
    assert!(
        (before.0 - after.0).abs() < 1.
            && (before.1 - after.1).abs() < 1.
            && (before.2 - after.2).abs() < 1.
            && (before.3 - after.3).abs() < 1.
    );
    assert!(matches!(
        restored.get_cell(4, 1).unwrap().value,
        CellValue::Date(_)
    ));
}

#[test]
fn print_uses_pixel_widths_all_columns_and_anchored_images_and_charts() {
    let book = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    let print = pdf::document_to_print_html(&serde_json::to_value(&book).unwrap(), "calc").unwrap();
    assert_eq!(print.html.matches("<section class=\"sheet\"").count(), 2);
    assert!(print.html.contains("width:133px"));
    assert!(!print.html.contains("width:705px"));
    assert!(print.html.contains("(¥300.00)"));
    assert!(print.html.contains("2026-11-01"));
    assert!(print.html.contains("5.0%"));
    assert!(print.html.contains("10,792.59"));
    assert!(print.html.contains("sheet-drawing sheet-image"));
    assert!(print.html.contains("data:image/png;base64,"));
    assert!(print.html.contains("sheet-drawing sheet-chart"));
    assert!(print.html.contains("worksheet-chart-background"));
    assert!(print.html.contains("每月活动数量"));
    assert!(print.html.contains("white-space:pre-wrap"));
    assert!(print.html.contains("border-bottom:2pt double"));
}

#[test]
fn renamed_chart_title_overrides_the_imported_presentation_in_all_render_paths() {
    let mut book = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    book.sheets[0].charts[0].title = "Updated chart title".into();
    let data = book.sheets[0].chart_data(&book.sheets[0].charts[0]);
    assert_eq!(
        data.presentation.as_ref().unwrap().title.as_deref(),
        Some("Updated chart title")
    );
    let print = pdf::document_to_print_html(&serde_json::to_value(&book).unwrap(), "calc").unwrap();
    assert!(print.html.contains("Updated chart title"));
    assert!(!print.html.contains("每月活动数量"));
    let exported = xlsx::workbook_to_xlsx(&book).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(exported)).unwrap();
    let mut xml = String::new();
    archive
        .by_name("xl/charts/chart1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains("Updated chart title"));
    assert!(!xml.contains("每月活动数量"));
}

#[test]
fn merged_cell_print_borders_include_the_hidden_edge_cells_without_mutating_them() {
    let book = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    let sheet = &book.sheets[1];
    let original = serde_json::to_value(sheet).unwrap();
    let anchor = &sheet.get_cell(8, 0).unwrap().format;
    assert!(anchor.borders.right.is_none() && anchor.borders.bottom.is_none());
    let effective = sheet.effective_format(8, 0);
    for edge in [
        &effective.borders.top,
        &effective.borders.left,
        &effective.borders.right,
        &effective.borders.bottom,
    ] {
        let edge = edge.as_ref().unwrap();
        assert_eq!(edge.style, BorderStyle::Medium);
        assert_eq!(edge.color, "#B45309");
    }
    assert_eq!(serde_json::to_value(sheet).unwrap(), original);
    let printed =
        pdf::document_to_print_html(&serde_json::to_value(&book).unwrap(), "calc").unwrap();
    assert!(printed.html.contains("border-right:1pt solid #B45309"));
    assert!(printed.html.contains("border-bottom:1pt solid #B45309"));
}

#[test]
fn conditional_fills_export_rgb_for_both_differential_fill_colors() {
    let book = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    let exported = xlsx::workbook_to_xlsx(&book).unwrap();
    let mut zip = ZipArchive::new(Cursor::new(exported)).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/styles.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let differential = xml.split("<dxfs").nth(1).unwrap();
    assert!(
        differential.contains("<fgColor rgb=\"FFDCFCE7\"/>")
            && differential.contains("<bgColor rgb=\"FFDCFCE7\"/>")
    );
    assert!(!differential.contains("<bgColor indexed=\"64\"/>"));
}

fn alignment_fixture(horizontal: Option<&str>) -> Vec<u8> {
    alignment_fixture_inherited(horizontal, None)
}

fn alignment_fixture_inherited(horizontal: Option<&str>, base_horizontal: Option<&str>) -> Vec<u8> {
    let alignment = horizontal
        .map(|value| format!(" horizontal=\"{value}\""))
        .unwrap_or_default();
    let styles = format!(
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"><alignment{alignment} vertical="center"/></xf></cellXfs></styleSheet>"#
    );
    let styles = if let Some(value) = base_horizontal {
        styles.replace("<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>",&format!("<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"><alignment horizontal=\"{value}\"/></xf></cellStyleXfs>"))
    } else {
        styles
    };
    let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>123</v></c><c r="B1"><f>1+2</f></c><c r="C1" t="inlineStr"><is><t>123</t></is></c><c r="D1" t="d"><v>2026-11-01T00:00:00.000Z</v></c></row></sheetData></worksheet>"#;
    let mut source = ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut part = source.by_index(index).unwrap();
        let name = part.name().unwrap().into_owned();
        if name.starts_with("customXml/") {
            continue;
        }
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        if name == "xl/styles.xml" {
            bytes = styles.as_bytes().to_vec();
        } else if name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml") {
            bytes = sheet.as_bytes().to_vec();
        }
        output
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn general_alignment_follows_value_type_and_preserves_explicit_alignment() {
    for horizontal in [None, Some("general")] {
        let mut book = xlsx::xlsx_to_workbook(&alignment_fixture(horizontal)).unwrap();
        let sheet = &mut book.sheets[0];
        for col in [0, 1, 3] {
            assert_eq!(
                sheet.get_cell(0, col).unwrap().format.horizontal_align,
                HAlign::Right
            );
        }
        assert_eq!(
            sheet.get_cell(0, 2).unwrap().format.horizontal_align,
            HAlign::Left
        );
        assert!(matches!(
            sheet.get_cell(0, 2).unwrap().value,
            CellValue::Text(_)
        ));
        sheet.recalculate();
        assert!(matches!(
            sheet.get_cell(0, 1).unwrap().value,
            CellValue::Number(3.)
        ));
        assert_eq!(
            sheet.get_cell(0, 1).unwrap().format.horizontal_align,
            HAlign::Right
        );
    }
    for (horizontal, expected) in [
        ("left", HAlign::Left),
        ("center", HAlign::Center),
        ("right", HAlign::Right),
    ] {
        let book = xlsx::xlsx_to_workbook(&alignment_fixture(Some(horizontal))).unwrap();
        for col in [0, 1, 2, 3] {
            assert_eq!(
                book.sheets[0]
                    .get_cell(0, col)
                    .unwrap()
                    .format
                    .horizontal_align,
                expected
            );
        }
    }
    for (horizontal, expected) in [(None, HAlign::Left), (Some("center"), HAlign::Center)] {
        let book =
            xlsx::xlsx_to_workbook(&alignment_fixture_inherited(horizontal, Some("left"))).unwrap();
        assert_eq!(
            book.sheets[0]
                .get_cell(0, 0)
                .unwrap()
                .format
                .horizontal_align,
            expected
        );
    }
}

#[test]
fn excessive_drawing_placements_and_far_anchor_print_expansion_are_bounded() {
    let mut source = ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut part = source.by_index(index).unwrap();
        let name = part.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        if name == "xl/drawings/drawing1.xml" {
            let mut xml = String::from_utf8(bytes).unwrap();
            let end = xml.rfind("</").unwrap();
            let placements="<audit:oneCellAnchor xmlns:audit=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\"><audit:pic/></audit:oneCellAnchor>".repeat(1001);
            xml.insert_str(end, &placements);
            bytes = xml.into_bytes();
        }
        output
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    let error = xlsx::xlsx_to_workbook(&output.finish().unwrap().into_inner()).unwrap_err();
    assert!(error.contains("1000-drawing"));
    let mut book = xlsx::xlsx_to_workbook(FIXTURE).unwrap();
    book.sheets[0].images[0].anchor_start.row = 1_000_000;
    let error =
        pdf::document_to_print_html(&serde_json::to_value(&book).unwrap(), "calc").unwrap_err();
    assert!(error.contains("500-page"));
    let picture = book.sheets[0].images[0].clone();
    book.sheets[0].images = vec![picture; 1001];
    assert!(
        xlsx::workbook_to_xlsx(&book)
            .unwrap_err()
            .contains("drawing-count")
    );
}
