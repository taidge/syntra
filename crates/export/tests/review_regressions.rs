use contrix_doc_engine::calc::{CellRef, CellValue, Workbook};
use contrix_doc_engine::slides::SlideElement;
use std::io::{Cursor, Read};
use zip::ZipArchive;
fn part(bytes: &[u8], name: &str) -> String {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut xml).unwrap();
    xml
}
#[test]
fn external_xlsx_keeps_formulas_and_recalculates_after_edit() {
    let mut book =
        contrix_export::xlsx::xlsx_to_workbook(include_bytes!("fixtures/external-formulas.xlsx"))
            .unwrap();
    assert_eq!(
        book.sheets[0].get_cell(0, 1).unwrap().formula.as_deref(),
        Some("=A1+1")
    );
    book.sheets[0].set_cell(0, 0, CellValue::Number(10.0));
    book.sheets[0].recalculate();
    assert!(matches!(
        book.sheets[0].get_cell(0, 1).unwrap().value,
        CellValue::Number(11.0)
    ));
    let bytes = contrix_export::xlsx::workbook_to_xlsx(&book).unwrap();
    assert!(part(&bytes, "xl/worksheets/sheet1.xml").contains("<f>A1+1</f>"));
}
#[test]
fn sparse_xlsx_visits_only_occupied_cells_and_writes_real_styles() {
    let mut book = Workbook::new("Sparse".into());
    let sheet = &mut book.sheets[0];
    sheet.set_cell(1_048_575, 16_383, CellValue::Number(42.0));
    let cell = sheet
        .cells
        .get_mut(&CellRef::new(1_048_575, 16_383))
        .unwrap();
    cell.format.bold = true;
    cell.format.text_color = Some("#123456".into());
    cell.format.number_format = Some("0.00%".into());
    let bytes = contrix_export::xlsx::workbook_to_xlsx(&book).unwrap();
    let xml = part(&bytes, "xl/worksheets/sheet1.xml");
    assert_eq!(xml.matches("<row ").count(), 1);
    assert_eq!(xml.matches("<c ").count(), 1);
    assert!(xml.contains("XFD1048576"));
    assert!(xml.contains(" s=\"1\""));
    let styles = part(&bytes, "xl/styles.xml");
    assert!(styles.contains("<b/>"));
    assert!(styles.contains("FF123456"));
    assert!(styles.contains("formatCode=\"0.00%\""));
    assert!(styles.contains("applyNumberFormat=\"1\""));
}
#[test]
fn external_pptx_uses_relationship_order_excludes_orphans_and_reports_fidelity() {
    let presentation = contrix_export::pptx::pptx_to_presentation(include_bytes!(
        "fixtures/external-slide-order.pptx"
    ))
    .unwrap();
    assert_eq!(presentation.slides.len(), 2);
    let text = |index: usize| match &presentation.slides[index].elements[0] {
        SlideElement::TextBox(text) => text.content[0].text.clone(),
        _ => panic!("expected text"),
    };
    assert_eq!(text(0), "Second file, first slide");
    assert_eq!(text(1), "First file, second slide");
    assert!(!presentation.metadata.import_warnings.is_empty());
}
