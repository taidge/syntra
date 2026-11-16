use std::io::{Cursor, Read, Write};

use contrix_doc_engine::writer::{Alignment, Block, Paragraph, WriterDocument};
use contrix_export::docx::{docx_to_writer, writer_to_docx};
use zip::{
    ZipArchive,
    write::{SimpleFileOptions, ZipWriter},
};

const TYPOGRAPHY: &[u8] = include_bytes!("fixtures/writer-typography.docx");

fn paragraph(block: &Block) -> &Paragraph {
    match block {
        Block::Paragraph(paragraph) => paragraph,
        _ => panic!("expected the original paragraph to stay in place"),
    }
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut content = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    content
}

fn replace_parts(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut input = ZipArchive::new(Cursor::new(TYPOGRAPHY)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut file = input.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        if parts.iter().any(|(replacement, _)| *replacement == name) {
            continue;
        }
        output
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        std::io::copy(&mut file, &mut output).unwrap();
    }
    for (name, content) in parts {
        output
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        output.write_all(content.as_bytes()).unwrap();
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn docx_typography_defaults_layout_and_chinese_numbering_stay_in_document_order() {
    let imported = docx_to_writer(TYPOGRAPHY).unwrap();
    assert_eq!(imported.body.len(), 4);
    let title = paragraph(&imported.body[0]);
    assert_eq!(title.alignment, Alignment::Center);
    assert!(title.runs[0].bold);
    assert_eq!(title.runs[0].font_size, Some(16.0));
    assert_eq!(title.runs[0].font_family.as_deref(), Some("SimSun"));

    let body = paragraph(&imported.body[1]);
    assert_eq!(body.alignment, Alignment::Justify);
    assert_eq!(body.spacing.line_height, 1.5);
    assert_eq!(body.runs[0].font_size, Some(12.0));
    assert!(!body.runs[0].bold);
    let format = imported
        .professional
        .paragraph_formats
        .iter()
        .find(|format| format.block_id == body.id)
        .unwrap();
    assert_eq!(format.first_line_indent_pt, Some(24.0));
    assert_eq!(format.first_line_indent_chars, Some(2.0));

    let numbered = paragraph(&imported.body[2]);
    assert_eq!(numbered.runs[0].text, "特殊说明");
    let format = imported
        .professional
        .paragraph_formats
        .iter()
        .find(|format| format.block_id == numbered.id)
        .unwrap();
    assert_eq!(format.numbering_label.as_deref(), Some("二、"));
    let cancelled = paragraph(&imported.body[3]);
    assert_eq!(cancelled.runs[0].text, "1. 自愿原则");
    assert!(
        imported
            .professional
            .paragraph_formats
            .iter()
            .find(|format| format.block_id == cancelled.id)
            .unwrap()
            .numbering_label
            .is_none()
    );
}

#[test]
fn docx_style_inheritance_direct_false_character_styles_and_exact_layout_are_resolved() {
    let styles = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
      <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:eastAsia="SimSun"/><w:sz w:val="24"/></w:rPr></w:rPrDefault></w:docDefaults>
      <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:rPr><w:b/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Base"><w:basedOn w:val="Normal"/><w:pPr><w:jc w:val="right"/><w:spacing w:before="120" w:after="180"/></w:pPr><w:rPr><w:i/><w:sz w:val="28"/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Derived"><w:basedOn w:val="Base"/><w:pPr><w:ind w:left="180" w:right="240" w:firstLine="420"/><w:spacing w:line="400" w:lineRule="exact"/></w:pPr><w:rPr><w:rFonts w:eastAsia="FangSong"/></w:rPr></w:style>
      <w:style w:type="character" w:styleId="Emphasis"><w:rPr><w:u/><w:color w:val="123456"/></w:rPr></w:style>
    </w:styles>"#;
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
      <w:p><w:pPr><w:pStyle w:val="Derived"/></w:pPr>
      <w:r><w:t>Inherited</w:t></w:r>
      <w:r><w:rPr><w:rStyle w:val="Emphasis"/><w:b w:val="0"/><w:i w:val="false"/><w:sz w:val="36"/></w:rPr><w:t>Direct override</w:t></w:r></w:p>
    </w:body></w:document>"#;
    let imported = docx_to_writer(&replace_parts(&[
        ("word/styles.xml", styles),
        ("word/document.xml", document),
    ]))
    .unwrap();
    let paragraph = paragraph(&imported.body[0]);
    assert_eq!(paragraph.alignment, Alignment::Right);
    assert_eq!(paragraph.spacing.before, 6.0);
    assert_eq!(paragraph.spacing.after, 9.0);
    assert!(paragraph.runs[0].bold && paragraph.runs[0].italic);
    assert_eq!(paragraph.runs[0].font_size, Some(14.0));
    assert_eq!(paragraph.runs[0].font_family.as_deref(), Some("FangSong"));
    assert!(!paragraph.runs[1].bold && !paragraph.runs[1].italic);
    assert!(paragraph.runs[1].underline);
    assert_eq!(paragraph.runs[1].color.as_deref(), Some("#123456"));
    assert_eq!(paragraph.runs[1].font_size, Some(18.0));
    let format = &imported.professional.paragraph_formats[0];
    assert_eq!(format.left_indent_pt, Some(9.0));
    assert_eq!(format.right_indent_pt, Some(12.0));
    assert_eq!(format.first_line_indent_pt, Some(21.0));
    assert_eq!(format.line_height_pt, Some(20.0));

    let exported = writer_to_docx(&imported).unwrap();
    let xml = part(&exported, "word/document.xml");
    assert!(xml.contains("w:firstLine=\"420\""));
    assert!(xml.contains("w:left=\"180\""));
    assert!(xml.contains("w:right=\"240\""));
    assert!(xml.contains("w:line=\"400\" w:lineRule=\"exact\""));
    let again = docx_to_writer(&exported).unwrap();
    let format = &again.professional.paragraph_formats[0];
    assert_eq!(format.line_height_pt, Some(20.0));
    assert_eq!(format.first_line_indent_pt, Some(21.0));
}

#[test]
fn docx_theme_east_asia_font_and_continued_chinese_start_are_preserved() {
    let styles = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:eastAsia="SimSun"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:styleId="Normal" w:default="1"><w:rPr><w:rFonts w:eastAsiaTheme="minorEastAsia"/></w:rPr></w:style></w:styles>"#;
    let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:fontScheme name="Test"><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:font script="Hans" typeface="宋体"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
    let document = part(TYPOGRAPHY, "word/document.xml").replace("</w:body>", "<w:p><w:pPr><w:numPr><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>Continued</w:t></w:r></w:p></w:body>");
    let imported = docx_to_writer(&replace_parts(&[
        ("word/styles.xml", styles),
        ("word/theme/theme1.xml", theme),
        ("word/document.xml", &document),
    ]))
    .unwrap();
    assert_eq!(
        paragraph(&imported.body[1]).runs[0].font_family.as_deref(),
        Some("宋体")
    );
    let labels = imported
        .professional
        .paragraph_formats
        .iter()
        .filter_map(|format| format.numbering_label.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["二、", "三、"]);
}

#[test]
fn docx_export_writes_new_layout_and_visible_numbering_without_native_metadata() {
    let imported = docx_to_writer(TYPOGRAPHY).unwrap();
    let exported = writer_to_docx(&imported).unwrap();
    let xml = part(&exported, "word/document.xml");
    assert!(xml.contains("w:firstLineChars=\"200\""));
    assert!(xml.contains("w:firstLine=\"480\""));
    assert!(xml.contains("w:eastAsia=\"SimSun\""));
    let again = docx_to_writer(&exported).unwrap();
    let visible = paragraph(&again.body[2])
        .runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>();
    assert_eq!(visible, "二、特殊说明");
}

#[test]
fn writer_old_professional_layout_without_paragraph_formats_remains_readable() {
    let mut value = serde_json::to_value(WriterDocument::new("Old document".into())).unwrap();
    value["professional"]
        .as_object_mut()
        .unwrap()
        .remove("paragraph_formats");
    let document: WriterDocument = serde_json::from_value(value).unwrap();
    assert!(document.professional.paragraph_formats.is_empty());
}

#[test]
fn docx_document_grid_scales_auto_spacing_but_respects_overrides_and_table_cells() {
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
      <w:p><w:pPr><w:rPr><w:snapToGrid w:val="0"/></w:rPr></w:pPr><w:r><w:t>Grid default</w:t></w:r></w:p>
      <w:p><w:pPr><w:snapToGrid w:val="0"/></w:pPr><w:r><w:t>Grid disabled</w:t></w:r></w:p>
      <w:p><w:pPr><w:snapToGrid/><w:spacing w:line="400" w:lineRule="exact"/></w:pPr><w:r><w:t>Exact wins</w:t></w:r></w:p>
      <w:p><w:pPr><w:pStyle w:val="Heading1"/><w:snapToGrid/></w:pPr><w:r><w:t>Grid heading</w:t></w:r></w:p>
      <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Table excludes grid</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
      <w:p><w:r><w:t>After table</w:t></w:r></w:p>
      <w:sectPr><w:docGrid w:type="lines" w:linePitch="312" w:charSpace="0"/></w:sectPr>
    </w:body></w:document>"#;
    let imported = docx_to_writer(&replace_parts(&[("word/document.xml", document)])).unwrap();
    let format_for = |block_id| {
        imported
            .professional
            .paragraph_formats
            .iter()
            .find(|format| format.block_id == block_id)
            .unwrap()
    };
    let automatic = paragraph(&imported.body[0]);
    assert_eq!(automatic.spacing.line_height, 1.5);
    assert!((format_for(automatic.id).line_height_pt.unwrap() - 23.4).abs() < 0.0001);
    assert!(
        format_for(paragraph(&imported.body[1]).id)
            .line_height_pt
            .is_none()
    );
    assert_eq!(
        format_for(paragraph(&imported.body[2]).id).line_height_pt,
        Some(20.0)
    );
    let Block::Heading(heading) = &imported.body[3] else {
        panic!("expected heading");
    };
    assert!((format_for(heading.id).line_height_pt.unwrap() - 23.4).abs() < 0.0001);
    let Block::Table(table) = &imported.body[4] else {
        panic!("expected table");
    };
    assert!(
        format_for(paragraph(&table.rows[0].cells[0].blocks[0]).id)
            .line_height_pt
            .is_none()
    );
    assert!(
        (format_for(paragraph(&imported.body[5]).id)
            .line_height_pt
            .unwrap()
            - 23.4)
            .abs()
            < 0.0001
    );
}
