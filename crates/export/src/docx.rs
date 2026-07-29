//! DOCX export for Writer documents.
//!
//! Converts a [`WriterDocument`] into a `.docx` byte vector (a ZIP archive
//! containing Open XML files).  The output is compatible with Microsoft Word,
//! LibreOffice, and other word processors that support the OOXML format.

use std::io::{Cursor, Read, Write};

use contrix_doc_engine::writer::{
    Block, Blockquote, CodeBlock, Heading, ImageBlock, List, ListItem, Paragraph, Table, TableCell,
    TableRow, TextRun, WriterDocument,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use uuid::Uuid;
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Convert a [`WriterDocument`] to a `.docx` byte vector (ZIP archive with Open XML).
pub fn writer_to_docx(doc: &WriterDocument) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#
    )
    .map_err(|e| e.to_string())?;

    // _rels/.rels
    zip.start_file("_rels/.rels", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
</Relationships>"#
    )
    .map_err(|e| e.to_string())?;

    // docProps/core.xml — carries the title
    zip.start_file("docProps/core.xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>{}</dc:title>
</cp:coreProperties>"#,
        escape_xml(&doc.title)
    )
    .map_err(|e| e.to_string())?;

    // word/_rels/document.xml.rels
    zip.start_file("word/_rels/document.xml.rels", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"#
    )
    .map_err(|e| e.to_string())?;

    // word/styles.xml — basic styles
    zip.start_file("word/styles.xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Normal" w:default="1"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="48"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:pPr><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:pPr><w:outlineLvl w:val="4"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading6"><w:name w:val="heading 6"/><w:pPr><w:outlineLvl w:val="5"/></w:pPr><w:rPr><w:b/><w:sz w:val="22"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/></w:style>
</w:styles>"#
    )
    .map_err(|e| e.to_string())?;

    // word/numbering.xml — bullet (numId 1) and decimal (numId 2) definitions,
    // each with two indent levels, so exported lists are real Word lists.
    zip.start_file("word/numbering.xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>
    <w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/><w:lvlText w:val="◦"/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>
  </w:abstractNum>
  <w:abstractNum w:abstractNumId="1">
    <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>
    <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%2."/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
  <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#
    )
    .map_err(|e| e.to_string())?;

    // word/document.xml — the actual content
    zip.start_file("word/document.xml", options)
        .map_err(|e| e.to_string())?;
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<w:body>"#,
    );

    // Convert each block
    for block in &doc.body {
        block_to_xml(block, &mut xml);
    }

    xml.push_str("</w:body></w:document>");
    write!(zip, "{}", xml).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

fn block_to_xml(block: &Block, xml: &mut String) {
    match block {
        Block::Paragraph(p) => paragraph_to_xml(p, xml),
        Block::Heading(h) => heading_to_xml(h, xml),
        Block::List(list) => list_to_xml(list, xml),
        Block::Table(table) => table_to_xml(table, xml),
        Block::CodeBlock(cb) => code_block_to_xml(cb, xml),
        Block::Blockquote(bq) => blockquote_to_xml(bq, xml),
        Block::HorizontalRule => {
            xml.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:color=\"auto\"/></w:pBdr></w:pPr></w:p>");
        }
        Block::PageBreak => {
            xml.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
        Block::Image(img) => image_to_xml(img, xml),
    }
}

fn paragraph_to_xml(p: &Paragraph, xml: &mut String) {
    xml.push_str("<w:p>");
    for run in &p.runs {
        run_to_xml(run, xml);
    }
    xml.push_str("</w:p>");
}

fn heading_to_xml(h: &Heading, xml: &mut String) {
    let style = match h.level {
        1 => "Heading1",
        2 => "Heading2",
        3 => "Heading3",
        4 => "Heading4",
        5 => "Heading5",
        _ => "Heading6",
    };
    xml.push_str(&format!(
        "<w:p><w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>",
        style
    ));
    for run in &h.runs {
        run_to_xml(run, xml);
    }
    xml.push_str("</w:p>");
}

fn list_to_xml(list: &List, xml: &mut String) {
    for item in &list.items {
        list_item_to_xml(item, list.ordered, xml);
    }
}

fn list_item_to_xml(item: &ListItem, ordered: bool, xml: &mut String) {
    // Real Word list: reference the numbering definition (numId 2 = decimal,
    // numId 1 = bullet) defined in numbering.xml, at the given indent level.
    let num_id = if ordered { 2 } else { 1 };

    list_item_paragraph(item, num_id, 0, xml);

    // Nested items render at indent level 1 of the same numbering definition.
    for nested in &item.children {
        list_item_paragraph(nested, num_id, 1, xml);
    }
}

fn list_item_paragraph(item: &ListItem, num_id: u32, ilvl: u32, xml: &mut String) {
    xml.push_str(&format!(
        "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>"
    ));
    for run in &item.runs {
        run_to_xml(run, xml);
    }
    xml.push_str("</w:p>");
}

fn table_to_xml(table: &Table, xml: &mut String) {
    xml.push_str("<w:tbl><w:tblPr><w:tblBorders>");
    xml.push_str("<w:top w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:left w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:right w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("</w:tblBorders></w:tblPr>");
    for row in &table.rows {
        xml.push_str("<w:tr>");
        for cell in &row.cells {
            xml.push_str("<w:tc>");
            if cell.blocks.is_empty() {
                // An empty cell still needs at least one paragraph.
                xml.push_str("<w:p/>");
            } else {
                for b in &cell.blocks {
                    block_to_xml(b, xml);
                }
            }
            xml.push_str("</w:tc>");
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl>");
}

fn code_block_to_xml(cb: &CodeBlock, xml: &mut String) {
    xml.push_str("<w:p><w:pPr><w:shd w:val=\"clear\" w:fill=\"F0F0F0\"/></w:pPr>");
    xml.push_str(&format!(
        "<w:r><w:rPr><w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
        escape_xml(&cb.code)
    ));
    xml.push_str("</w:p>");
}

fn blockquote_to_xml(bq: &Blockquote, xml: &mut String) {
    for b in &bq.blocks {
        block_to_xml(b, xml);
    }
}

fn image_to_xml(img: &ImageBlock, xml: &mut String) {
    // Images cannot be embedded inline without a full relationship + image part.
    // For now we emit a placeholder text run.
    let label = if img.alt_text.is_empty() {
        &img.url
    } else {
        &img.alt_text
    };
    xml.push_str(&format!(
        "<w:p><w:r><w:t>[Image: {}]</w:t></w:r></w:p>",
        escape_xml(label)
    ));
}

fn run_to_xml(run: &TextRun, xml: &mut String) {
    xml.push_str("<w:r>");

    // Run properties
    let has_props = run.bold
        || run.italic
        || run.underline
        || run.strikethrough
        || run.font_family.is_some()
        || run.font_size.is_some()
        || run.color.is_some();

    if has_props {
        xml.push_str("<w:rPr>");
        if run.bold {
            xml.push_str("<w:b/>");
        }
        if run.italic {
            xml.push_str("<w:i/>");
        }
        if run.underline {
            xml.push_str("<w:u w:val=\"single\"/>");
        }
        if run.strikethrough {
            xml.push_str("<w:strike/>");
        }
        if let Some(ref font) = run.font_family {
            xml.push_str(&format!(
                "<w:rFonts w:ascii=\"{}\" w:hAnsi=\"{}\"/>",
                escape_xml(font),
                escape_xml(font)
            ));
        }
        if let Some(size) = run.font_size {
            // OOXML sizes are in half-points
            xml.push_str(&format!("<w:sz w:val=\"{}\"/>", (size * 2.0) as u32));
        }
        if let Some(ref color) = run.color {
            let c = color.trim_start_matches('#');
            xml.push_str(&format!("<w:color w:val=\"{}\"/>", c));
        }
        xml.push_str("</w:rPr>");
    }

    xml.push_str(&format!(
        "<w:t xml:space=\"preserve\">{}</w:t>",
        escape_xml(&run.text)
    ));
    xml.push_str("</w:r>");
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
// DOCX import
// ---------------------------------------------------------------------------

/// Convert a `.docx` byte slice into a [`WriterDocument`].
pub fn docx_to_writer(bytes: &[u8]) -> Result<WriterDocument, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip open: {e}"))?;

    let title = read_core_title(&mut archive).unwrap_or_else(|| "Imported Document".to_string());

    let mut document_xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| format!("missing word/document.xml: {e}"))?
        .read_to_string(&mut document_xml)
        .map_err(|e| format!("read document.xml: {e}"))?;

    let blocks = parse_body(&document_xml)?;
    let mut doc = WriterDocument::new(title);
    doc.body = blocks;
    Ok(doc)
}

fn read_core_title<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Option<String> {
    let mut file = archive.by_name("docProps/core.xml").ok()?;
    let mut s = String::new();
    file.read_to_string(&mut s).ok()?;
    let mut reader = Reader::from_str(&s);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut in_title = false;
    let mut title = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"dc:title" => in_title = true,
            Ok(Event::End(e)) if e.name().as_ref() == b"dc:title" => break,
            Ok(Event::Text(t)) if in_title => {
                title.push_str(&t.unescape().ok()?);
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    if title.is_empty() { None } else { Some(title) }
}

fn parse_body(xml: &str) -> Result<Vec<Block>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut blocks = Vec::new();
    let mut in_body = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                let n = name.as_ref();
                if n == b"w:body" {
                    in_body = true;
                } else if in_body && n == b"w:p" {
                    blocks.push(parse_paragraph(&mut reader)?);
                } else if in_body && n == b"w:tbl" {
                    blocks.push(parse_table(&mut reader)?);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:body" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(blocks)
}

fn parse_paragraph(reader: &mut Reader<&[u8]>) -> Result<Block, String> {
    let mut buf = Vec::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut heading_level: Option<u8> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.name().as_ref().to_vec();
                if n == b"w:pPr" {
                    heading_level = parse_p_pr(reader)?;
                } else if n == b"w:r" {
                    runs.push(parse_run(reader)?);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:p" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse paragraph: {e}")),
            _ => {}
        }
        buf.clear();
    }
    if let Some(level) = heading_level {
        Ok(Block::Heading(Heading {
            id: Uuid::new_v4(),
            level,
            runs,
        }))
    } else {
        Ok(Block::Paragraph(Paragraph {
            id: Uuid::new_v4(),
            runs,
            alignment: Default::default(),
            indent_level: 0,
            spacing: Default::default(),
        }))
    }
}

fn parse_p_pr(reader: &mut Reader<&[u8]>) -> Result<Option<u8>, String> {
    let mut buf = Vec::new();
    let mut level: Option<u8> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"w:pStyle" {
                    if let Some(val) = attr_val(&e, b"w:val") {
                        if let Some(rest) = val.strip_prefix("Heading") {
                            if let Ok(n) = rest.parse::<u8>() {
                                level = Some(n.clamp(1, 6));
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:pPr" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse pPr: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(level)
}

fn parse_run(reader: &mut Reader<&[u8]>) -> Result<TextRun, String> {
    let mut buf = Vec::new();
    let mut run = TextRun::default();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.name().as_ref().to_vec();
                if n == b"w:rPr" {
                    apply_r_pr(reader, &mut run)?;
                } else if n == b"w:t" {
                    let text = read_text_content(reader, b"w:t")?;
                    run.text.push_str(&text);
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(e)) if e.name().as_ref() == b"w:r" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse run: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(run)
}

fn apply_r_pr(reader: &mut Reader<&[u8]>, run: &mut TextRun) -> Result<(), String> {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => match e.name().as_ref() {
                b"w:b" => run.bold = true,
                b"w:i" => run.italic = true,
                b"w:u" => run.underline = true,
                b"w:strike" => run.strikethrough = true,
                b"w:rFonts" => {
                    if let Some(v) = attr_val(&e, b"w:ascii") {
                        run.font_family = Some(v);
                    }
                }
                b"w:sz" => {
                    if let Some(v) = attr_val(&e, b"w:val") {
                        if let Ok(n) = v.parse::<f32>() {
                            run.font_size = Some(n / 2.0);
                        }
                    }
                }
                b"w:color" => {
                    if let Some(v) = attr_val(&e, b"w:val") {
                        run.color = Some(format!("#{}", v));
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.name().as_ref() == b"w:rPr" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse rPr: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

fn read_text_content(reader: &mut Reader<&[u8]>, end: &[u8]) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut out = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => out.push_str(&t.unescape().map_err(|e| e.to_string())?),
            Ok(Event::End(e)) if e.name().as_ref() == end => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml text: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn parse_table(reader: &mut Reader<&[u8]>) -> Result<Block, String> {
    let mut buf = Vec::new();
    let mut rows: Vec<TableRow> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"w:tr" {
                    rows.push(parse_row(reader)?);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:tbl" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse table: {e}")),
            _ => {}
        }
        buf.clear();
    }
    let column_widths = rows
        .first()
        .map(|r| vec![0.0; r.cells.len()])
        .unwrap_or_default();
    Ok(Block::Table(Table {
        id: Uuid::new_v4(),
        rows,
        column_widths,
    }))
}

fn parse_row(reader: &mut Reader<&[u8]>) -> Result<TableRow, String> {
    let mut buf = Vec::new();
    let mut cells: Vec<TableCell> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"w:tc" {
                    cells.push(parse_cell(reader)?);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:tr" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse row: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(TableRow { cells })
}

fn parse_cell(reader: &mut Reader<&[u8]>) -> Result<TableCell, String> {
    let mut buf = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.name().as_ref().to_vec();
                if n == b"w:p" {
                    blocks.push(parse_paragraph(reader)?);
                } else if n == b"w:tbl" {
                    blocks.push(parse_table(reader)?);
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"w:tc" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse cell: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(TableCell {
        blocks,
        col_span: 1,
        row_span: 1,
        background_color: None,
    })
}

fn attr_val(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.as_ref() == key {
            return std::str::from_utf8(&a.value).ok().map(|s| s.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use contrix_doc_engine::writer::{Paragraph, TextRun, WriterDocument};

    #[test]
    fn test_docx_generation() {
        let mut doc = WriterDocument::new("Test".into());
        doc.add_paragraph("Hello World");
        let bytes = writer_to_docx(&doc).unwrap();
        assert!(bytes.len() > 100);
        // Verify it starts with the ZIP magic bytes
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_docx_with_formatting() {
        let mut doc = WriterDocument::new("Formatted".into());
        doc.add_heading("Title", 1);
        doc.add_paragraph("Normal paragraph");
        let bytes = writer_to_docx(&doc).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_docx_bold_text() {
        let mut doc = WriterDocument::new("Bold Test".into());
        doc.body
            .push(Block::Paragraph(contrix_doc_engine::writer::Paragraph {
                id: uuid::Uuid::new_v4(),
                runs: vec![TextRun {
                    text: "Bold text".into(),
                    bold: true,
                    ..Default::default()
                }],
                alignment: Default::default(),
                indent_level: 0,
                spacing: Default::default(),
            }));
        let bytes = writer_to_docx(&doc).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_docx_contains_document_xml() {
        let mut doc = WriterDocument::new("Content Check".into());
        doc.add_paragraph("Some text content");
        let bytes = writer_to_docx(&doc).unwrap();

        // Read the ZIP and verify it contains the expected files
        let reader = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(reader).unwrap();
        let expected_files = [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/document.xml",
            "word/_rels/document.xml.rels",
            "word/styles.xml",
        ];
        for name in expected_files {
            assert!(
                archive.by_name(name).is_ok(),
                "Missing expected file: {}",
                name
            );
        }

        // Verify document.xml contains our text
        let mut doc_xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("word/document.xml").unwrap(),
            &mut doc_xml,
        )
        .unwrap();
        assert!(doc_xml.contains("Some text content"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<script>"), "&lt;script&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"hello\""), "&quot;hello&quot;");
    }

    #[test]
    fn test_docx_round_trip() {
        let mut doc = WriterDocument::new("Round Trip".into());
        doc.add_heading("Hello", 1);
        doc.add_paragraph("World text content");
        let bytes = writer_to_docx(&doc).unwrap();

        let imported = docx_to_writer(&bytes).unwrap();
        assert_eq!(imported.body.len(), doc.body.len());

        match &imported.body[0] {
            Block::Heading(h) => {
                assert_eq!(h.level, 1);
                assert_eq!(h.runs[0].text, "Hello");
            }
            _ => panic!("expected heading"),
        }
        match &imported.body[1] {
            Block::Paragraph(p) => {
                assert_eq!(p.runs[0].text, "World text content");
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn test_docx_import_from_export() {
        let mut doc = WriterDocument::new("Fixture".into());
        doc.add_paragraph("First paragraph");
        doc.add_paragraph("Second paragraph");
        let bytes = writer_to_docx(&doc).unwrap();

        let imported = docx_to_writer(&bytes).unwrap();
        assert_eq!(imported.body.len(), 2);
        let text = imported.to_plain_text();
        assert!(text.contains("First paragraph"));
        assert!(text.contains("Second paragraph"));
    }

    #[test]
    fn test_docx_bold_round_trip() {
        let mut doc = WriterDocument::new("Bold Round Trip".into());
        doc.body.push(Block::Paragraph(Paragraph {
            id: uuid::Uuid::new_v4(),
            runs: vec![TextRun {
                text: "Bold text".into(),
                bold: true,
                ..Default::default()
            }],
            alignment: Default::default(),
            indent_level: 0,
            spacing: Default::default(),
        }));
        let bytes = writer_to_docx(&doc).unwrap();
        let imported = docx_to_writer(&bytes).unwrap();

        match &imported.body[0] {
            Block::Paragraph(p) => {
                assert_eq!(p.runs[0].text, "Bold text");
                assert!(p.runs[0].bold);
            }
            _ => panic!("expected paragraph"),
        }
    }
}
