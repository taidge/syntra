use std::io::{Cursor, Read, Write};
use syntra_doc_engine::writer::{Block, CellVerticalAlignment};
use syntra_export::{
    docx::{docx_to_writer, writer_to_docx},
    pdf::document_to_print_html,
};

#[test]
fn cells_and_theme_fonts_survive_standard_office_roundtrip() {
    let source = include_bytes!("fixtures/render-audit.docx");
    let imported = docx_to_writer(source).unwrap();
    assert_eq!(imported.professional.language.as_deref(), Some("en-US"));
    assert_eq!(
        imported.professional.east_asia_language.as_deref(),
        Some("en-US")
    );
    let table = imported
        .body
        .iter()
        .find_map(|b| {
            if let Block::Table(t) = b {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    let layout = table.rows[0].cells[0].layout.as_ref().unwrap();
    assert_eq!(layout.padding, [Some(5.); 4]);
    assert!(matches!(
        layout.vertical_alignment,
        CellVerticalAlignment::Center
    ));
    let title = imported
        .body
        .iter()
        .find_map(|b| {
            if let Block::Paragraph(p) = b {
                Some(p)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        title
            .runs
            .iter()
            .all(|r| r.font_family.as_deref() == Some("ＭＳ ゴシック")),
        "themeFontLang=ja-JP and eastAsiaTheme override stale explicit SimHei"
    );
    let bytes = writer_to_docx(&imported).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains("<w:vAlign w:val=\"center\"/>"));
    assert!(xml.contains("<w:top w:w=\"100\" w:type=\"dxa\"/>"));
    let reloaded = docx_to_writer(&bytes).unwrap();
    assert_eq!(
        reloaded.professional.east_asia_language.as_deref(),
        Some("en-US")
    );
    let table2 = reloaded
        .body
        .iter()
        .find_map(|b| {
            if let Block::Table(t) = b {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        table2.rows[0].cells[0].layout.as_ref().unwrap().padding,
        layout.padding
    );
    let html = document_to_print_html(&serde_json::to_value(&imported).unwrap(), "writer").unwrap();
    assert!(
        html.html
            .contains("vertical-align:middle;padding:5pt 5pt 5pt 5pt")
    );
}

#[test]
fn table_margins_are_inherited_and_explicit_zero_overrides_one_side() {
    let mut input =
        zip::ZipArchive::new(Cursor::new(include_bytes!("fixtures/render-audit.docx"))).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut part = input.by_index(index).unwrap();
        let name = part.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        part.read_to_end(&mut bytes).unwrap();
        if name == "word/document.xml" {
            let mut xml = String::from_utf8(bytes).unwrap();
            while let Some(start) = xml.find("<w:tcMar>") {
                let end = start + xml[start..].find("</w:tcMar>").unwrap() + 10;
                xml.replace_range(start..end, "");
            }
            xml=xml.replacen("</w:tblPr>","<w:tblCellMar><w:top w:w=\"40\" w:type=\"dxa\"/><w:right w:w=\"80\" w:type=\"dxa\"/><w:bottom w:w=\"120\" w:type=\"dxa\"/><w:left w:w=\"160\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr>",1);
            xml = xml.replacen(
                "</w:tcPr>",
                "<w:tcMar><w:top w:w=\"0\" w:type=\"dxa\"/></w:tcMar></w:tcPr>",
                1,
            );
            bytes = xml.into_bytes();
        }
        output
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(&bytes).unwrap();
    }
    let imported = docx_to_writer(&output.finish().unwrap().into_inner()).unwrap();
    let table = imported
        .body
        .iter()
        .find_map(|b| {
            if let Block::Table(t) = b {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        table.rows[0].cells[0].layout.as_ref().unwrap().padding,
        [Some(0.), Some(4.), Some(6.), Some(8.)]
    );
    assert_eq!(
        table.rows[1].cells[0].layout.as_ref().unwrap().padding,
        [Some(2.), Some(4.), Some(6.), Some(8.)]
    );
}
