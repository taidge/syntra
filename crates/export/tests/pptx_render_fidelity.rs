use std::io::{Cursor, Read, Write};
use syntra_doc_engine::slides::{Presentation, SlideElement};

const FIXTURE: &[u8] = include_bytes!("fixtures/render-audit.pptx");

fn native_parts_only(bytes: &[u8]) -> Vec<u8> {
    let mut input = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..input.len() {
        let mut part = input.by_index(i).unwrap();
        let name = part.name().unwrap().into_owned();
        if name.starts_with("customXml/") {
            continue;
        }
        let mut content = Vec::new();
        part.read_to_end(&mut content).unwrap();
        output
            .start_file(&name, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&content).unwrap();
    }
    output.finish().unwrap().into_inner()
}
fn assert_native_content(model: &Presentation) {
    let table = model.slides[1]
        .elements
        .iter()
        .find_map(|e| {
            if let SlideElement::Table(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .expect("editable native table");
    assert_eq!(table.cells.len(), 10);
    let title = &table.cells[0];
    assert_eq!(title.column_span, 3);
    assert_eq!(title.row_span, 1);
    assert_eq!(title.text_layout.vertical_alignment, "center");
    assert_eq!(
        title.content[0].paragraph.as_ref().unwrap().alignment,
        "center"
    );
    assert_eq!(title.borders.top.as_ref().unwrap().width, 2.);
    assert_eq!(title.borders.top.as_ref().unwrap().color, "#527A9E");
    let charts: Vec<_> = model.slides[2]
        .elements
        .iter()
        .filter_map(|e| {
            if let SlideElement::Chart(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(charts.len(), 2);
    let bar = charts[0].data.presentation.as_ref().unwrap();
    assert_eq!(bar.title.as_deref(), Some("收入对比 Revenue"));
    assert!(bar.show_legend && bar.labels.show_value && bar.value_axis.major_gridlines);
    let pie = charts[1].data.presentation.as_ref().unwrap();
    assert!(pie.show_legend && pie.labels.show_percent);
    assert!(
        pie.labels.position.is_empty(),
        "an absent label position must remain automatic"
    );
    let value = serde_json::to_value(charts[0]).unwrap();
    let svg = syntra_doc_engine::chart_render::render_chart_svg(&value, 0., 0., 750., 460.);
    for label in [
        "收入对比 Revenue",
        "计划 Plan",
        "实际 Actual",
        ">70</text>",
        ">80</text>",
    ] {
        assert!(svg.contains(label), "missing chart semantic: {label}");
    }
    let value = serde_json::to_value(charts[1]).unwrap();
    let svg = syntra_doc_engine::chart_render::render_chart_svg(&value, 0., 0., 370., 360.);
    for label in ["50%", "30%", "20%", "研发", "交付", "支持"] {
        assert!(svg.contains(label), "missing pie semantic: {label}");
    }
    let cells =
        syntra_doc_engine::slides::table_render_cells(&serde_json::to_value(table).unwrap());
    assert_eq!(cells[0]["width"], 1120.);
    assert_eq!(cells[0]["height"], 95.);
    let body = model.slides[0]
        .elements
        .iter()
        .find_map(|e| {
            if let SlideElement::TextBox(t) = e {
                t.content.iter().find(|r| r.text.starts_with("这是一段"))
            } else {
                None
            }
        })
        .unwrap();
    assert!((body.paragraph.as_ref().unwrap().line_height.unwrap() - 1.35).abs() < 0.0001);
}

#[test]
fn independent_pptx_retains_table_and_chart_semantics() {
    let model = syntra_export::pptx::pptx_to_presentation(FIXTURE).unwrap();
    assert_eq!(model.metadata.pptx_import_revision, 3);
    assert_native_content(&model);
}

#[test]
fn native_pptx_roundtrip_does_not_depend_on_custom_model() {
    let model = syntra_export::pptx::pptx_to_presentation(FIXTURE).unwrap();
    let exported = syntra_export::pptx::presentation_to_pptx(&model).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(exported.as_slice())).unwrap();
    let mut pie_xml = String::new();
    zip.by_name("ppt/charts/chart2.xml")
        .unwrap()
        .read_to_string(&mut pie_xml)
        .unwrap();
    assert_eq!(
        pie_xml.matches("<c:dPt>").count(),
        3,
        "native pie points need explicit fills in standalone packages"
    );
    assert!(
        !pie_xml.contains("<c:dLblPos"),
        "do not invent outEnd for an automatic pie label position"
    );
    let reparsed =
        syntra_export::pptx::pptx_to_presentation(&native_parts_only(&exported)).unwrap();
    assert_native_content(&reparsed);
}

#[test]
fn gap_width_controls_native_and_svg_bar_geometry() {
    let mut model = syntra_export::pptx::pptx_to_presentation(FIXTURE).unwrap();
    let chart = model.slides[2]
        .elements
        .iter_mut()
        .find_map(|e| {
            if let SlideElement::Chart(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .unwrap();
    fn first_bar_width(chart: &syntra_doc_engine::slides::SlideChart) -> f64 {
        let svg = syntra_doc_engine::chart_render::render_chart_svg(
            &serde_json::to_value(chart).unwrap(),
            0.,
            0.,
            750.,
            460.,
        );
        let first = svg.split("<rect ").nth(1).unwrap();
        first
            .split("width=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }
    let default_width = first_bar_width(chart);
    chart.data.presentation.as_mut().unwrap().gap_width = Some(250.);
    let custom_width = first_bar_width(chart);
    assert!((default_width / custom_width - 4.5 / 3.5).abs() < 0.001);
    let exported = syntra_export::pptx::presentation_to_pptx(&model).unwrap();
    let reparsed =
        syntra_export::pptx::pptx_to_presentation(&native_parts_only(&exported)).unwrap();
    let chart = reparsed.slides[2]
        .elements
        .iter()
        .find_map(|e| {
            if let SlideElement::Chart(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        chart.data.presentation.as_ref().unwrap().gap_width,
        Some(250.)
    );
    let html = syntra_export::pdf::document_to_print_html(
        &serde_json::to_value(&reparsed).unwrap(),
        "slides",
    )
    .unwrap()
    .html;
    assert!(
        html.contains("font-family:'Microsoft YaHei',Arial"),
        "paragraph struts use the same resolved font as the source text"
    );
}

fn table_export_model() -> Presentation {
    let source = syntra_export::pptx::pptx_to_presentation(FIXTURE).unwrap();
    let table = source.slides[1]
        .elements
        .iter()
        .find_map(|e| {
            if let SlideElement::Table(t) = e {
                Some(t.clone())
            } else {
                None
            }
        })
        .unwrap();
    let mut model = Presentation::new("Table export budget".into());
    model.slides[0].elements = vec![SlideElement::Group(
        syntra_doc_engine::slides::ElementGroup {
            id: uuid::Uuid::new_v4(),
            elements: vec![SlideElement::Table(table)],
        },
    )];
    model
}
fn nested_table(model: &mut Presentation) -> &mut syntra_doc_engine::slides::SlideTable {
    let SlideElement::Group(group) = &mut model.slides[0].elements[0] else {
        panic!("group")
    };
    let SlideElement::Table(table) = &mut group.elements[0] else {
        panic!("table")
    };
    table
}

#[test]
fn enormous_sparse_table_is_rejected_before_dense_grid_expansion() {
    let mut model = table_export_model();
    let table = nested_table(&mut model);
    table.column_widths = vec![1.; 100_000];
    table.row_heights = vec![1.; 100_000];
    table.cells.truncate(1);
    table.cells[0].column_span = 1;
    assert!(
        serde_json::to_vec(&model).unwrap().len() < syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES,
        "sparse input fits the JSON byte budget despite a 10-billion-cell grid"
    );
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("grid budget")
    );
}

#[test]
fn table_owner_scan_and_span_geometry_have_explicit_budgets() {
    let mut model = table_export_model();
    let table = nested_table(&mut model);
    table.column_widths = vec![1.; 1000];
    table.row_heights = vec![1.; 100];
    let mut cell = table.cells[0].clone();
    cell.column_span = 1;
    table.cells = vec![cell; 100];
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("dense-work budget")
    );
    let mut model = table_export_model();
    nested_table(&mut model).cells[0].row_span = usize::MAX;
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("row/span")
    );
    let mut model = table_export_model();
    let table = nested_table(&mut model);
    table.cells.push(table.cells[0].clone());
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("overlap")
    );
    let mut model = table_export_model();
    nested_table(&mut model).column_widths[0] = f32::NAN;
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("finite and positive")
    );
    let mut model = table_export_model();
    nested_table(&mut model).cells[0].background_color = Some("x".repeat(1024 * 1024));
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("RGB color")
    );
}

#[test]
fn over_budget_table_import_reports_the_whole_table_instead_of_truncating_rows() {
    let mut input = zip::ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..input.len() {
        let mut part = input.by_index(i).unwrap();
        let name = part.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        if name == "ppt/slides/slide2.xml" {
            let xml = String::from_utf8(bytes).unwrap();
            let start = xml.find("<a:tr ").unwrap();
            let end = xml.rfind("</a:tr>").unwrap() + "</a:tr>".len();
            bytes = format!(
                "{}{}{}",
                &xml[..start],
                "<a:tr h=\"9525\"><a:tc/><a:tc/><a:tc/></a:tr>".repeat(1001),
                &xml[end..]
            )
            .into_bytes();
        }
        output
            .start_file(&name, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    let model =
        syntra_export::pptx::pptx_to_presentation(&output.finish().unwrap().into_inner()).unwrap();
    assert!(
        !model.slides[1]
            .elements
            .iter()
            .any(|e| matches!(e, SlideElement::Table(_))),
        "do not return a truncated native table"
    );
    assert!(
        model
            .metadata
            .import_warnings
            .iter()
            .any(|w| w.contains("grid budget") && w.contains("entire table"))
    );
}

#[test]
fn native_pie_rejects_large_point_expansion_inside_the_json_budget() {
    let mut model = syntra_export::pptx::pptx_to_presentation(FIXTURE).unwrap();
    let chart = model.slides[2]
        .elements
        .iter_mut()
        .find_map(|e| match e {
            SlideElement::Chart(chart)
                if matches!(chart.chart_type, syntra_doc_engine::slides::ChartType::Pie) =>
            {
                Some(chart)
            }
            _ => None,
        })
        .unwrap();
    chart.data.datasets[0].values = vec![1.; 100_001];
    chart.data.labels = vec!["A".into()];
    assert!(
        serde_json::to_vec(&model).unwrap().len() < syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES
    );
    assert!(
        syntra_export::pptx::presentation_to_pptx(&model)
            .unwrap_err()
            .contains("chart point budget")
    );
}
