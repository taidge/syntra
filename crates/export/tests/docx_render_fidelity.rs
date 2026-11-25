use std::io::{Cursor, Read, Write};
use syntra_doc_engine::writer::{Alignment, Block, WriterDocument};
use syntra_export::{
    docx::{docx_to_writer, writer_to_docx},
    pdf::document_to_print_html,
};

fn fixture() -> WriterDocument {
    docx_to_writer(include_bytes!("fixtures/render-audit.docx")).unwrap()
}

fn check_layout(document: &WriterDocument) {
    let table = document
        .body
        .iter()
        .find_map(|block| match block {
            Block::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    assert_eq!(table.column_widths.len(), 3);
    assert!(table.column_widths.iter().all(|width| *width > 0.));
    assert_eq!(table.rows[0].cells.len(), 1);
    assert_eq!(table.rows[0].cells[0].col_span, 3);
    assert_eq!(
        table.rows[0].cells[0].background_color.as_deref(),
        Some("#1E3A5F")
    );
    assert_eq!(table.rows[2].cells[0].row_span, 2);
    assert_eq!(
        table.rows[3].cells.len(),
        2,
        "vertical continuation must not create a phantom cell"
    );
    assert!(
        document
            .body
            .iter()
            .any(|block| matches!(block, Block::PageBreak))
    );
    let image = document
        .body
        .iter()
        .find_map(|block| match block {
            Block::Image(image) => Some(image),
            _ => None,
        })
        .unwrap();
    assert_eq!(image.alignment, Alignment::Center);
    assert!((image.width.unwrap() - 460.8).abs() < 0.1);
    assert!(
        document
            .professional
            .header
            .contains("Office rendering audit")
    );
    assert!(document.professional.footer.contains("Synthetic fixture"));
    assert_eq!(
        document.professional.header_alignment,
        Some(Alignment::Left)
    );
    assert_eq!(
        document.professional.footer_alignment,
        Some(Alignment::Right)
    );
    assert!(document.professional.page_numbers);
    let list = document
        .body
        .iter()
        .find_map(|block| match block {
            Block::List(list) => Some(list),
            _ => None,
        })
        .unwrap();
    let layout = list.items[0]
        .layout
        .as_ref()
        .expect("list paragraph layout");
    assert_eq!(layout.spacing.after, 6.);
    assert!((layout.spacing.line_height - 1.2).abs() < 0.001);
    assert_eq!(layout.format.contextual_spacing, Some(true));
    assert_eq!(layout.format.left_indent_pt, Some(18.));
    assert_eq!(layout.format.first_line_indent_pt, Some(-18.));
    assert_eq!(layout.format.numbering_label.as_deref(), Some("1."));
    assert_eq!(layout.label_font_family.as_deref(), Some("Times New Roman"));
    assert_eq!(layout.label_font_size, Some(12.));
    assert!(document.professional.font_metrics.contains_key("simsun"));
    let body = document
        .body
        .iter()
        .find_map(|block| match block {
            Block::Paragraph(p) if p.runs.iter().any(|run| run.text.starts_with("本文用于")) => {
                Some(p)
            }
            _ => None,
        })
        .unwrap();
    let format = document
        .professional
        .paragraph_formats
        .iter()
        .find(|format| format.block_id == body.id);
    assert_eq!(
        syntra_doc_engine::writer::office_line_box(
            &body.runs,
            body.spacing,
            format,
            &document.professional.font_metrics
        )
        .unwrap()
        .line_height_pt,
        20.25
    );
    let mixed = document
        .body
        .iter()
        .find_map(|block| match block {
            Block::Paragraph(p) if p.runs.iter().any(|r| r.text.contains("Times New Roman")) => {
                Some(p)
            }
            _ => None,
        })
        .unwrap();
    assert!(
        mixed
            .runs
            .iter()
            .any(|run| run.text.contains("Times New Roman")
                && run.font_family.as_deref() == Some("Times New Roman"))
    );
    assert!(
        mixed.runs.iter().any(
            |run| run.text.contains("普通宋体") && run.font_family.as_deref() == Some("SimSun")
        )
    );
}

#[test]
fn independent_docx_retains_tables_page_breaks_pictures_fields_and_script_fonts() {
    check_layout(&fixture());
}

#[test]
fn imported_layout_survives_an_office_download_round_trip() {
    let bytes = writer_to_docx(&fixture()).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let marker_properties = xml
        .split("</w:pPr>")
        .find(|properties| properties.contains("<w:numPr>"))
        .unwrap()
        .rsplit("<w:pPr>")
        .next()
        .unwrap();
    assert!(
        marker_properties
            .contains("<w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\"/>")
    );
    assert!(marker_properties.contains("<w:sz w:val=\"24\"/>"));
    check_layout(&docx_to_writer(&bytes).unwrap());
}

#[test]
fn printable_html_contains_the_imported_layout_including_page_furniture() {
    let printable =
        document_to_print_html(&serde_json::to_value(fixture()).unwrap(), "writer").unwrap();
    assert!(printable.html.contains("colspan=\"3\""));
    assert!(printable.html.contains("rowspan=\"2\""));
    assert!(printable.html.contains("background:#1E3A5F"));
    assert!(
        printable
            .html
            .contains("<div class=\"writer-page-break\"></div>")
    );
    assert!(
        printable
            .html
            .contains("<figure style=\"text-align:center\"")
    );
    assert!(
        printable
            .html
            .contains("@top-left{content:'Office rendering audit")
    );
    assert!(printable.html.contains("@bottom-right{content:"));
    assert!(printable.html.contains("Synthetic fixture"));
    assert!(printable.html.contains("counter(page)"));
    assert!(printable.html.contains("Times New Roman"));
    assert!(printable.html.contains("padding-left:0;margin:0"));
    assert!(printable.html.contains("text-indent:-18pt"));
    assert!(
        printable
            .html
            .contains("display:inline-block;text-indent:0;width:18pt")
    );
    assert!(
        printable
            .html
            .contains(".writer-page-break+*{margin-top:0!important}")
    );
}

#[test]
fn older_native_list_items_without_layout_keep_their_default_behavior() {
    let item: syntra_doc_engine::writer::ListItem =
        serde_json::from_value(serde_json::json!({"runs":[{"text":"Old list"}],"children":[]}))
            .unwrap();
    assert!(item.layout.is_none());
}

#[test]
fn office_line_boxes_keep_superscript_and_subscript_run_semantics() {
    let mut document = fixture();
    document.body = vec![Block::Paragraph(syntra_doc_engine::writer::Paragraph {
        id: uuid::Uuid::new_v4(),
        alignment: Alignment::Left,
        indent_level: 0,
        spacing: Default::default(),
        runs: vec![
            syntra_doc_engine::writer::TextRun {
                text: "SUP".into(),
                font_family: Some("SimSun".into()),
                font_size: Some(12.),
                superscript: true,
                ..Default::default()
            },
            syntra_doc_engine::writer::TextRun {
                text: "SUB".into(),
                font_family: Some("SimSun".into()),
                font_size: Some(12.),
                subscript: true,
                ..Default::default()
            },
        ],
    })];
    let html = document_to_print_html(&serde_json::to_value(document).unwrap(), "writer")
        .unwrap()
        .html;
    for (text, alignment) in [("SUP", "super"), ("SUB", "sub")] {
        let end = html.find(&format!(">{text}</span>")).unwrap();
        let start = html[..end].rfind("<span ").unwrap();
        assert!(html[start..end].contains(&format!("vertical-align:{alignment}")));
        assert!(!html[start..end].contains("position:relative"));
    }
}

#[test]
fn a_page_break_inside_a_run_keeps_text_on_each_side() {
    let mut input =
        zip::ZipArchive::new(Cursor::new(include_bytes!("fixtures/render-audit.docx"))).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut file = input.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        if name == "word/document.xml" {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("<w:br w:type=\"page\"/>"));
            bytes = text
                .replace(
                    "<w:br w:type=\"page\"/>",
                    "<w:t>BeforeBreak</w:t><w:br w:type=\"page\"/><w:t>AfterBreak</w:t>",
                )
                .into_bytes();
        }
        output
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    let bytes = output.finish().unwrap().into_inner();
    let document = docx_to_writer(&bytes).unwrap();
    let index = document
        .body
        .iter()
        .position(|block| matches!(block, Block::PageBreak))
        .unwrap();
    let text = |block: &Block| match block {
        Block::Paragraph(p) => p
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        _ => String::new(),
    };
    assert_eq!(text(&document.body[index - 1]), "BeforeBreak");
    assert_eq!(text(&document.body[index + 1]), "AfterBreak");
}
