use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use quick_xml::{Reader, events::Event};
use syntra_doc_engine::writer::{
    Alignment, Block, DocumentNote, List, ListItem, Paragraph, Spacing, Table, TableCell, TableRow,
    TextRun, WriterDocument,
};
use syntra_export::docx::{docx_to_writer, writer_to_docx};
use uuid::Uuid;
use zip::{
    ZipArchive,
    write::{SimpleFileOptions, ZipWriter},
};

#[test]
fn functional_docx_footnotes_and_endnotes_preserve_xml_entities_without_sidecar() {
    let mut doc = WriterDocument::new("Notes".into());
    doc.body = vec![paragraph(vec![TextRun::plain("Body")])];
    let text = "A & B < C > D \"quoted\" '中文'";
    doc.professional.footnotes.push(DocumentNote {
        id: 1,
        reference_block: 0,
        text: text.into(),
    });
    doc.professional.endnotes.push(DocumentNote {
        id: 2,
        reference_block: 0,
        text: text.into(),
    });
    let bytes = without_native_parts(&writer_to_docx(&doc).unwrap());
    for name in ["word/footnotes.xml", "word/endnotes.xml"] {
        let xml = part(&bytes, name);
        for entity in ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"] {
            assert!(xml.contains(entity), "{name} must exercise {entity}");
        }
    }
    let imported = docx_to_writer(&bytes).unwrap();
    assert_eq!(imported.professional.footnotes[0].id, 1);
    assert_eq!(imported.professional.footnotes[0].text, text);
    assert_eq!(imported.professional.endnotes[0].id, 2);
    assert_eq!(imported.professional.endnotes[0].text, text);
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn without_native_parts(bytes: &[u8]) -> Vec<u8> {
    let mut input = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..input.len() {
        let mut file = input.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        if name.starts_with("customXml/") || name.contains("syntra") {
            continue;
        }
        output
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        std::io::copy(&mut file, &mut output).unwrap();
    }
    output.finish().unwrap().into_inner()
}

fn linked(text: &str, url: &str) -> TextRun {
    TextRun {
        link: Some(url.into()),
        bold: true,
        ..TextRun::plain(text)
    }
}

fn paragraph(runs: Vec<TextRun>) -> Block {
    Block::Paragraph(Paragraph {
        id: Uuid::new_v4(),
        runs,
        alignment: Default::default(),
        indent_level: 0,
        spacing: Default::default(),
    })
}

fn table(blocks: Vec<Block>) -> Block {
    Block::Table(Table {
        id: Uuid::new_v4(),
        column_widths: vec![120.0],
        rows: vec![TableRow {
            cells: vec![TableCell {
                blocks,
                col_span: 1,
                row_span: 1,
                background_color: None,
            }],
        }],
    })
}

fn runs(blocks: &[Block]) -> Vec<&TextRun> {
    fn visit_list<'a>(items: &'a [ListItem], found: &mut Vec<&'a TextRun>) {
        for item in items {
            found.extend(&item.runs);
            visit_list(&item.children, found);
        }
    }
    let mut found = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(p) => found.extend(&p.runs),
            Block::Heading(h) => found.extend(&h.runs),
            Block::List(list) => visit_list(&list.items, &mut found),
            Block::Table(t) => {
                for row in &t.rows {
                    for cell in &row.cells {
                        found.extend(runs(&cell.blocks));
                    }
                }
            }
            Block::Blockquote(q) => found.extend(runs(&q.blocks)),
            _ => {}
        }
    }
    found
}

#[test]
fn docx_standard_hyperlink_relationships_roundtrip_through_nested_content() {
    let url = "https://example.org/search?q=a&quote=\"yes\"";
    let mut doc = WriterDocument::new("Links & <中文>".into());
    doc.professional.header = "Header & <标题>".into();
    doc.professional.footer = "Footer & <页脚>".into();
    doc.body = vec![
        paragraph(vec![
            TextRun::plain("Before "),
            linked("web", url),
            TextRun::plain(" after"),
        ]),
        table(vec![
            paragraph(vec![linked("table", "http://example.org/table")]),
            table(vec![paragraph(vec![linked("nested", url)])]),
        ]),
        Block::List(List {
            id: Uuid::new_v4(),
            ordered: true,
            items: vec![ListItem {
                runs: vec![linked(
                    "mail",
                    "mailto:person@example.org?subject=hello&body=world",
                )],
                children: vec![ListItem {
                    runs: vec![linked("child", url)],
                    children: vec![],
                }],
            }],
        }),
        paragraph(vec![linked("unsafe stays text", "javascript:alert(1)")]),
    ];
    let bytes = writer_to_docx(&doc).unwrap();
    let xml = part(&bytes, "word/document.xml");
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    let mut targets = BTreeMap::new();
    let mut reader = Reader::from_str(&rels);
    loop {
        match reader.read_event().unwrap() {
            Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let attributes: BTreeMap<_, _> = e
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                            a.decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )
                            .unwrap()
                            .into_owned(),
                        )
                    })
                    .collect();
                if attributes["Type"].ends_with("/hyperlink") {
                    assert_eq!(attributes["TargetMode"], "External");
                    assert!(
                        targets
                            .insert(attributes["Id"].clone(), attributes["Target"].clone())
                            .is_none()
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(
        targets.len(),
        3,
        "standard relationships must carry each supported target"
    );
    assert!(targets.values().any(|value| value == url));
    assert!(rels.contains("&amp;quote=&quot;yes&quot;"));
    assert!(!rels.contains("javascript:"));
    assert_eq!(xml.matches("<w:hyperlink ").count(), 5);
    let imported = docx_to_writer(&without_native_parts(&bytes)).unwrap();
    assert_eq!(imported.title, "Links & <中文>");
    assert_eq!(imported.professional.header, "Header & <标题>");
    assert_eq!(imported.professional.footer, "Footer & <页脚>");
    let imported_runs = runs(&imported.body);
    for (text, expected) in [
        ("web", Some(url)),
        ("table", Some("http://example.org/table")),
        ("nested", Some(url)),
        ("child", Some(url)),
        (
            "mail",
            Some("mailto:person@example.org?subject=hello&body=world"),
        ),
        ("unsafe stays text", None),
    ] {
        let run = imported_runs
            .iter()
            .find(|run| run.text == text)
            .expect(text);
        assert_eq!(run.link.as_deref(), expected, "link for {text}");
        assert!(run.bold, "hyperlink must preserve existing run formatting");
    }
    assert_eq!(imported_runs.first().unwrap().link, None);
    assert_eq!(imported_runs.last().unwrap().text, "unsafe stays text");
}

fn external_package(document: &str, relationships: &str) -> Vec<u8> {
    let mut package = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, value) in [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="document" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", relationships),
    ] {
        package
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        package.write_all(value.as_bytes()).unwrap();
    }
    package.finish().unwrap().into_inner()
}

#[test]
fn paragraph_properties_and_text_controls_survive_standard_docx_roundtrip() {
    let mut doc = WriterDocument::new("Paragraph layout".into());
    doc.body = vec![
        Block::Paragraph(Paragraph {
            id: Uuid::new_v4(),
            runs: vec![linked("A\tB\nC", "https://example.org/layout")],
            alignment: Alignment::Center,
            indent_level: 2,
            spacing: Spacing {
                before: 6.0,
                after: 12.0,
                line_height: 1.5,
            },
        }),
        table(vec![Block::Paragraph(Paragraph {
            id: Uuid::new_v4(),
            runs: vec![],
            alignment: Alignment::Right,
            indent_level: 1,
            spacing: Spacing {
                before: 2.0,
                after: 3.0,
                line_height: 1.25,
            },
        })]),
    ];
    let bytes = writer_to_docx(&doc).unwrap();
    let xml = part(&bytes, "word/document.xml");
    assert!(xml.contains("<w:jc w:val=\"center\"/>"));
    assert!(xml.contains("<w:ind w:left=\"1440\"/>"));
    assert!(xml.contains("w:before=\"120\" w:after=\"240\" w:line=\"360\" w:lineRule=\"auto\""));
    assert!(xml.contains("<w:tab/>") && xml.contains("<w:br/>"));
    let imported = docx_to_writer(&without_native_parts(&bytes)).unwrap();
    let Block::Paragraph(p) = &imported.body[0] else {
        panic!("paragraph missing")
    };
    assert_eq!(p.alignment, Alignment::Center);
    assert_eq!(p.indent_level, 2);
    assert_eq!(
        (p.spacing.before, p.spacing.after, p.spacing.line_height),
        (6.0, 12.0, 1.5)
    );
    assert_eq!(p.runs[0].text, "A\tB\nC");
    assert_eq!(
        p.runs[0].link.as_deref(),
        Some("https://example.org/layout")
    );
    let Block::Table(t) = &imported.body[1] else {
        panic!("table missing")
    };
    let Block::Paragraph(empty) = &t.rows[0].cells[0].blocks[0] else {
        panic!("empty paragraph missing")
    };
    assert_eq!(empty.alignment, Alignment::Right);
    assert_eq!(empty.indent_level, 1);
    assert_eq!(
        (
            empty.spacing.before,
            empty.spacing.after,
            empty.spacing.line_height
        ),
        (2.0, 3.0, 1.25)
    );
}

#[test]
fn alternate_prefix_external_docx_keeps_paragraphs_hyperlinks_tabs_and_breaks() {
    let document = r#"<d:document xmlns:d="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:link="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><d:body><d:p><d:pPr><d:jc d:val="justify"/><d:ind d:left="2160"/><d:spacing d:before="180" d:after="60" d:line="300" d:lineRule="auto"/></d:pPr><d:hyperlink link:id="url"><d:r><d:t>A</d:t><d:tab/><d:t>B</d:t><d:br/><d:t>C</d:t></d:r></d:hyperlink></d:p></d:body></d:document>"#;
    let relationships = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="url" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.org/external" TargetMode="External"/></Relationships>"#;
    let imported = docx_to_writer(&external_package(document, relationships)).unwrap();
    let Some(Block::Paragraph(p)) = imported.body.first() else {
        panic!("valid alternate-prefix document text must not silently disappear")
    };
    assert_eq!(p.alignment, Alignment::Justify);
    assert_eq!(p.indent_level, 3);
    assert_eq!(
        (p.spacing.before, p.spacing.after, p.spacing.line_height),
        (9.0, 3.0, 1.25)
    );
    assert_eq!(p.runs[0].text, "A\tB\nC");
    assert_eq!(
        p.runs[0].link.as_deref(),
        Some("https://example.org/external")
    );
}

#[test]
fn explicit_off_run_properties_and_xml_entities_are_preserved() {
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:b w:val="0"/><w:i w:val="false"/><w:u w:val="none"/><w:strike w:val="off"/></w:rPr><w:t>A &amp; &lt;B&gt; &#x4e2d; &#25991; &amp;amp;<![CDATA[ <raw>]]></w:t></w:r></w:p></w:body></w:document>"#;
    let imported = docx_to_writer(&external_package(document, "<Relationships/>")).unwrap();
    let run = runs(&imported.body)[0];
    assert_eq!(run.text, "A & <B> 中 文 &amp; <raw>");
    assert!(!run.bold && !run.italic && !run.underline && !run.strikethrough);
}

#[test]
fn empty_paragraphs_and_lists_keep_their_positions_inside_nested_tables() {
    let mut doc = WriterDocument::new("Nested blocks".into());
    doc.body = vec![
        paragraph(vec![]),
        table(vec![
            paragraph(vec![]),
            Block::List(List {
                id: Uuid::new_v4(),
                ordered: false,
                items: vec![ListItem {
                    runs: vec![linked("list item", "https://example.org/list")],
                    children: vec![ListItem {
                        runs: vec![TextRun::plain("child")],
                        children: vec![],
                    }],
                }],
            }),
            table(vec![paragraph(vec![TextRun::plain("inner")])]),
        ]),
    ];
    let bytes = writer_to_docx(&doc).unwrap();
    assert!(part(&bytes, "word/document.xml").contains("</w:tbl><w:p/></w:tc>"));
    let imported = docx_to_writer(&without_native_parts(&bytes)).unwrap();
    let Block::Paragraph(empty) = &imported.body[0] else {
        panic!("empty paragraph lost")
    };
    assert!(empty.runs.is_empty());
    let Block::Table(outer) = &imported.body[1] else {
        panic!("table lost")
    };
    let blocks = &outer.rows[0].cells[0].blocks;
    assert!(matches!(&blocks[0], Block::Paragraph(p) if p.runs.is_empty()));
    let Block::List(list) = &blocks[1] else {
        panic!("list lost")
    };
    assert!(!list.ordered);
    assert_eq!(
        list.items[0].runs[0].link.as_deref(),
        Some("https://example.org/list")
    );
    assert_eq!(list.items[0].children[0].runs[0].text, "child");
    assert!(matches!(&blocks[2], Block::Table(_)));
    assert!(matches!(&blocks[3], Block::Paragraph(p) if p.runs.is_empty()));
}

#[test]
fn independent_docx_import_reads_external_relationships_and_keeps_unsafe_links_inert() {
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body><w:p>
      <w:hyperlink r:id="safe"><w:r><w:rPr><w:i/></w:rPr><w:t>safe first</w:t></w:r><w:r><w:t>safe second</w:t></w:r></w:hyperlink>
      <w:hyperlink r:id="script"><w:r><w:t>script</w:t></w:r></w:hyperlink>
      <w:hyperlink r:id="file"><w:r><w:t>file</w:t></w:r></w:hyperlink>
      <w:hyperlink r:id="internal"><w:r><w:t>internal</w:t></w:r></w:hyperlink>
      <w:hyperlink r:id="wrongtype"><w:r><w:t>wrongtype</w:t></w:r></w:hyperlink>
      <w:hyperlink r:id="missing"><w:r><w:t>missing</w:t></w:r></w:hyperlink>
      <w:r><w:t>outside</w:t></w:r>
    </w:p></w:body></w:document>"#;
    let relationships = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
      <Relationship Id="safe" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="HTTPS://example.org/?a=1&amp;b=2" TargetMode="External"/>
      <Relationship Id="script" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="javascript:alert(1)" TargetMode="External"/>
      <Relationship Id="file" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="file:///C:/private.txt" TargetMode="External"/>
      <Relationship Id="internal" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="other.xml"/>
      <Relationship Id="wrongtype" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://example.org/image" TargetMode="External"/>
    </Relationships>"#;
    let imported = docx_to_writer(&external_package(document, relationships)).unwrap();
    let imported_runs = runs(&imported.body);
    assert_eq!(imported_runs.len(), 8);
    for run in &imported_runs[..2] {
        assert_eq!(run.link.as_deref(), Some("HTTPS://example.org/?a=1&b=2"));
    }
    assert!(imported_runs[0].italic);
    assert!(imported_runs[2..].iter().all(|run| run.link.is_none()));
    assert_eq!(imported_runs.last().unwrap().text, "outside");
}
