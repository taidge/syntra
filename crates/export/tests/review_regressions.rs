use std::io::{Cursor, Read};
use syntra_doc_engine::calc::{
    CellRef, CellValue, PivotAggregation, PivotTable, Sheet, SheetChart, SheetChartType, Workbook,
};
use syntra_doc_engine::slides::SlideElement;
use syntra_export::xlsx::{XlsxExportStats, workbook_to_xlsx_with_stats};
use zip::ZipArchive;
fn part(bytes: &[u8], name: &str) -> String {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn functional_saved_workbook_csv_recalculates_lazy_conditions_and_lookup_dependencies() {
    let mut book = Workbook::new("Lazy formulas".into());
    let sheet = &mut book.sheets[0];
    sheet.set_formula(0, 0, "=IF(TRUE,3,A1)".into());
    sheet.set_formula(0, 1, "=A1*2".into());
    sheet.set_formula(0, 2, "=SUMIF(A1:A1,3,B1:B1)".into());
    sheet.set_formula(0, 3, "=VLOOKUP(3,A1:B1,2,FALSE)".into());

    // Persisted formula cells have no fresh cache. Match the conversion path:
    // deserialize the saved model, recalculate, then render CSV values.
    let saved = serde_json::to_vec(&book).unwrap();
    let mut loaded: Workbook = serde_json::from_slice(&saved).unwrap();
    loaded.sheets[0].recalculate();
    assert_eq!(
        syntra_export::csv::sheet_to_csv(&loaded.sheets[0]),
        "3,6,6,6\r\n"
    );
    loaded.sheets[0].set_formula(0, 0, "=IF(TRUE,4,A1)".into());
    loaded.sheets[0].set_formula(0, 2, "=SUMIF(A1:A1,4,B1:B1)".into());
    loaded.sheets[0].set_formula(0, 3, "=VLOOKUP(4,A1:B1,2,FALSE)".into());
    loaded.sheets[0].recalculate();
    assert_eq!(
        syntra_export::csv::sheet_to_csv(&loaded.sheets[0]),
        "4,8,8,8\r\n"
    );
}

#[test]
fn external_xlsx_keeps_formulas_and_recalculates_after_edit() {
    let mut book =
        syntra_export::xlsx::xlsx_to_workbook(include_bytes!("fixtures/external-formulas.xlsx"))
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
    let bytes = syntra_export::xlsx::workbook_to_xlsx(&book).unwrap();
    assert!(part(&bytes, "xl/worksheets/sheet1.xml").contains("<f>A1+1</f>"));
}
#[test]
fn sparse_xlsx_visits_only_occupied_cells_and_writes_real_styles() {
    let mut near_stats = None;
    for (row, col) in [(0, 0), (1_048_575, 16_383)] {
        let mut book = Workbook::new("Sparse".into());
        let sheet = &mut book.sheets[0];
        sheet.set_cell(row, col, CellValue::Number(42.0));
        let cell = sheet.cells.get_mut(&CellRef::new(row, col)).unwrap();
        cell.format.bold = true;
        cell.format.text_color = Some("#123456".into());
        cell.format.number_format = Some("0.00%".into());
        let (bytes, stats) = workbook_to_xlsx_with_stats(&book).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(
            stats,
            XlsxExportStats {
                native_snapshot_cell_visits: 2,
                coordinate_cell_visits: 1,
                shared_string_cell_visits: 1,
                style_cell_visits: 1,
                worksheet_grouping_cell_visits: 1,
                worksheet_cell_visits: 1,
                emitted_cells: 1,
                ..Default::default()
            }
        );
        assert_eq!(stats.source_cell_visits(), 7);
        if let Some(near) = near_stats {
            assert_eq!(stats, near, "a larger empty rectangle adds no cell visits");
        } else {
            near_stats = Some(stats);
        }
        let xml = part(&bytes, "xl/worksheets/sheet1.xml");
        assert_eq!(xml.matches("<row ").count(), 1);
        assert_eq!(xml.matches("<c ").count(), stats.emitted_cells);
        assert!(xml.contains(&CellRef::new(row, col).to_a1()));
        assert!(xml.contains(" s=\"1\""));
        let styles = part(&bytes, "xl/styles.xml");
        assert!(styles.contains("<b/>"));
        assert!(styles.contains("FF123456"));
        assert!(styles.contains("formatCode=\"0.00%\""));
        assert!(styles.contains("applyNumberFormat=\"1\""));
        let restored = syntra_export::xlsx::xlsx_to_workbook(&bytes).unwrap();
        assert!(matches!(
            restored.sheets[0].get_cell(row, col).unwrap().value,
            CellValue::Number(42.0)
        ));
        let ordinary = syntra_export::xlsx::workbook_to_xlsx(&book).unwrap();
        assert_eq!(part(&ordinary, "xl/worksheets/sheet1.xml"), xml);
        assert_eq!(part(&ordinary, "xl/styles.xml"), styles);
    }
}

#[test]
fn xlsx_cell_counts_include_stored_empty_cells_and_all_sheets() {
    let empty = Workbook::new("Empty".into());
    let (_, stats) = workbook_to_xlsx_with_stats(&empty).unwrap();
    assert_eq!(stats, XlsxExportStats::default());

    let mut book = Workbook::new("Stored cells".into());
    book.sheets[0].set_cell(0, 0, CellValue::Empty);
    book.sheets.push(Sheet::new("Second".into()));
    book.sheets[1].set_cell(1_048_575, 16_383, CellValue::Text("edge".into()));
    let (bytes, stats) = workbook_to_xlsx_with_stats(&book).unwrap();
    assert_eq!(stats.native_snapshot_cell_visits, 4);
    assert_eq!(stats.coordinate_cell_visits, 2);
    assert_eq!(stats.shared_string_cell_visits, 2);
    assert_eq!(stats.style_cell_visits, 2);
    assert_eq!(stats.worksheet_grouping_cell_visits, 2);
    assert_eq!(stats.worksheet_cell_visits, 2);
    assert_eq!(stats.source_cell_visits(), 14);
    assert_eq!(stats.emitted_cells, 2);
    let emitted: usize = (1..=2)
        .map(|index| {
            part(&bytes, &format!("xl/worksheets/sheet{index}.xml"))
                .matches("<c ")
                .count()
        })
        .sum();
    assert_eq!(emitted, stats.emitted_cells);
    let restored = syntra_export::xlsx::xlsx_to_workbook(&bytes).unwrap();
    assert!(matches!(
        restored.sheets[0].get_cell(0, 0).unwrap().value,
        CellValue::Empty
    ));
    assert!(matches!(
        &restored.sheets[1].get_cell(1_048_575, 16_383).unwrap().value,
        CellValue::Text(value) if value == "edge"
    ));
}

#[test]
fn xlsx_cell_counts_include_chart_and_pivot_coordinate_lookups() {
    let mut book = Workbook::new("Lookup counts".into());
    let sheet = &mut book.sheets[0];
    sheet.set_cell(0, 0, CellValue::Text("Group".into()));
    sheet.set_cell(0, 1, CellValue::Text("Amount".into()));
    sheet.set_cell(1, 0, CellValue::Text("A".into()));
    sheet.set_cell(1, 1, CellValue::Number(10.0));
    sheet.charts.push(SheetChart {
        drawing: None,
        id: uuid::Uuid::new_v4(),
        title: "Reads an absent coordinate too".into(),
        chart_type: SheetChartType::Bar,
        data_start: CellRef::new(1, 0),
        data_end: CellRef::new(1, 2),
        anchor_start: CellRef::new(3, 0),
        anchor_end: CellRef::new(10, 4),
        cached_data: None,
        series_sources: Vec::new(),
    });
    let sheet_id = sheet.id;
    book.pivot_tables.push(PivotTable {
        id: uuid::Uuid::new_v4(),
        name: "Counts".into(),
        source_sheet_id: sheet_id,
        source_start: CellRef::new(1, 0),
        source_end: CellRef::new(1, 1),
        target_sheet_id: sheet_id,
        target: CellRef::new(3, 0),
        row_field_col: 0,
        value_field_col: 1,
        aggregation: PivotAggregation::Sum,
    });
    let (bytes, stats) = workbook_to_xlsx_with_stats(&book).unwrap();
    assert_eq!(stats.chart_cell_lookups, 3);
    assert_eq!(
        stats.pivot_cell_lookups, 4,
        "two data cells plus two headers"
    );
    assert_eq!(stats.source_cell_visits(), 4 * 7 + 3 + 4);
    assert_eq!(stats.emitted_cells, 4);
    let chart = part(&bytes, "xl/charts/chart1.xml");
    assert_eq!(chart.matches("<c:ser>").count(), 1);
    // This fixture has no series_sources, so its values and categories use
    // OOXML literals. Explicit worksheet bindings use numRef/numCache instead.
    let values = chart
        .split_once("<c:val><c:numLit>")
        .unwrap()
        .1
        .split_once("</c:numLit></c:val>")
        .unwrap()
        .0;
    assert!(values.contains("<c:formatCode>General</c:formatCode>"));
    assert!(values.contains("<c:ptCount val=\"3\"/>"));
    assert_eq!(values.matches("<c:pt ").count(), 3);
    for (index, value) in [0, 10, 0].into_iter().enumerate() {
        assert!(values.contains(&format!("<c:pt idx=\"{index}\"><c:v>{value}</c:v></c:pt>")));
    }
    let categories = chart
        .split_once("<c:cat><c:strLit>")
        .unwrap()
        .1
        .split_once("</c:strLit></c:cat>")
        .unwrap()
        .0;
    assert!(categories.contains("<c:ptCount val=\"3\"/>"));
    assert_eq!(categories.matches("<c:pt ").count(), 3);
    for (index, label) in [1, 2, 3].into_iter().enumerate() {
        assert!(categories.contains(&format!("<c:pt idx=\"{index}\"><c:v>{label}</c:v></c:pt>")));
    }
    assert!(part(&bytes, "xl/pivotCache/pivotCacheRecords1.xml").contains("count=\"1\""));
    assert!(matches!(
        syntra_export::xlsx::xlsx_to_workbook(&bytes)
            .unwrap()
            .sheets[0]
            .get_cell(1, 1)
            .unwrap()
            .value,
        CellValue::Number(10.0)
    ));
}
#[test]
fn external_pptx_uses_relationship_order_excludes_orphans_and_reports_fidelity() {
    let presentation = syntra_export::pptx::pptx_to_presentation(include_bytes!(
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

#[test]
fn reordered_external_pptx_keeps_rich_elements_and_explicit_degradation_warnings() {
    // The browser uploads and recovers these same independently authored bytes.
    let bytes = include_bytes!("fixtures/external-rich-reordered.pptx");
    let archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert!(archive.file_names().all(|name| {
        !name
            .expect("valid ZIP entry name")
            .starts_with("customXml/")
    }));
    let order = part(bytes, "ppt/presentation.xml");
    assert!(order.find("r:id=\"leading\"").unwrap() < order.find("r:id=\"slide\"").unwrap());
    let presentation = syntra_export::pptx::pptx_to_presentation(bytes).unwrap();
    assert_eq!(presentation.slides.len(), 2);
    assert!(matches!(&presentation.slides[0].elements[0],
        SlideElement::TextBox(text) if text.content[0].text == "Second file, first slide"));
    let rich = &presentation.slides[1];
    assert!(matches!(&rich.elements[0],
        SlideElement::TextBox(text) if text.content[0].text == "Rich "));
    assert!(matches!(&rich.elements[1],
        SlideElement::Shape(shape) if shape.fill_color.as_deref() == Some("#CC5500")));
    assert!(matches!(&rich.elements[2],
        SlideElement::Image(image) if image.url.starts_with("data:image/png;base64,")
            && image.alt_text == "Tiny red pixel"));
    assert!(matches!(&rich.elements[4],
        SlideElement::Chart(chart) if chart.data.datasets[0].values == vec![10., 20., 5.]));
    let warnings = presentation.metadata.import_warnings.join(" ");
    assert!(warnings.contains("External media"));
    assert!(warnings.contains("SmartArt"));
}
