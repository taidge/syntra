//! Fixtures are hand-authored standard Office parts, with no syntra cache.
use calamine::{Data, Reader, Xlsx};
use std::io::{Cursor, Read, Write};
use syntra_doc_engine::{calc::*, slides::*};
use zip::{
    ZipArchive,
    write::{SimpleFileOptions, ZipWriter},
};

fn part(bytes: &[u8], path: &str) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    archive
        .by_name(path)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}
fn replace(bytes: &[u8], path: &str, value: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        if name.starts_with("customXml/") {
            continue;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        output
            .start_file(&name, SimpleFileOptions::default())
            .unwrap();
        output
            .write_all(if name == path {
                value.as_bytes()
            } else {
                &data
            })
            .unwrap();
    }
    output.finish().unwrap().into_inner()
}
#[test]
fn external_xlsx_imports_styles_dimensions_names_validations_chart_and_simple_pivot() {
    let bytes = include_bytes!("fixtures/external-rich.xlsx");
    let mut book = syntra_export::xlsx::xlsx_to_workbook(bytes).unwrap();
    let sheet = &book.sheets[0];
    let cell = sheet.get_cell(1, 1).unwrap();
    assert!(cell.format.bold && cell.format.italic && cell.format.underline);
    assert_eq!(cell.format.font_family.as_deref(), Some("Georgia"));
    assert_eq!(cell.format.font_size, Some(16.));
    assert_eq!(cell.format.text_color.as_deref(), Some("#336699"));
    assert_eq!(cell.format.bg_color.as_deref(), Some("#FFEECC"));
    assert_eq!(cell.format.horizontal_align, HAlign::Center);
    assert_eq!(cell.format.vertical_align, VAlign::Top);
    assert!(cell.format.wrap_text);
    assert_eq!(cell.format.number_format.as_deref(), Some("0.00%"));
    assert_eq!(cell.format.borders.left.as_ref().unwrap().color, "#123456");
    assert_eq!(
        cell.format.borders.bottom.as_ref().unwrap().style,
        BorderStyle::Double
    );
    assert_eq!(sheet.frozen_rows, 1);
    assert_eq!(sheet.frozen_cols, 1);
    assert_eq!(sheet.column_widths.get(&0), Some(&140.));
    assert_eq!(sheet.row_heights.get(&0), Some(&40.));
    assert!(
        matches!(&sheet.get_cell(1,3).unwrap().value,CellValue::Date(value) if value=="2024-01-01")
    );
    assert_eq!(book.named_ranges.len(), 1);
    assert_eq!(book.named_ranges[0].name, "SalesValues");
    assert_eq!(sheet.data_validations.len(), 1);
    assert!(
        !sheet.data_validations[0]
            .rule
            .accepts(&CellValue::Number(101.))
    );
    assert_eq!(sheet.conditional_formats.len(), 1);
    assert_eq!(
        sheet.conditional_formats[0].format.text_color.as_deref(),
        Some("#AA0000")
    );
    assert_eq!(sheet.charts.len(), 1);
    let chart = &sheet.charts[0];
    assert_eq!(chart.anchor_start, CellRef::new(1, 4));
    assert_eq!(
        chart.cached_data.as_ref().unwrap().datasets[0].values,
        vec![10., 20., 5.]
    );
    assert_eq!(chart.series_sources.len(), 1);
    assert_eq!(book.pivot_tables.len(), 1);
    assert_eq!(book.pivot_tables[0].source_start, CellRef::new(1, 0));
    assert_eq!(book.pivot_tables[0].target, CellRef::new(1, 0));
    let warnings = book.metadata.import_warnings.join(" ");
    assert!(
        warnings.contains("validation")
            && warnings.contains("conditional")
            && warnings.contains("defined name")
    );
    book.sheets[0].set_cell(1, 1, CellValue::Number(40.));
    let exported = syntra_export::xlsx::workbook_to_xlsx(&book).unwrap();
    let chart = part(&exported, "xl/charts/chart1.xml");
    assert!(chart.contains("<c:numCache>") && chart.contains("<c:v>40</c:v>"));
    assert!(chart.contains("Data&apos;!B2:B4"));
    assert!(part(&exported, "xl/workbook.xml").contains("SalesValues"));
    assert!(part(&exported, "xl/worksheets/sheet1.xml").contains("<dataValidations"));
    assert!(part(&exported, "xl/workbook.xml").contains("<pivotCache cacheId=\"0\""));
    assert!(
        part(&exported, "xl/worksheets/sheet2.xml").contains("<pivotTablePart r:id=\"rIdPivot1\"")
    );
    assert!(part(&exported, "xl/pivotTables/pivotTable1.xml").contains("subtotal=\"sum\""));
    assert!(
        part(&exported, "xl/pivotCache/pivotCacheDefinition1.xml")
            .contains("<worksheetSource ref=\"A1:B4\" sheet=\"Data\"")
    );
    assert!(part(&exported, "xl/pivotCache/pivotCacheRecords1.xml").contains("count=\"3\""));
    let external = replace(&exported, "unused", "");
    let reparsed = syntra_export::xlsx::xlsx_to_workbook(&external).unwrap();
    assert_eq!(reparsed.pivot_tables.len(), 1);
    assert_eq!(reparsed.named_ranges.len(), 1);
    assert_eq!(reparsed.sheets[0].charts[0].series_sources.len(), 1);
    assert_eq!(
        reparsed.sheets[0].charts[0]
            .cached_data
            .as_ref()
            .unwrap()
            .datasets[0]
            .values,
        vec![40., 20., 5.]
    );
    // A separate reader consumes standard parts, without using native customXml.
    let mut independent = Xlsx::new(Cursor::new(exported)).unwrap();
    assert_eq!(
        independent
            .worksheet_range("Data")
            .unwrap()
            .get_value((1, 1)),
        Some(&Data::Float(40.))
    );
    assert_eq!(
        independent
            .worksheet_formula("Data")
            .unwrap()
            .get_value((0, 2))
            .map(String::as_str),
        Some("SUM(B2:B4)")
    );
}
#[test]
fn external_pptx_imports_actual_coordinates_theme_runs_images_groups_charts_and_timing() {
    let bytes = include_bytes!("fixtures/external-rich.pptx");
    let presentation = syntra_export::pptx::pptx_to_presentation(bytes).unwrap();
    assert_eq!(presentation.metadata.slide_width, 1280.);
    assert_eq!(presentation.metadata.slide_height, 720.);
    let slide = &presentation.slides[0];
    let SlideElement::TextBox(text) = &slide.elements[0] else {
        panic!("title textbox")
    };
    assert_eq!(
        (text.x, text.y, text.width, text.height),
        (80., 60., 1100., 80.)
    );
    assert_eq!(text.content[0].text, "Rich ");
    assert!(text.content[0].bold);
    assert_eq!(text.content[0].font_size, Some(24.));
    assert_eq!(text.content[0].font_family.as_deref(), Some("Georgia"));
    assert_eq!(text.content[0].color.as_deref(), Some("#336699"));
    assert_eq!(
        text.content[0].link.as_deref(),
        Some("https://example.org/safe")
    );
    assert!(text.content[1].italic && text.content[1].underline);
    let SlideElement::Shape(shape) = &slide.elements[1] else {
        panic!("shape")
    };
    assert_eq!(shape.shape_type, ShapeType::RoundedRectangle);
    assert_eq!(shape.rotation, 15.);
    assert_eq!(shape.fill_color.as_deref(), Some("#CC5500"));
    let SlideElement::Image(image) = &slide.elements[2] else {
        panic!("picture")
    };
    assert_eq!(
        (image.x, image.y, image.width, image.height),
        (400., 180., 200., 100.)
    );
    assert!(image.url.starts_with("data:image/png;base64,"));
    assert_eq!(image.alt_text, "Tiny red pixel");
    assert!(image.crop.is_some());
    let SlideElement::Group(group) = &slide.elements[3] else {
        panic!("group")
    };
    let SlideElement::Shape(grouped) = &group.elements[0] else {
        panic!("group child")
    };
    assert_eq!(
        (grouped.x, grouped.y, grouped.width, grouped.height),
        (100., 440., 100., 60.)
    );
    let SlideElement::Chart(chart) = &slide.elements[4] else {
        panic!("chart")
    };
    assert_eq!(
        (chart.x, chart.y, chart.width, chart.height),
        (700., 180., 500., 300.)
    );
    assert_eq!(chart.data.labels, vec!["East", "West", "East"]);
    assert_eq!(chart.data.datasets[0].values, vec![10., 20., 5.]);
    assert_eq!(slide.notes, "Presenter notes");
    assert_eq!(slide.transition.as_ref().unwrap().duration_ms, 750);
    assert_eq!(slide.animations.len(), 1);
    assert_eq!(slide.animations[0].target_id, shape.id);
    assert_eq!(slide.animations[0].effect, AnimationEffect::Fade);
    assert_eq!(slide.animations[0].duration_ms, 600);
    assert_eq!(slide.animations[0].delay_ms, 120);
    let model = serde_json::to_string(&presentation).unwrap();
    assert!(!model.contains("127.0.0.1")); // external media never enters the renderable model.
    assert!(
        presentation
            .metadata
            .import_warnings
            .join(" ")
            .contains("External media")
    );
    assert!(
        presentation
            .metadata
            .import_warnings
            .join(" ")
            .contains("SmartArt")
    );
    let exported = syntra_export::pptx::presentation_to_pptx(&presentation).unwrap();
    assert!(part(&exported, "ppt/presentation.xml").contains("cx=\"12192000\""));
    assert!(part(&exported, "ppt/slides/slide1.xml").contains("<p:pic>"));
    assert!(part(&exported, "ppt/charts/chart1.xml").contains("<c:v>20</c:v>"));
    // Strip the native part, then verify the standard exported elements.
    let external = replace(&exported, "unused", "");
    let reparsed = syntra_export::pptx::pptx_to_presentation(&external).unwrap();
    assert!(
        reparsed.slides[0]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Image(_)))
    );
    assert!(
        reparsed.slides[0]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Chart(_)))
    );
}
#[test]
fn external_xml_budget_is_enforced_at_public_import_entry() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let deep = format!("{}{}", "<x>".repeat(130), "</x>".repeat(130));
    let bytes = replace(fixture, "ppt/slides/slide1.xml", &deep);
    assert!(
        syntra_export::pptx::pptx_to_presentation(&bytes)
            .unwrap_err()
            .contains("budget")
    );
    let wide = format!("<root>{}</root>", "<item/>".repeat(200_001));
    let bytes = replace(fixture, "ppt/slides/slide1.xml", &wide);
    assert!(
        syntra_export::pptx::pptx_to_presentation(&bytes)
            .unwrap_err()
            .contains("budget")
    );
}

#[test]
fn external_hyperlinks_are_filtered_and_media_is_budgeted_before_base64() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let links = part(fixture, "ppt/slides/_rels/slide1.xml.rels")
        .replace("https://example.org/safe", "javascript:alert(1)");
    let unsafe_file = replace(fixture, "ppt/slides/_rels/slide1.xml.rels", &links);
    let model = syntra_export::pptx::pptx_to_presentation(&unsafe_file).unwrap();
    let SlideElement::TextBox(text) = &model.slides[0].elements[0] else {
        panic!("textbox")
    };
    assert!(text.content[0].link.is_none());
    assert!(model.metadata.import_warnings.join(" ").contains("unsafe"));
    // Highly compressed media would fit the ZIP upload limit, but must be
    // rejected before creating a base64 String or a renderable image model.
    let oversized = replace(
        fixture,
        "ppt/media/pixel.png",
        &"x".repeat(2 * 1024 * 1024 + 1),
    );
    let model = syntra_export::pptx::pptx_to_presentation(&oversized).unwrap();
    assert!(
        !model.slides[0]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Image(_)))
    );
    assert!(model.metadata.import_warnings.join(" ").contains("budget"));
    let slide = part(fixture, "ppt/slides/slide1.xml");
    let begin = slide.find("<p:pic>").unwrap();
    let end = slide[begin..].find("</p:pic>").unwrap() + begin + 8;
    let slide = slide.replacen(&slide[begin..end], &slide[begin..end].repeat(10), 1);
    let repeated = replace(fixture, "ppt/media/pixel.png", &"x".repeat(1024 * 1024));
    let repeated = replace(&repeated, "ppt/slides/slide1.xml", &slide);
    let model = syntra_export::pptx::pptx_to_presentation(&repeated).unwrap();
    assert_eq!(
        model.slides[0]
            .elements
            .iter()
            .filter(|e| matches!(e, SlideElement::Image(_)))
            .count(),
        8
    );
    assert!(
        serde_json::to_vec(&model).unwrap().len() <= syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES
    );
    assert!(model.metadata.import_warnings.join(" ").contains("budget"));
}

#[test]
fn external_import_cannot_create_a_model_larger_than_the_editor_can_save() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let slide = part(fixture, "ppt/slides/slide1.xml").replace(
        "Rich ",
        &"x".repeat(syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES),
    );
    let bytes = replace(fixture, "ppt/slides/slide1.xml", &slide);
    assert!(
        syntra_export::pptx::pptx_to_presentation(&bytes)
            .unwrap_err()
            .contains("document content budget")
    );
}

#[test]
fn imported_pivot_count_and_average_follow_office_nonempty_numeric_rules() {
    let mut book =
        syntra_export::xlsx::xlsx_to_workbook(include_bytes!("fixtures/external-rich.xlsx"))
            .unwrap();
    let id = book.pivot_tables[0].id;
    book.sheets[0].set_cell(1, 1, CellValue::Number(40.));
    book.sheets[0].set_cell(2, 1, CellValue::Text("not numeric".into()));
    book.sheets[0].set_cell(3, 1, CellValue::Empty);
    book.pivot_tables[0].aggregation = PivotAggregation::Average;
    book.refresh_pivot(id).unwrap();
    assert!(matches!(
        book.sheets[1].get_cell(1, 1).unwrap().value,
        CellValue::Number(40.)
    ));
    book.pivot_tables[0].aggregation = PivotAggregation::Count;
    book.refresh_pivot(id).unwrap();
    assert!(matches!(
        book.sheets[1].get_cell(1, 1).unwrap().value,
        CellValue::Number(1.)
    ));
    assert!(matches!(
        book.sheets[1].get_cell(2, 1).unwrap().value,
        CellValue::Number(1.)
    ));
}

#[test]
fn relationship_attribute_prefixes_and_invalid_non_ascii_colors_are_safe() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let slide = part(fixture, "ppt/slides/slide1.xml")
        .replace("xmlns:r=", "xmlns:rel=")
        .replace("r:embed=", "rel:embed=")
        .replace("r:id=", "rel:id=")
        .replace("r:link=", "rel:link=")
        .replace("val=\"123456\"", "val=\"中abc\"");
    let bytes = replace(fixture, "ppt/slides/slide1.xml", &slide);
    let model = syntra_export::pptx::pptx_to_presentation(&bytes).unwrap();
    assert!(
        model.slides[0]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Image(_)))
    );
    let SlideElement::Shape(shape) = &model.slides[0].elements[1] else {
        panic!("shape")
    };
    assert_eq!(shape.fill_color.as_deref(), Some("#CC5500"));
    assert_eq!(shape.border.as_ref().unwrap().color, "#000000");
    let fixture = include_bytes!("fixtures/external-rich.xlsx");
    let styles = part(fixture, "xl/styles.xml").replace("FF123456", "中abcde");
    let bytes = replace(fixture, "xl/styles.xml", &styles);
    assert!(syntra_export::xlsx::xlsx_to_workbook(&bytes).is_ok());
}

#[test]
fn sparse_chart_indices_share_a_document_wide_expansion_budget() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let chart = part(fixture, "ppt/charts/chart1.xml");
    let begin = chart.find("<c:ser>").unwrap();
    let end = chart.find("</c:ser>").unwrap() + 8;
    let series = chart[begin..end].replace(
        "<c:pt idx=\"2\"><c:v>5</c:v>",
        "<c:pt idx=\"99999\"><c:v>5</c:v>",
    );
    let chart = chart.replace(&chart[begin..end], &series.repeat(2));
    let bytes = replace(fixture, "ppt/charts/chart1.xml", &chart);
    let model = syntra_export::pptx::pptx_to_presentation(&bytes).unwrap();
    assert!(
        !model.slides[0]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Chart(_)))
    );
    assert!(
        model
            .metadata
            .import_warnings
            .join(" ")
            .contains("point expansion budget")
    );
}

#[test]
fn master_decorations_and_embedded_background_images_are_real_standard_objects() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let slide = part(fixture, "ppt/slides/slide1.xml");
    let first = slide.find("</p:sp>").unwrap() + 7;
    let begin = slide[first..].find("<p:sp>").unwrap() + first;
    let end = slide[begin..].find("</p:sp>").unwrap() + begin + 7;
    let decoration = slide[begin..end].replace("id=\"3\"", "id=\"30\"");
    let master = part(fixture, "ppt/slideMasters/slideMaster1.xml")
        .replace("<p:spTree/>", &format!("<p:spTree>{decoration}</p:spTree>"));
    let bytes = replace(fixture, "ppt/slideMasters/slideMaster1.xml", &master);
    let slide = slide.replace(
        "<a:solidFill><a:srgbClr val=\"F0F4F8\"/></a:solidFill>",
        "<a:blipFill><a:blip r:embed=\"img\"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>",
    );
    let bytes = replace(&bytes, "ppt/slides/slide1.xml", &slide);
    let model = syntra_export::pptx::pptx_to_presentation(&bytes).unwrap();
    assert!(matches!(&model.slides[0].elements[0],SlideElement::Shape(s) if s.x==80. && s.y==180.));
    assert!(
        matches!(&model.slides[0].background,Background::Image(url) if url.starts_with("data:image/png;base64,"))
    );
    let exported = syntra_export::pptx::presentation_to_pptx(&model).unwrap();
    assert!(part(&exported, "ppt/slides/slide1.xml").contains("<p:bg><p:bgPr><a:blipFill>"));
    let model =
        syntra_export::pptx::pptx_to_presentation(&replace(&exported, "unused", "")).unwrap();
    assert!(
        matches!(&model.slides[0].background,Background::Image(url) if url.starts_with("data:image/png;base64,"))
    );
}

#[test]
fn drawingml_numeric_overflow_is_rejected_before_float_model_conversion() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let slide = part(fixture, "ppt/slides/slide1.xml");
    let invalid_fields=[
        ("border",slide.replace("w=\"19050\"","w=\"1e300\"")),
        ("padding",slide.replacen("<a:bodyPr/>","<a:bodyPr lIns=\"1e300\"/>",1)),
        ("crop",slide.replace("l=\"10000\"","l=\"1e300\"")),
        ("font",slide.replace("sz=\"2400\"","sz=\"1e300\"")),
        ("nonfinite",slide.replace("sz=\"2400\"","sz=\"NaN\"")),
        ("overflowingparse",slide.replace("sz=\"2400\"","sz=\"1e400\"")),
        ("gradient",slide.replace("<p:bgPr>","<p:bgPr><a:gradFill><a:lin ang=\"1e300\"/></a:gradFill>")),
        ("tablewidth",slide.replace("<p:spTree>","<p:spTree><p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w=\"1e300\"/></a:tblGrid></a:tbl></a:graphicData></a:graphic></p:graphicFrame>")),
        ("tableheight",slide.replace("<p:spTree>","<p:spTree><p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr h=\"1e300\"/></a:tbl></a:graphicData></a:graphic></p:graphicFrame>")),
    ];
    for (field, xml) in invalid_fields {
        let bytes = replace(fixture, "ppt/slides/slide1.xml", &xml);
        let error = syntra_export::pptx::pptx_to_presentation(&bytes).unwrap_err();
        assert!(error.contains("finite geometry budget"), "{field}: {error}");
    }
    // Inherited placeholders/themes go through the same bounded part reader.
    let layout = part(fixture, "ppt/slideLayouts/slideLayout1.xml")
        .replace("cx=\"10477500\"", "cx=\"1e300\"");
    assert_ne!(layout, part(fixture, "ppt/slideLayouts/slideLayout1.xml"));
    let bytes = replace(fixture, "ppt/slideLayouts/slideLayout1.xml", &layout);
    assert!(
        syntra_export::pptx::pptx_to_presentation(&bytes)
            .unwrap_err()
            .contains("finite geometry budget")
    );
}

#[test]
fn bounded_large_drawingml_numbers_still_make_a_reloadable_typed_model() {
    let fixture = include_bytes!("fixtures/external-rich.pptx");
    let slide=part(fixture,"ppt/slides/slide1.xml")
        .replace("w=\"19050\"","w=\"1000000000000\"")
        .replacen("<a:bodyPr/>","<a:bodyPr lIns=\"1000000000000\"/>",1)
        .replace("sz=\"2400\"","sz=\"1000000000000\"")
        .replace("<p:bgPr><a:solidFill><a:srgbClr val=\"F0F4F8\"/></a:solidFill></p:bgPr>","<p:bgPr><a:gradFill><a:gsLst><a:gs pos=\"0\"><a:srgbClr val=\"FFFFFF\"/></a:gs><a:gs pos=\"100000\"><a:srgbClr val=\"000000\"/></a:gs></a:gsLst><a:lin ang=\"1000000000000\"/></a:gradFill></p:bgPr>");
    let bytes = replace(fixture, "ppt/slides/slide1.xml", &slide);
    let model = syntra_export::pptx::pptx_to_presentation(&bytes).unwrap();
    let reparsed: Presentation =
        serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    let Background::Gradient(_, _, angle) = reparsed.slides[0].background else {
        panic!("gradient")
    };
    assert!(angle.is_finite() && (0.0..360.0).contains(&angle));
    let SlideElement::TextBox(text) = &reparsed.slides[0].elements[0] else {
        panic!("text")
    };
    assert!(text.padding.is_finite());
    assert_eq!(text.content[0].font_size, Some(10000.));
    // A table is also converted through actual grid/row numeric geometry.
    let slide=part(fixture,"ppt/slides/slide1.xml").replace("<p:spTree>","<p:spTree><p:graphicFrame><p:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"952500\" cy=\"952500\"/></p:xfrm><a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w=\"1000000000000\"/></a:tblGrid><a:tr h=\"1000000000000\"><a:tc><a:txBody><a:p><a:r><a:t>Large table cell</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    let model = syntra_export::pptx::pptx_to_presentation(&replace(
        fixture,
        "ppt/slides/slide1.xml",
        &slide,
    ))
    .unwrap();
    let model: Presentation =
        serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
    let SlideElement::Group(table) = &model.slides[0].elements[0] else {
        panic!("table")
    };
    let SlideElement::TextBox(cell) = &table.elements[0] else {
        panic!("cell")
    };
    assert!(cell.width.is_finite() && cell.height.is_finite());
}

#[test]
fn xlsx_float_metrics_reject_nonfinite_and_out_of_budget_values() {
    let fixture = include_bytes!("fixtures/external-rich.xlsx");
    let styles = part(fixture, "xl/styles.xml").replace("<sz val=\"16\"/>", "<sz val=\"1e300\"/>");
    assert!(
        syntra_export::xlsx::xlsx_to_workbook(&replace(fixture, "xl/styles.xml", &styles)).is_err()
    );
    let sheet = part(fixture, "xl/worksheets/sheet1.xml");
    for (from, to) in [
        ("width=\"20\"", "width=\"1e300\""),
        ("ht=\"30\"", "ht=\"NaN\""),
    ] {
        let bytes = replace(
            fixture,
            "xl/worksheets/sheet1.xml",
            &sheet.replace(from, to),
        );
        assert!(syntra_export::xlsx::xlsx_to_workbook(&bytes).is_err());
    }
}

#[test]
fn external_shared_string_rich_runs_keep_text_and_report_style_downgrade() {
    let fixture = include_bytes!("fixtures/external-shared-rich.xlsx");
    assert!(
        ZipArchive::new(Cursor::new(fixture))
            .unwrap()
            .by_name("customXml/syntra-workbook.json")
            .is_err()
    );
    let model = syntra_export::xlsx::xlsx_to_workbook(fixture).unwrap();
    assert!(
        matches!(&model.sheets[0].get_cell(1,0).unwrap().value,CellValue::Text(text) if text=="East")
    );
    assert!(
        matches!(&model.sheets[0].get_cell(3,0).unwrap().value,CellValue::Text(text) if text=="East")
    );
    assert!(
        model
            .metadata
            .import_warnings
            .join(" ")
            .contains("original per-run formatting")
    );
    // The reference count is two while uniqueCount and actual si are one;
    // shared references are normal and must not be mistaken for an oversized table.
    assert!(part(fixture, "xl/sharedStrings.xml").contains("count=\"2\" uniqueCount=\"1\""));
}

#[test]
fn tiny_external_xlsx_cannot_reserve_from_unbounded_shared_string_headers() {
    let fixture = include_bytes!("fixtures/external-shared-rich.xlsx");
    let table = part(fixture, "xl/sharedStrings.xml");
    for (from, to) in [
        ("uniqueCount=\"1\"", "uniqueCount=\"18446744073709551615\""),
        ("count=\"2\"", "count=\"18446744073709551615\""),
        (
            "uniqueCount=\"1\"",
            "uniqueCount=\"999999999999999999999999999999999999999999999999\"",
        ),
    ] {
        let bytes = replace(fixture, "xl/sharedStrings.xml", &table.replace(from, to));
        assert!(bytes.len() < 100_000);
        let outcome = std::panic::catch_unwind(|| syntra_export::xlsx::xlsx_to_workbook(&bytes));
        assert!(
            outcome.is_ok(),
            "untrusted count must not reach Calamine's reserve"
        );
        assert!(outcome.unwrap().unwrap_err().contains("budget"));
    }
    // Calamine reads this canonical path even if the package relationship is
    // absent. Its actual input must still be protected before construction.
    let links =
        part(fixture, "xl/_rels/workbook.xml.rels").replace("/sharedStrings", "/unknownStrings");
    let bytes = replace(fixture, "xl/_rels/workbook.xml.rels", &links);
    let bytes = replace(
        &bytes,
        "xl/sharedStrings.xml",
        &table.replace("uniqueCount=\"1\"", "uniqueCount=\"18446744073709551615\""),
    );
    assert!(
        syntra_export::xlsx::xlsx_to_workbook(&bytes)
            .unwrap_err()
            .contains("allocation budget")
    );
}

#[test]
fn actual_shared_string_nodes_are_bounded_even_when_headers_understate_them() {
    let fixture = include_bytes!("fixtures/external-shared-rich.xlsx");
    let table = format!(
        "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"0\" uniqueCount=\"0\">{}</sst>",
        "<si/>".repeat(syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES / 200 + 1)
    );
    let bytes = replace(fixture, "xl/sharedStrings.xml", &table);
    assert!(
        syntra_export::xlsx::xlsx_to_workbook(&bytes)
            .unwrap_err()
            .contains("actual content exceeds allocation budget")
    );
}

#[test]
fn standard_pptx_page_size_uses_model_pixels_for_landscape_portrait_and_fractional_pages() {
    for (width, height) in [
        (1024., 768.),
        (768., 1024.),
        (1920., 1080.),
        (1280.25, 720.5),
    ] {
        let mut presentation = Presentation::new("Page dimensions".into());
        presentation.metadata.slide_width = width;
        presentation.metadata.slide_height = height;
        let bytes = syntra_export::pptx::presentation_to_pptx(&presentation).unwrap();
        let xml = part(&bytes, "ppt/presentation.xml");
        let expected = format!(
            "<p:sldSz cx=\"{}\" cy=\"{}\"",
            (f64::from(width) * 9525.).round() as i64,
            (f64::from(height) * 9525.).round() as i64
        );
        assert!(xml.contains(&expected), "{xml}");
        assert!(xml.contains("<p:notesSz cx=\"6858000\" cy=\"9144000\"/>"));
        let standard_only = replace(&bytes, "unused", "");
        let imported = syntra_export::pptx::pptx_to_presentation(&standard_only).unwrap();
        assert!((imported.metadata.slide_width - width).abs() < 0.001);
        assert!((imported.metadata.slide_height - height).abs() < 0.001);
    }
}

#[test]
fn standard_pptx_export_rejects_invalid_page_size_instead_of_writing_null_or_saturated_geometry() {
    for (width, height) in [
        (f32::NAN, 720.),
        (1280., f32::INFINITY),
        (0., 720.),
        (-1., 720.),
        (100001., 720.),
    ] {
        let mut presentation = Presentation::new("Invalid page dimensions".into());
        presentation.metadata.slide_width = width;
        presentation.metadata.slide_height = height;
        assert!(
            syntra_export::pptx::presentation_to_pptx(&presentation)
                .unwrap_err()
                .contains("page dimensions")
        );
    }
}

#[test]
fn calc_reader_preserves_excel_column_widths_merges_and_short_dates() {
    let fixture = include_bytes!("fixtures/external-rich.xlsx");
    let workbook = replace(
        fixture,
        "xl/styles.xml",
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
      <fonts count="1"><font><sz val="11"/><name val="SimSun"/></font></fonts>
      <cellStyleXfs count="1"><xf fontId="0"/></cellStyleXfs>
      <cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs>
      <cellStyles count="1"><cellStyle name="Normal" builtinId="0" xfId="0"/></cellStyles></styleSheet>"#,
    );
    let workbook = replace(
        &workbook,
        "xl/worksheets/sheet1.xml",
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
      <dimension ref="A1:F4"/><sheetFormatPr defaultColWidth="9" defaultRowHeight="18"/>
      <cols><col min="1" max="1" width="17.125"/><col min="2" max="2" width="50.625"/>
      <col min="6" max="6" width="120"/></cols>
      <sheetData><row r="1" ht="48" customHeight="1"><c r="A1" t="inlineStr"><is><t>Title</t></is></c></row>
      <row r="2"><c r="A2" s="1"><v>46266</v></c><c r="C2"><v>42</v></c><c r="F2"><v>6</v></c></row></sheetData>
      <mergeCells count="2"><mergeCell ref="A1:C1"/><mergeCell ref="A2:A4"/></mergeCells></worksheet>"#,
    );
    let book = syntra_export::xlsx::xlsx_to_workbook(&workbook).unwrap();
    let content = serde_json::to_value(&book).unwrap();
    let html = syntra_export::pdf::document_to_reader_html(&content, "calc")
        .unwrap()
        .html;
    for expected in [
        "<col style=\"width:137px\">",
        "<col style=\"width:405px\">",
        "<col style=\"width:72px\">",
        "height:64px",
        "rowspan=\"1\" colspan=\"3\"",
        "rowspan=\"3\" colspan=\"1\" data-short-date",
        ">2026-09-01</td>",
    ] {
        assert!(html.contains(expected), "Missing {expected}");
    }
    let sheets = book.sheets.len();
    assert_eq!(html.matches("<section class=\"sheet\">").count(), sheets);
    let print = syntra_export::pdf::document_to_print_html(&content, "calc").unwrap();
    assert!(print.html.matches("<section class=\"sheet\">").count() > sheets);
}
