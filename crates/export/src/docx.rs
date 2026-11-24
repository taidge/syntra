//! DOCX export for Writer documents.
//!
//! Converts a [`WriterDocument`] into a `.docx` byte vector (a ZIP archive
//! containing Open XML files).  The output is compatible with Microsoft Word,
//! LibreOffice, and other word processors that support the OOXML format.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use syntra_doc_engine::writer::{
    Alignment, Block, Blockquote, CodeBlock, Heading, ImageBlock, List, ListItem, Paragraph,
    ParagraphFormat, Spacing, Table, TableCell, TableRow, TextRun, WriterDocument,
};
use uuid::Uuid;
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

const MAX_EMBEDDED_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 50 * 1024 * 1024;
const HYPERLINK_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

fn safe_hyperlink(url: &str) -> bool {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, target)) = url.split_once(':') else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("mailto") {
        !target.is_empty()
    } else {
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && target
                .strip_prefix("//")
                .is_some_and(|host| !host.is_empty())
    }
}

fn collect_hyperlinks(doc: &WriterDocument) -> BTreeMap<String, String> {
    fn visit_runs(runs: &[TextRun], links: &mut BTreeMap<String, String>) {
        for run in runs {
            if let Some(url) = run.link.as_ref().filter(|url| safe_hyperlink(url))
                && !links.contains_key(url)
            {
                let id = format!("rIdHyperlink{}", links.len() + 1);
                links.insert(url.clone(), id);
            }
        }
    }
    fn visit_items(items: &[ListItem], links: &mut BTreeMap<String, String>) {
        for item in items {
            visit_runs(&item.runs, links);
            visit_items(&item.children, links);
        }
    }
    fn visit(blocks: &[Block], links: &mut BTreeMap<String, String>) {
        for block in blocks {
            match block {
                Block::Paragraph(p) => visit_runs(&p.runs, links),
                Block::Heading(h) => visit_runs(&h.runs, links),
                Block::List(list) => visit_items(&list.items, links),
                Block::Table(table) => {
                    for row in &table.rows {
                        for cell in &row.cells {
                            visit(&cell.blocks, links);
                        }
                    }
                }
                Block::Blockquote(quote) => visit(&quote.blocks, links),
                _ => {}
            }
        }
    }
    let mut links = BTreeMap::new();
    visit(&doc.body, &mut links);
    links
}

struct EmbeddedImage {
    block_id: Uuid,
    relationship_id: String,
    file_name: String,
    bytes: Vec<u8>,
    drawing_id: u32,
}

fn image_data_from_url(url: &str) -> Option<(&'static str, Vec<u8>)> {
    let (metadata, encoded) = url.strip_prefix("data:")?.split_once(',')?;
    if !metadata.ends_with(";base64") {
        return None;
    }
    let mime = metadata.strip_suffix(";base64")?.to_ascii_lowercase();
    let extension = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        _ => return None,
    };
    let bytes = BASE64.decode(encoded).ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_EMBEDDED_IMAGE_BYTES).then_some((extension, bytes))
}

fn collect_embedded_images(doc: &WriterDocument) -> Vec<EmbeddedImage> {
    fn visit(blocks: &[Block], images: &mut Vec<(Uuid, &'static str, Vec<u8>)>) {
        for block in blocks {
            match block {
                Block::Image(image) => {
                    if let Some((extension, bytes)) = image_data_from_url(&image.url) {
                        images.push((image.id, extension, bytes));
                    }
                }
                Block::Table(table) => {
                    for row in &table.rows {
                        for cell in &row.cells {
                            visit(&cell.blocks, images);
                        }
                    }
                }
                Block::Blockquote(quote) => visit(&quote.blocks, images),
                _ => {}
            }
        }
    }

    let mut discovered = Vec::new();
    visit(&doc.body, &mut discovered);
    let mut total = 0usize;
    discovered
        .into_iter()
        .enumerate()
        .filter_map(|(index, (block_id, extension, bytes))| {
            total = total.saturating_add(bytes.len());
            (total <= MAX_TOTAL_IMAGE_BYTES).then(|| EmbeddedImage {
                block_id,
                relationship_id: format!("rId{}", index + 10),
                file_name: format!("image{}.{}", index + 1, extension),
                bytes,
                drawing_id: (index + 1) as u32,
            })
        })
        .collect()
}

/// Convert a [`WriterDocument`] to a `.docx` byte vector (ZIP archive with Open XML).
pub fn writer_to_docx(doc: &WriterDocument) -> Result<Vec<u8>, String> {
    validate_paragraph_layout(&doc.body)?;
    for format in &doc.professional.paragraph_formats {
        if format.line_height_pt.is_some_and(|value| value <= 0.0) {
            return Err("DOCX exact line height must be positive".into());
        }
        for value in [
            format.first_line_indent_pt,
            format.first_line_indent_chars,
            format.left_indent_pt,
            format.right_indent_pt,
            format.line_height_pt,
        ]
        .into_iter()
        .flatten()
        {
            if !value.is_finite() || value.abs() > 1_000_000.0 {
                return Err("Unsupported DOCX paragraph format".into());
            }
        }
    }
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();
    let embedded_images = collect_embedded_images(doc);
    let hyperlinks = collect_hyperlinks(doc);
    let image_lookup: HashMap<Uuid, &EmbeddedImage> = embedded_images
        .iter()
        .map(|image| (image.block_id, image))
        .collect();

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    let mut content_types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="jpg" ContentType="image/jpeg"/>
  <Default Extension="jpeg" ContentType="image/jpeg"/>
  <Default Extension="gif" ContentType="image/gif"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#,
    );
    if !doc.professional.header.is_empty() {
        content_types.push_str("\n  <Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>");
    }
    if !doc.professional.footer.is_empty() || doc.professional.page_numbers {
        content_types.push_str("\n  <Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>");
    }
    if !doc.professional.footnotes.is_empty() {
        content_types.push_str("\n  <Override PartName=\"/word/footnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/>");
    }
    if !doc.professional.endnotes.is_empty() {
        content_types.push_str("\n  <Override PartName=\"/word/endnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml\"/>");
    }
    content_types.push_str("\n</Types>");
    write!(zip, "{content_types}").map_err(|e| e.to_string())?;

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
    let mut document_relationships = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    if !doc.professional.header.is_empty() {
        document_relationships.push_str("<Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/>");
    }
    if !doc.professional.footer.is_empty() || doc.professional.page_numbers {
        document_relationships.push_str("<Relationship Id=\"rId5\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>");
    }
    if !doc.professional.footnotes.is_empty() {
        document_relationships.push_str("<Relationship Id=\"rId6\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes.xml\"/>");
    }
    if !doc.professional.endnotes.is_empty() {
        document_relationships.push_str("<Relationship Id=\"rId7\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes\" Target=\"endnotes.xml\"/>");
    }
    for image in &embedded_images {
        document_relationships.push_str(&format!(
            r#"<Relationship Id="{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{}"/>"#,
            image.relationship_id, image.file_name
        ));
    }

    for (url, id) in &hyperlinks {
        document_relationships.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{HYPERLINK_RELATIONSHIP}\" Target=\"{}\" TargetMode=\"External\"/>",
            escape_xml(id), escape_xml(url)
        ));
    }
    document_relationships.push_str("</Relationships>");
    write!(zip, "{document_relationships}").map_err(|e| e.to_string())?;

    if !doc.professional.header.is_empty() {
        zip.start_file("word/header1.xml", options)
            .map_err(|e| e.to_string())?;
        write!(zip, "{}", header_xml(&doc.professional.header)).map_err(|e| e.to_string())?;
    }
    if !doc.professional.footer.is_empty() || doc.professional.page_numbers {
        zip.start_file("word/footer1.xml", options)
            .map_err(|e| e.to_string())?;
        write!(
            zip,
            "{}",
            footer_xml(&doc.professional.footer, doc.professional.page_numbers)
        )
        .map_err(|e| e.to_string())?;
    }
    if !doc.professional.footnotes.is_empty() {
        zip.start_file("word/footnotes.xml", options)
            .map_err(|e| e.to_string())?;
        write!(zip, "{}", notes_xml(&doc.professional.footnotes, false))
            .map_err(|e| e.to_string())?;
    }
    if !doc.professional.endnotes.is_empty() {
        zip.start_file("word/endnotes.xml", options)
            .map_err(|e| e.to_string())?;
        write!(zip, "{}", notes_xml(&doc.professional.endnotes, true))
            .map_err(|e| e.to_string())?;
    }

    for image in &embedded_images {
        zip.start_file(format!("word/media/{}", image.file_name), options)
            .map_err(|e| e.to_string())?;
        zip.write_all(&image.bytes).map_err(|e| e.to_string())?;
    }

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

    // word/numbering.xml — bullet (numId 1) and decimal (numId 2) definitions.
    // OOXML supports levels 0..=8; define all of them so deeply nested lists
    // never reference a missing numbering level.
    zip.start_file("word/numbering.xml", options)
        .map_err(|e| e.to_string())?;
    write!(zip, "{}", numbering_xml()).map_err(|e| e.to_string())?;

    // word/document.xml — the actual content
    zip.start_file("word/document.xml", options)
        .map_err(|e| e.to_string())?;
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
<w:body>"#,
    );

    if doc.professional.table_of_contents {
        xml.push_str("<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o &quot;1-3&quot; \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>Update field to generate table of contents</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>");
    }

    // Convert each block and place note/merge/revision markers at their
    // declared block boundary.
    let paragraph_formats = doc
        .professional
        .paragraph_formats
        .iter()
        .map(|format| (format.block_id, format))
        .collect::<HashMap<_, _>>();
    for (block_index, block) in doc.body.iter().enumerate() {
        block_to_xml(
            block,
            &image_lookup,
            &hyperlinks,
            &paragraph_formats,
            &mut xml,
        );
        append_professional_markers(doc, block_index, &mut xml);
    }

    if !doc.professional.bibliography.is_empty() {
        xml.push_str("<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Bibliography</w:t></w:r></w:p>");
        for entry in &doc.professional.bibliography {
            let year = entry
                .year
                .map(|year| format!(" ({year})"))
                .unwrap_or_default();
            xml.push_str(&format!(
                "<w:p><w:r><w:t>{}. {}{}</w:t></w:r></w:p>",
                escape_xml(&entry.author),
                escape_xml(&entry.title),
                year
            ));
        }
    }

    xml.push_str(&section_properties_xml(doc));

    xml.push_str("</w:body></w:document>");
    write!(zip, "{}", xml).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

fn header_xml(text: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:hdr>"#,
        escape_xml(text)
    )
}

fn footer_xml(text: &str, page_numbers: bool) -> String {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
        escape_xml(text)
    );
    if page_numbers {
        xml.push_str("<w:r><w:tab/><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>");
    }
    xml.push_str("</w:p></w:ftr>");
    xml
}

fn notes_xml(notes: &[syntra_doc_engine::writer::DocumentNote], endnotes: bool) -> String {
    let (root, item) = if endnotes {
        ("endnotes", "endnote")
    } else {
        ("footnotes", "footnote")
    };
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:{root} xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
    );
    for note in notes {
        xml.push_str(&format!(
            "<w:{item} w:id=\"{}\"><w:p><w:r><w:{}Reference/><w:t xml:space=\"preserve\"> {}</w:t></w:r></w:p></w:{item}>",
            note.id,
            if endnotes { "endnote" } else { "footnote" },
            escape_xml(&note.text)
        ));
    }
    xml.push_str(&format!("</w:{root}>"));
    xml
}

fn append_professional_markers(doc: &WriterDocument, block_index: usize, xml: &mut String) {
    for note in &doc.professional.footnotes {
        if note.reference_block == block_index {
            xml.push_str(&format!(
                "<w:p><w:r><w:footnoteReference w:id=\"{}\"/></w:r></w:p>",
                note.id
            ));
        }
    }
    for note in &doc.professional.endnotes {
        if note.reference_block == block_index {
            xml.push_str(&format!(
                "<w:p><w:r><w:endnoteReference w:id=\"{}\"/></w:r></w:p>",
                note.id
            ));
        }
    }
    for field in &doc.professional.mail_merge_fields {
        if field.block_index == block_index {
            xml.push_str(&format!(
                "<w:p><w:fldSimple w:instr=\" MERGEFIELD {} \"><w:r><w:t>{}</w:t></w:r></w:fldSimple></w:p>",
                escape_xml(&field.name),
                escape_xml(field.default_value.as_deref().unwrap_or(&field.name))
            ));
        }
    }
    for revision in &doc.professional.revisions {
        if revision.block_index == block_index {
            let tag = match revision.kind {
                syntra_doc_engine::writer::RevisionKind::Insert => "ins",
                syntra_doc_engine::writer::RevisionKind::Delete => "del",
            };
            let text_tag = if tag == "del" { "delText" } else { "t" };
            xml.push_str(&format!(
                "<w:p><w:{tag} w:id=\"{}\" w:author=\"{}\" w:date=\"{}\"><w:r><w:{text_tag}>{}</w:{text_tag}></w:r></w:{tag}></w:p>",
                revision.id.as_u128() & u32::MAX as u128,
                escape_xml(&revision.author),
                revision.timestamp.to_rfc3339(),
                escape_xml(&revision.text)
            ));
        }
    }
    for section in &doc.professional.sections {
        if section.start_block == block_index + 1 {
            xml.push_str("<w:p><w:pPr>");
            xml.push_str(&section_properties_for(
                &section.page_settings,
                section.header.as_deref(),
                section.footer.as_deref(),
                doc,
            ));
            xml.push_str("</w:pPr></w:p>");
        }
    }
}

fn section_properties_xml(doc: &WriterDocument) -> String {
    section_properties_for(&doc.page_settings, None, None, doc)
}

fn section_properties_for(
    settings: &syntra_doc_engine::writer::PageSettings,
    section_header: Option<&str>,
    section_footer: Option<&str>,
    doc: &WriterDocument,
) -> String {
    let twips = |millimetres: f32| (millimetres.max(0.0) * 56.692_913) as u32;
    let mut xml = String::from("<w:sectPr>");
    if !doc.professional.header.is_empty() || section_header.is_some_and(|value| !value.is_empty())
    {
        xml.push_str("<w:headerReference w:type=\"default\" r:id=\"rId4\"/>");
    }
    if !doc.professional.footer.is_empty()
        || doc.professional.page_numbers
        || section_footer.is_some_and(|value| !value.is_empty())
    {
        xml.push_str("<w:footerReference w:type=\"default\" r:id=\"rId5\"/>");
    }
    xml.push_str(&format!(
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{} /><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/>",
        twips(settings.width_mm),
        twips(settings.height_mm),
        if matches!(settings.orientation, syntra_doc_engine::writer::Orientation::Landscape) {
            " w:orient=\"landscape\""
        } else {
            ""
        },
        twips(settings.margin_top),
        twips(settings.margin_right),
        twips(settings.margin_bottom),
        twips(settings.margin_left),
    ));
    if doc.professional.different_first_page {
        xml.push_str("<w:titlePg/>");
    }
    xml.push_str("</w:sectPr>");
    xml
}

fn numbering_xml() -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">"#,
    );
    let bullet_symbols = ["•", "◦", "▪"];
    for level in 0..=8 {
        let left = (level + 1) * 720;
        let symbol = bullet_symbols[level as usize % bullet_symbols.len()];
        xml.push_str(&format!(
            r#"<w:lvl w:ilvl="{level}"><w:numFmt w:val="bullet"/><w:lvlText w:val="{symbol}"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#,
        ));
    }
    xml.push_str(r#"</w:abstractNum><w:abstractNum w:abstractNumId="1">"#);
    for level in 0..=8 {
        let left = (level + 1) * 720;
        let number = level + 1;
        xml.push_str(&format!(
            r#"<w:lvl w:ilvl="{level}"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%{number}."/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#,
        ));
    }
    xml.push_str(
        r#"</w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
  <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#,
    );
    xml
}

fn block_to_xml(
    block: &Block,
    images: &HashMap<Uuid, &EmbeddedImage>,
    links: &BTreeMap<String, String>,
    formats: &HashMap<Uuid, &ParagraphFormat>,
    xml: &mut String,
) {
    match block {
        Block::Paragraph(p) => paragraph_to_xml(p, links, formats.get(&p.id).copied(), xml),
        Block::Heading(h) => heading_to_xml(h, links, formats.get(&h.id).copied(), xml),
        Block::List(list) => list_to_xml(list, links, xml),
        Block::Table(table) => table_to_xml(table, images, links, formats, xml),
        Block::CodeBlock(cb) => code_block_to_xml(cb, xml),
        Block::Blockquote(bq) => blockquote_to_xml(bq, images, links, formats, xml),
        Block::HorizontalRule => {
            xml.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:color=\"auto\"/></w:pBdr></w:pPr></w:p>");
        }
        Block::PageBreak => {
            xml.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
        Block::Image(img) => image_to_xml(img, images.get(&img.id).copied(), xml),
    }
}

fn paragraph_properties_xml(
    alignment: Alignment,
    indent_level: u8,
    spacing: Spacing,
    format: Option<&ParagraphFormat>,
) -> String {
    let alignment = match format
        .and_then(|format| format.alignment)
        .unwrap_or(alignment)
    {
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::Justify => "both",
    };
    let mut ind = format!(
        " w:left=\"{}\"",
        (format
            .and_then(|f| f.left_indent_pt)
            .unwrap_or(f32::from(indent_level) * 36.0)
            * 20.0)
            .round() as i32
    );
    if let Some(value) = format.and_then(|f| f.right_indent_pt) {
        ind.push_str(&format!(" w:right=\"{}\"", (value * 20.0).round() as i32));
    }
    if let Some(value) = format.and_then(|f| f.first_line_indent_pt) {
        ind.push_str(&format!(
            " w:{}=\"{}\"",
            if value < 0.0 { "hanging" } else { "firstLine" },
            (value.abs() * 20.0).round() as u32
        ));
    }
    if let Some(value) = format.and_then(|f| f.first_line_indent_chars) {
        ind.push_str(&format!(
            " w:{}=\"{}\"",
            if value < 0.0 {
                "hangingChars"
            } else {
                "firstLineChars"
            },
            (value.abs() * 100.0).round() as u32
        ));
    }
    let (line, rule) = match format.and_then(|f| f.line_height_pt) {
        Some(value) => ((value * 20.0).round() as u32, "exact"),
        None => ((spacing.line_height * 240.0).round() as u32, "auto"),
    };
    format!(
        "<w:jc w:val=\"{alignment}\"/><w:ind{ind}/><w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{line}\" w:lineRule=\"{rule}\"/>",
        (spacing.before * 20.0).round() as u32,
        (spacing.after * 20.0).round() as u32
    )
}

fn write_numbering_label(
    format: Option<&ParagraphFormat>,
    runs: &[TextRun],
    links: &BTreeMap<String, String>,
    xml: &mut String,
) {
    if let Some(label) = format.and_then(|format| format.numbering_label.as_ref()) {
        let mut run = runs.first().cloned().unwrap_or_default();
        run.text = label.clone();
        run.link = None;
        run_to_xml(&run, links, xml);
    }
}

fn paragraph_to_xml(
    p: &Paragraph,
    links: &BTreeMap<String, String>,
    format: Option<&ParagraphFormat>,
    xml: &mut String,
) {
    xml.push_str("<w:p><w:pPr>");
    xml.push_str(&paragraph_properties_xml(
        p.alignment,
        p.indent_level,
        p.spacing,
        format,
    ));
    xml.push_str("</w:pPr>");
    write_numbering_label(format, &p.runs, links, xml);
    for run in &p.runs {
        run_to_xml(run, links, xml);
    }
    xml.push_str("</w:p>");
}

fn validate_paragraph_layout(blocks: &[Block]) -> Result<(), String> {
    for block in blocks {
        match block {
            Block::Paragraph(p) => {
                for (value, scale) in [
                    (p.spacing.before, 20.0),
                    (p.spacing.after, 20.0),
                    (p.spacing.line_height, 240.0),
                ] {
                    if !value.is_finite()
                        || value < 0.0
                        || f64::from(value) * scale > f64::from(u32::MAX)
                    {
                        return Err("Unsupported DOCX paragraph spacing".into());
                    }
                }
                if (f64::from(p.spacing.line_height) * 240.0).round() < 1.0 {
                    return Err("DOCX line-height multiplier must be positive".into());
                }
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        validate_paragraph_layout(&cell.blocks)?;
                    }
                }
            }
            Block::Blockquote(quote) => validate_paragraph_layout(&quote.blocks)?,
            _ => {}
        }
    }
    Ok(())
}

fn heading_to_xml(
    h: &Heading,
    links: &BTreeMap<String, String>,
    format: Option<&ParagraphFormat>,
    xml: &mut String,
) {
    let style = match h.level {
        1 => "Heading1",
        2 => "Heading2",
        3 => "Heading3",
        4 => "Heading4",
        5 => "Heading5",
        _ => "Heading6",
    };
    xml.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"{}\"/>", style));
    if format.is_some() {
        xml.push_str(&paragraph_properties_xml(
            Alignment::Left,
            0,
            Spacing::default(),
            format,
        ));
    }
    xml.push_str("</w:pPr>");
    write_numbering_label(format, &h.runs, links, xml);
    for run in &h.runs {
        run_to_xml(run, links, xml);
    }
    xml.push_str("</w:p>");
}

fn list_to_xml(list: &List, links: &BTreeMap<String, String>, xml: &mut String) {
    for item in &list.items {
        list_item_to_xml(item, list.ordered, 0, links, xml);
    }
}

fn list_item_to_xml(
    item: &ListItem,
    ordered: bool,
    level: u32,
    links: &BTreeMap<String, String>,
    xml: &mut String,
) {
    // Real Word list: reference the numbering definition (numId 2 = decimal,
    // numId 1 = bullet) defined in numbering.xml, at the given indent level.
    let num_id = if ordered { 2 } else { 1 };

    // A numbering definition has at most nine levels (0..=8). Keep deeper
    // application nesting valid by flattening it into the deepest OOXML level.
    list_item_paragraph(item, num_id, level.min(8), links, xml);

    for nested in &item.children {
        list_item_to_xml(nested, ordered, level + 1, links, xml);
    }
}

fn list_item_paragraph(
    item: &ListItem,
    num_id: u32,
    ilvl: u32,
    links: &BTreeMap<String, String>,
    xml: &mut String,
) {
    xml.push_str(&format!(
        "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"{ilvl}\"/><w:numId w:val=\"{num_id}\"/></w:numPr></w:pPr>"
    ));
    for run in &item.runs {
        run_to_xml(run, links, xml);
    }
    xml.push_str("</w:p>");
}

fn table_to_xml(
    table: &Table,
    images: &HashMap<Uuid, &EmbeddedImage>,
    links: &BTreeMap<String, String>,
    formats: &HashMap<Uuid, &ParagraphFormat>,
    xml: &mut String,
) {
    xml.push_str("<w:tbl><w:tblPr><w:tblBorders>");
    xml.push_str("<w:top w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:left w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:right w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("<w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"000000\"/>");
    xml.push_str("</w:tblBorders></w:tblPr>");
    // Standard readers use tblGrid to address cells, even for auto-sized tables.
    let columns = table
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0)
        .max(table.column_widths.len());
    xml.push_str("<w:tblGrid>");
    for column in 0..columns {
        match table
            .column_widths
            .get(column)
            .filter(|w| w.is_finite() && **w > 0.0)
        {
            Some(width) => xml.push_str(&format!(
                r#"<w:gridCol w:w="{}"/>"#,
                (width * 15.).round() as u32
            )),
            None => xml.push_str("<w:gridCol/>"),
        }
    }
    xml.push_str("</w:tblGrid>");
    for row in &table.rows {
        xml.push_str("<w:tr>");
        for cell in &row.cells {
            xml.push_str("<w:tc>");
            if cell.blocks.is_empty() {
                // An empty cell still needs at least one paragraph.
                xml.push_str("<w:p/>");
            } else {
                for b in &cell.blocks {
                    block_to_xml(b, images, links, formats, xml);
                }
                if matches!(cell.blocks.last(), Some(Block::Table(_))) {
                    // Word requires a paragraph after the final nested table.
                    xml.push_str("<w:p/>");
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

fn blockquote_to_xml(
    bq: &Blockquote,
    images: &HashMap<Uuid, &EmbeddedImage>,
    links: &BTreeMap<String, String>,
    formats: &HashMap<Uuid, &ParagraphFormat>,
    xml: &mut String,
) {
    for b in &bq.blocks {
        block_to_xml(b, images, links, formats, xml);
    }
}

fn image_to_xml(img: &ImageBlock, embedded: Option<&EmbeddedImage>, xml: &mut String) {
    if let Some(embedded) = embedded {
        let width_emu = (img.width.unwrap_or(320.0).clamp(1.0, 4096.0) * 9525.0) as i64;
        let height_emu = (img.height.unwrap_or(240.0).clamp(1.0, 4096.0) * 9525.0) as i64;
        let alt = escape_xml(&img.alt_text);
        xml.push_str(&format!(
            r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{width_emu}" cy="{height_emu}"/><wp:docPr id="{}" name="{}" descr="{alt}"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="{}" descr="{alt}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{width_emu}" cy="{height_emu}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            embedded.drawing_id,
            escape_xml(&embedded.file_name),
            escape_xml(&embedded.file_name),
            embedded.relationship_id,
        ));
        return;
    }

    // Remote/blob URLs have no bytes available to this synchronous exporter.
    // Preserve a visible, escaped fallback rather than silently dropping them.
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

fn run_to_xml(run: &TextRun, links: &BTreeMap<String, String>, xml: &mut String) {
    let hyperlink = run.link.as_ref().and_then(|url| links.get(url));
    if let Some(id) = hyperlink {
        xml.push_str(&format!("<w:hyperlink r:id=\"{}\">", escape_xml(id)));
    }
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
                "<w:rFonts w:ascii=\"{}\" w:hAnsi=\"{}\" w:eastAsia=\"{}\"/>",
                escape_xml(font),
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

    let normalized = run.text.replace("\r\n", "\n").replace('\r', "\n");
    for (line_index, line) in normalized.split('\n').enumerate() {
        if line_index != 0 {
            xml.push_str("<w:br/>");
        }
        for (tab_index, text) in line.split('\t').enumerate() {
            if tab_index != 0 {
                xml.push_str("<w:tab/>");
            }
            xml.push_str(&format!(
                "<w:t xml:space=\"preserve\">{}</w:t>",
                escape_xml(text)
            ));
        }
    }
    xml.push_str("</w:r>");
    if hyperlink.is_some() {
        xml.push_str("</w:hyperlink>");
    }
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
    crate::ooxml::validate(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip open: {e}"))?;

    let title = read_core_title(&mut archive).unwrap_or_else(|| "Imported Document".to_string());

    let numbering = match archive.by_name("word/numbering.xml") {
        Ok(mut file) => {
            let mut xml = String::new();
            crate::ooxml::bounded(&mut file)
                .read_to_string(&mut xml)
                .map_err(|e| format!("read numbering.xml: {e}"))?;
            parse_numbering(&xml)?
        }
        Err(_) => NumberingDefinitions::default(),
    };
    let imported_images = load_imported_images(&mut archive)?;
    let imported_hyperlinks = load_imported_hyperlinks(&mut archive)?;
    let header = read_part_text(&mut archive, "word/header1.xml").unwrap_or_default();
    let footer_xml = read_part_xml(&mut archive, "word/footer1.xml").unwrap_or_default();
    let footer = xml_visible_text(&footer_xml);
    let footnotes = read_notes(&mut archive, "word/footnotes.xml", false)?;
    let endnotes = read_notes(&mut archive, "word/endnotes.xml", true)?;

    let mut document_xml = String::new();
    crate::ooxml::bounded(
        archive
            .by_name("word/document.xml")
            .map_err(|e| format!("missing word/document.xml: {e}"))?,
    )
    .read_to_string(&mut document_xml)
    .map_err(|e| format!("read document.xml: {e}"))?;

    let styles_xml = read_part_xml(&mut archive, "word/styles.xml").unwrap_or_default();
    let theme_xml = read_part_xml(&mut archive, "word/theme/theme1.xml").unwrap_or_default();
    let mut context = ImportContext {
        styles: ImportStyles::parse(&styles_xml, ThemeFonts::parse(&theme_xml)?)?,
        grid_line_pitch_pt: parse_grid_line_pitch(&document_xml)?,
        ..Default::default()
    };
    let blocks = parse_body(
        &document_xml,
        &numbering,
        &imported_images,
        &imported_hyperlinks,
        &mut context,
    )?;
    let mut doc = WriterDocument::new(title);
    doc.body = blocks;
    doc.professional.paragraph_formats = context.formats;
    doc.professional.header = header;
    doc.professional.footer = footer;
    doc.professional.page_numbers = footer_xml.contains(">PAGE<");
    doc.professional.table_of_contents = document_xml.contains("> TOC ");
    doc.professional.footnotes = footnotes;
    doc.professional.endnotes = endnotes;
    if let Some(settings) = parse_page_settings(&document_xml) {
        doc.page_settings = settings;
    }
    crate::ooxml_import::model_budget(&doc)?;
    Ok(doc)
}

fn read_part_xml<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut xml = String::new();
    crate::ooxml::bounded(archive.by_name(name).ok()?)
        .read_to_string(&mut xml)
        .ok()?;
    Some(xml)
}

fn read_part_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Option<String> {
    read_part_xml(archive, name).map(|xml| xml_visible_text(&xml))
}

fn load_imported_hyperlinks<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<HashMap<String, String>, String> {
    let Some(xml) = read_part_xml(archive, "word/_rels/document.xml.rels") else {
        return Ok(HashMap::new());
    };
    let relationships = crate::ooxml_import::parse(&xml)?;
    let mut seen = HashSet::new();
    let mut links = HashMap::new();
    for relationship in relationships
        .children
        .iter()
        .filter(|r| r.name == "Relationship")
    {
        let Some(id) = relationship.attr("Id") else {
            continue;
        };
        if !seen.insert(id) {
            return Err("Duplicate DOCX relationship Id".into());
        }
        let kind = relationship.attr("Type").unwrap_or_default();
        if (kind == HYPERLINK_RELATIONSHIP
            || kind == "http://purl.oclc.org/ooxml/officeDocument/relationships/hyperlink")
            && relationship.attr("TargetMode") == Some("External")
            && let Some(target) = relationship
                .attr("Target")
                .filter(|target| safe_hyperlink(target))
        {
            links.insert(id.to_owned(), target.to_owned());
        }
    }
    Ok(links)
}

fn xml_visible_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut output = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) if matches!(event.local_name().as_ref(), b"t" | b"delText") => {
                if let Ok(text) = read_text_content(&mut reader, event.local_name().as_ref()) {
                    output.push_str(&text);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    output
}

fn read_notes<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    endnotes: bool,
) -> Result<Vec<syntra_doc_engine::writer::DocumentNote>, String> {
    let Some(xml) = read_part_xml(archive, name) else {
        return Ok(Vec::new());
    };
    let item_name: &[u8] = if endnotes { b"endnote" } else { b"footnote" };
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut current_id = None;
    let mut current_text = String::new();
    let mut notes = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) if event.local_name().as_ref() == item_name => {
                current_id = attr_val(&event, b"id").and_then(|value| value.parse::<i32>().ok());
                current_text.clear();
            }
            Ok(Event::Start(event)) if event.local_name().as_ref() == b"t" => {
                current_text.push_str(&read_text_content(&mut reader, b"t")?);
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == item_name => {
                if let Some(id) = current_id.take()
                    && id > 0
                {
                    notes.push(syntra_doc_engine::writer::DocumentNote {
                        id: id as u32,
                        reference_block: 0,
                        text: current_text.trim().to_string(),
                    });
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(error.to_string()),
            _ => {}
        }
        buf.clear();
    }
    Ok(notes)
}

fn parse_page_settings(xml: &str) -> Option<syntra_doc_engine::writer::PageSettings> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut settings = syntra_doc_engine::writer::PageSettings::default();
    let mut found = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(event)) | Ok(Event::Start(event))
                if event.local_name().as_ref() == b"pgSz" =>
            {
                if let Some(width) = attr_val(&event, b"w").and_then(|v| v.parse::<f32>().ok()) {
                    settings.width_mm = width / 56.692_913;
                }
                if let Some(height) = attr_val(&event, b"h").and_then(|v| v.parse::<f32>().ok()) {
                    settings.height_mm = height / 56.692_913;
                }
                if attr_val(&event, b"orient").as_deref() == Some("landscape") {
                    settings.orientation = syntra_doc_engine::writer::Orientation::Landscape;
                }
                found = true;
            }
            Ok(Event::Empty(event)) | Ok(Event::Start(event))
                if event.local_name().as_ref() == b"pgMar" =>
            {
                for (attribute, target) in [
                    (b"top".as_slice(), &mut settings.margin_top),
                    (b"right".as_slice(), &mut settings.margin_right),
                    (b"bottom".as_slice(), &mut settings.margin_bottom),
                    (b"left".as_slice(), &mut settings.margin_left),
                ] {
                    if let Some(value) =
                        attr_val(&event, attribute).and_then(|v| v.parse::<f32>().ok())
                    {
                        *target = value / 56.692_913;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    found.then_some(settings)
}

fn load_imported_images<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<HashMap<String, String>, String> {
    let mut relationships_xml = String::new();
    let Ok(mut relationships_file) = archive.by_name("word/_rels/document.xml.rels") else {
        return Ok(HashMap::new());
    };
    crate::ooxml::bounded(&mut relationships_file)
        .read_to_string(&mut relationships_xml)
        .map_err(|e| format!("read document relationships: {e}"))?;
    drop(relationships_file);

    let mut reader = Reader::from_str(&relationships_xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut targets = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let relationship_type = attr_val(&e, b"Type").unwrap_or_default();
                if relationship_type.ends_with("/image")
                    && let (Some(id), Some(target)) = (attr_val(&e, b"Id"), attr_val(&e, b"Target"))
                {
                    targets.push((id, target));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("parse document relationships: {e}")),
            _ => {}
        }
        buf.clear();
    }

    let mut images = HashMap::new();
    let mut total_bytes = 0usize;
    for (relationship_id, target) in targets {
        let target = target.replace('\\', "/");
        if target.split('/').any(|part| part == "..") {
            continue;
        }
        let archive_path = if target.starts_with('/') {
            target.trim_start_matches('/').to_string()
        } else {
            format!("word/{target}")
        };
        if !archive_path.starts_with("word/media/") {
            continue;
        }
        let extension = archive_path
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            _ => continue,
        };
        let Ok(mut file) = archive.by_name(&archive_path) else {
            continue;
        };
        if file.size() > MAX_EMBEDDED_IMAGE_BYTES as u64 {
            continue;
        }
        let mut bytes = Vec::with_capacity(file.size() as usize);
        crate::ooxml::bounded(&mut file)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read embedded image: {e}"))?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_TOTAL_IMAGE_BYTES {
            break;
        }
        images.insert(
            relationship_id,
            format!("data:{mime};base64,{}", BASE64.encode(bytes)),
        );
    }
    Ok(images)
}

fn read_core_title<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Option<String> {
    let mut file = archive.by_name("docProps/core.xml").ok()?;
    let mut s = String::new();
    crate::ooxml::bounded(&mut file)
        .read_to_string(&mut s)
        .ok()?;
    let mut reader = Reader::from_str(&s);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"title" => {
                let title = read_text_content(&mut reader, b"title").ok()?;
                return (!title.is_empty()).then_some(title);
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    None
}

#[derive(Clone, Default)]
struct RunProperties {
    style_id: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strikethrough: Option<bool>,
    ascii_font: Option<String>,
    east_asia_font: Option<String>,
    font_size: Option<f32>,
    color: Option<String>,
    background_color: Option<String>,
    vertical_align: Option<String>,
}

impl RunProperties {
    fn overlay(&mut self, other: &Self) {
        macro_rules! merge { ($($field:ident),*) => { $(if other.$field.is_some() { self.$field = other.$field.clone(); })* }; }
        merge!(
            style_id,
            bold,
            italic,
            underline,
            strikethrough,
            ascii_font,
            east_asia_font,
            font_size,
            color,
            background_color,
            vertical_align
        );
    }
    fn apply(&self, run: &mut TextRun) {
        if let Some(value) = self.bold {
            run.bold = value;
        }
        if let Some(value) = self.italic {
            run.italic = value;
        }
        if let Some(value) = self.underline {
            run.underline = value;
        }
        if let Some(value) = self.strikethrough {
            run.strikethrough = value;
        }
        run.font_family = self
            .east_asia_font
            .clone()
            .or_else(|| self.ascii_font.clone());
        run.font_size = self.font_size;
        run.color = self
            .color
            .as_ref()
            .filter(|color| valid_hex_color(color))
            .map(|color| format!("#{color}"));
        run.background_color = self
            .background_color
            .as_ref()
            .filter(|color| valid_hex_color(color))
            .map(|color| format!("#{color}"));
        run.superscript = self.vertical_align.as_deref() == Some("superscript");
        run.subscript = self.vertical_align.as_deref() == Some("subscript");
    }
}

fn valid_hex_color(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Default)]
struct ThemeFonts {
    fonts: HashMap<String, String>,
}
impl ThemeFonts {
    fn parse(xml: &str) -> Result<Self, String> {
        let mut result = Self::default();
        let mut reader = Reader::from_str(xml);
        let mut family = None;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e))
                    if matches!(e.local_name().as_ref(), b"majorFont" | b"minorFont") =>
                {
                    family = Some(if e.local_name().as_ref() == b"majorFont" {
                        "major"
                    } else {
                        "minor"
                    });
                }
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    if let Some(family) = family {
                        let face = attr_val(&e, b"typeface").filter(|value| !value.is_empty());
                        let suffix = match e.local_name().as_ref() {
                            b"latin" => Some("HAnsi"),
                            b"ea" => Some("EastAsia"),
                            b"cs" => Some("Bidi"),
                            b"font" if attr_val(&e, b"script").as_deref() == Some("Hans") => {
                                Some("EastAsia")
                            }
                            _ => None,
                        };
                        if let (Some(face), Some(suffix)) = (face, suffix) {
                            result.fonts.insert(format!("{family}{suffix}"), face);
                        }
                    }
                }
                Ok(Event::End(e))
                    if matches!(e.local_name().as_ref(), b"majorFont" | b"minorFont") =>
                {
                    family = None
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(format!("xml parse theme fonts: {error}")),
                _ => {}
            }
        }
        Ok(result)
    }
    fn get(&self, key: &str) -> Option<String> {
        self.fonts.get(&key.replace("Ascii", "HAnsi")).cloned()
    }
}

#[derive(Clone, Default)]
struct ImportStyle {
    based_on: Option<String>,
    paragraph: ParagraphProperties,
    run: RunProperties,
}

#[derive(Default)]
struct ImportStyles {
    defaults: ImportStyle,
    paragraph_default: Option<String>,
    definitions: HashMap<String, ImportStyle>,
    theme: ThemeFonts,
}

impl ImportStyles {
    fn parse(xml: &str, theme: ThemeFonts) -> Result<Self, String> {
        let mut result = Self {
            theme,
            ..Default::default()
        };
        let mut reader = Reader::from_str(xml);
        let mut current = None::<(String, ImportStyle)>;
        let mut in_defaults = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => match e.local_name().as_ref() {
                    b"docDefaults" => in_defaults = true,
                    b"style" => {
                        if let Some(id) = attr_val(&e, b"styleId") {
                            if attr_val(&e, b"type").as_deref() == Some("paragraph")
                                && attr_val(&e, b"default").is_some_and(|value| {
                                    matches!(value.as_str(), "1" | "true" | "on")
                                })
                            {
                                result.paragraph_default = Some(id.clone());
                            }
                            current = Some((id, ImportStyle::default()));
                        }
                    }
                    b"pPr" => {
                        let properties = parse_p_pr(&mut reader)?;
                        if let Some((_, style)) = current.as_mut() {
                            style.paragraph = properties;
                        } else if in_defaults {
                            result.defaults.paragraph = properties;
                        }
                    }
                    b"rPr" => {
                        let properties = read_run_properties(&mut reader, &result.theme)?;
                        if let Some((_, style)) = current.as_mut() {
                            style.run = properties;
                        } else if in_defaults {
                            result.defaults.run = properties;
                        }
                    }
                    b"basedOn" => {
                        if let Some((_, style)) = current.as_mut() {
                            style.based_on = attr_val(&e, b"val");
                        }
                    }
                    _ => {}
                },
                Ok(Event::Empty(e)) if e.local_name().as_ref() == b"basedOn" => {
                    if let Some((_, style)) = current.as_mut() {
                        style.based_on = attr_val(&e, b"val");
                    }
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"style" => {
                    if let Some((id, style)) = current.take() {
                        result.definitions.insert(id, style);
                    }
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"docDefaults" => {
                    in_defaults = false
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(format!("xml parse styles: {error}")),
                _ => {}
            }
        }
        if result.paragraph_default.is_none() && result.definitions.contains_key("Normal") {
            result.paragraph_default = Some("Normal".into());
        }
        Ok(result)
    }

    fn apply_chain(&self, id: &str, result: &mut ImportStyle, seen: &mut HashSet<String>) {
        if seen.len() >= 32 || !seen.insert(id.to_owned()) {
            return;
        }
        if let Some(style) = self.definitions.get(id) {
            if let Some(base) = &style.based_on {
                self.apply_chain(base, result, seen);
            }
            result.paragraph.overlay(&style.paragraph);
            result.run.overlay(&style.run);
        }
    }

    fn paragraph(&self, direct: &ParagraphProperties) -> ImportStyle {
        let mut result = self.defaults.clone();
        let mut seen = HashSet::new();
        if let Some(default) = &self.paragraph_default {
            self.apply_chain(default, &mut result, &mut seen);
        }
        if let Some(id) = &direct.style_id {
            self.apply_chain(id, &mut result, &mut HashSet::new());
        }
        result.paragraph.overlay(direct);
        result
    }

    fn run(&self, base: &RunProperties, direct: &RunProperties) -> RunProperties {
        let mut result = base.clone();
        if let Some(style) = &direct.style_id {
            let mut resolved = ImportStyle::default();
            self.apply_chain(style, &mut resolved, &mut HashSet::new());
            result.overlay(&resolved.run);
        }
        result.overlay(direct);
        result
    }
}

fn read_run_properties(
    reader: &mut Reader<&[u8]>,
    theme: &ThemeFonts,
) -> Result<RunProperties, String> {
    let mut properties = RunProperties::default();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"rStyle" => properties.style_id = attr_val(&e, b"val"),
                b"b" => properties.bold = Some(run_property_enabled(&e)),
                b"i" => properties.italic = Some(run_property_enabled(&e)),
                b"u" => properties.underline = Some(run_property_enabled(&e)),
                b"strike" => properties.strikethrough = Some(run_property_enabled(&e)),
                b"rFonts" => {
                    properties.ascii_font = attr_val(&e, b"ascii")
                        .or_else(|| attr_val(&e, b"hAnsi"))
                        .or_else(|| {
                            attr_val(&e, b"asciiTheme")
                                .or_else(|| attr_val(&e, b"hAnsiTheme"))
                                .and_then(|key| theme.get(&key))
                        })
                        .filter(|value| !value.is_empty());
                    properties.east_asia_font = attr_val(&e, b"eastAsia")
                        .or_else(|| attr_val(&e, b"eastAsiaTheme").and_then(|key| theme.get(&key)))
                        .filter(|value| !value.is_empty());
                }
                b"sz" => {
                    properties.font_size = attr_val(&e, b"val")
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite() && *value > 0.0)
                        .map(|value| value / 2.0)
                }
                b"color" => properties.color = attr_val(&e, b"val"),
                b"shd" => properties.background_color = attr_val(&e, b"fill"),
                b"vertAlign" => properties.vertical_align = attr_val(&e, b"val"),
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"rPr" => break,
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("xml parse run properties: {error}")),
            _ => {}
        }
    }
    Ok(properties)
}

#[derive(Default)]
struct ImportContext {
    styles: ImportStyles,
    formats: Vec<ParagraphFormat>,
    numbering_counters: HashMap<(String, u32), u32>,
    grid_line_pitch_pt: Option<f32>,
    table_depth: usize,
}

fn parse_grid_line_pitch(xml: &str) -> Result<Option<f32>, String> {
    let mut reader = Reader::from_str(xml);
    let mut in_section = false;
    let mut pitch = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"sectPr" => in_section = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"sectPr" => in_section = false,
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if in_section && e.local_name().as_ref() == b"docGrid" =>
            {
                pitch = if matches!(
                    attr_val(&e, b"type").as_deref(),
                    Some("lines" | "linesAndChars" | "snapToChars")
                ) {
                    attr_val(&e, b"linePitch")
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite() && *value > 0.0)
                        .map(|value| value / 20.0)
                } else {
                    None
                };
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("xml parse document grid: {error}")),
            _ => {}
        }
    }
    Ok(pitch)
}

#[derive(Clone)]
struct NumberingLevel {
    format: String,
    start: u32,
    text: String,
}

#[derive(Default)]
struct NumberingDefinitions {
    levels: HashMap<(String, u32), NumberingLevel>,
}

impl NumberingDefinitions {
    fn level(&self, num_id: &str, level: u32) -> Option<&NumberingLevel> {
        self.levels
            .get(&(num_id.to_owned(), level))
            .or_else(|| self.levels.get(&(num_id.to_owned(), 0)))
    }
    fn is_ordered(&self, num_id: &str, level: u32) -> bool {
        self.level(num_id, level)
            .is_some_and(|level| !matches!(level.format.as_str(), "bullet" | "none"))
    }

    fn label(
        &self,
        reference: &NumberingRef,
        counters: &mut HashMap<(String, u32), u32>,
    ) -> Option<String> {
        let definition = self.level(&reference.num_id, reference.level)?;
        if definition.format == "none" {
            return None;
        }
        counters
            .retain(|(num_id, level), _| num_id != &reference.num_id || *level <= reference.level);
        let counter = counters
            .entry((reference.num_id.clone(), reference.level))
            .and_modify(|value| *value = value.saturating_add(1))
            .or_insert(definition.start);
        let current = *counter;
        let mut label = definition.text.clone();
        for level in 0..=8 {
            let Some(level_definition) = self.level(&reference.num_id, level) else {
                continue;
            };
            let number = if level == reference.level {
                current
            } else {
                counters
                    .get(&(reference.num_id.clone(), level))
                    .copied()
                    .unwrap_or(level_definition.start)
            };
            label = label.replace(
                &format!("%{}", level + 1),
                &format_list_number(number, &level_definition.format),
            );
        }
        Some(label)
    }
}

fn format_list_number(number: u32, format: &str) -> String {
    match format {
        "chineseCounting"
        | "chineseCountingThousand"
        | "ideographTraditional"
        | "taiwaneseCounting" => chinese_number(number),
        "lowerLetter" | "upperLetter" => {
            let mut value = number.max(1);
            let mut letters = Vec::new();
            while value > 0 {
                value -= 1;
                letters.push((b'a' + (value % 26) as u8) as char);
                value /= 26;
            }
            let result = letters.into_iter().rev().collect::<String>();
            if format == "upperLetter" {
                result.to_ascii_uppercase()
            } else {
                result
            }
        }
        "lowerRoman" | "upperRoman" if (1..4000).contains(&number) => {
            let mut value = number;
            let mut result = String::new();
            for (amount, symbol) in [
                (1000, "M"),
                (900, "CM"),
                (500, "D"),
                (400, "CD"),
                (100, "C"),
                (90, "XC"),
                (50, "L"),
                (40, "XL"),
                (10, "X"),
                (9, "IX"),
                (5, "V"),
                (4, "IV"),
                (1, "I"),
            ] {
                while value >= amount {
                    value -= amount;
                    result.push_str(symbol);
                }
            }
            if format == "lowerRoman" {
                result.to_ascii_lowercase()
            } else {
                result
            }
        }
        _ => number.to_string(),
    }
}

fn chinese_number(number: u32) -> String {
    if number == 0 {
        return "零".into();
    }
    if number >= 100_000_000 {
        return number.to_string();
    }
    if number >= 10_000 {
        let rest = number % 10_000;
        return format!(
            "{}万{}{}",
            chinese_number(number / 10_000),
            if rest > 0 && rest < 1000 { "零" } else { "" },
            if rest > 0 {
                chinese_number(rest)
            } else {
                String::new()
            }
        );
    }
    let digits = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    let mut out = String::new();
    let mut zero = false;
    for (scale, unit) in [(1000, "千"), (100, "百"), (10, "十"), (1, "")] {
        let digit = number / scale % 10;
        if digit > 0 {
            if zero {
                out.push_str("零");
                zero = false;
            }
            if !(digit == 1 && scale == 10 && out.is_empty()) {
                out.push_str(digits[digit as usize]);
            }
            out.push_str(unit);
        } else if !out.is_empty() && number % scale > 0 {
            zero = true;
        }
    }
    out
}

fn parse_numbering(xml: &str) -> Result<NumberingDefinitions, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut current_abstract: Option<String> = None;
    let mut current_level: Option<u32> = None;
    let mut current_num: Option<String> = None;
    let mut abstract_formats: HashMap<(String, u32), NumberingLevel> = HashMap::new();
    let mut num_to_abstract: HashMap<String, String> = HashMap::new();
    let mut overrides = HashMap::<(String, u32), u32>::new();
    let mut override_level = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"abstractNum" => current_abstract = attr_val(&e, b"abstractNumId"),
                b"lvl" if current_abstract.is_some() => {
                    current_level = attr_val(&e, b"ilvl").and_then(|v| v.parse().ok());
                    abstract_formats
                        .entry((
                            current_abstract.clone().unwrap_or_default(),
                            current_level.unwrap_or(0),
                        ))
                        .or_insert_with(|| NumberingLevel {
                            format: "decimal".into(),
                            start: 1,
                            text: format!("%{}.", current_level.unwrap_or(0) + 1),
                        });
                }
                b"numFmt" if current_abstract.is_some() && current_level.is_some() => {
                    if let Some(format) = attr_val(&e, b"val") {
                        if let Some(level) = abstract_formats.get_mut(&(
                            current_abstract.clone().unwrap_or_default(),
                            current_level.unwrap_or(0),
                        )) {
                            level.format = format;
                        }
                    }
                }
                b"start" if current_abstract.is_some() && current_level.is_some() => {
                    if let Some(value) = attr_val(&e, b"val").and_then(|value| value.parse().ok())
                        && let Some(level) = abstract_formats.get_mut(&(
                            current_abstract.clone().unwrap_or_default(),
                            current_level.unwrap_or(0),
                        ))
                    {
                        level.start = value;
                    }
                }
                b"lvlText" if current_abstract.is_some() && current_level.is_some() => {
                    if let Some(value) = attr_val(&e, b"val")
                        && let Some(level) = abstract_formats.get_mut(&(
                            current_abstract.clone().unwrap_or_default(),
                            current_level.unwrap_or(0),
                        ))
                    {
                        level.text = value;
                    }
                }
                b"num" => current_num = attr_val(&e, b"numId"),
                b"lvlOverride" if current_num.is_some() => {
                    override_level = attr_val(&e, b"ilvl")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0)
                }
                b"startOverride" if current_num.is_some() => {
                    if let Some(value) = attr_val(&e, b"val").and_then(|value| value.parse().ok()) {
                        overrides.insert(
                            (current_num.clone().unwrap_or_default(), override_level),
                            value,
                        );
                    }
                }
                b"abstractNumId" if current_num.is_some() => {
                    if let Some(abstract_id) = attr_val(&e, b"val") {
                        num_to_abstract
                            .insert(current_num.clone().unwrap_or_default(), abstract_id);
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"abstractNum" => {
                    current_abstract = None;
                    current_level = None;
                }
                b"lvl" => current_level = None,
                b"num" => current_num = None,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse numbering: {e}")),
            _ => {}
        }
        buf.clear();
    }

    let mut definitions = NumberingDefinitions::default();
    for (num_id, abstract_id) in num_to_abstract {
        for ((candidate_id, level), definition) in &abstract_formats {
            if candidate_id == &abstract_id {
                let mut definition = definition.clone();
                if let Some(start) = overrides.get(&(num_id.clone(), *level)) {
                    definition.start = *start;
                }
                definitions
                    .levels
                    .insert((num_id.clone(), *level), definition);
            }
        }
    }
    Ok(definitions)
}

#[derive(Clone)]
struct NumberingRef {
    num_id: String,
    level: u32,
}

struct ParsedParagraph {
    blocks: Vec<Block>,
    numbering: Option<NumberingRef>,
    format: Option<ParagraphFormat>,
}

struct PendingList {
    num_id: String,
    ordered: bool,
    entries: Vec<(u32, Vec<TextRun>)>,
}

fn flush_pending_list(pending: &mut Option<PendingList>, blocks: &mut Vec<Block>) {
    if let Some(list) = pending.take() {
        blocks.push(Block::List(List {
            id: Uuid::new_v4(),
            ordered: list.ordered,
            items: build_list_items(&list.entries),
        }));
    }
}

fn build_list_items(entries: &[(u32, Vec<TextRun>)]) -> Vec<ListItem> {
    let Some(base_level) = entries.iter().map(|(level, _)| *level).min() else {
        return Vec::new();
    };
    let mut index = 0;
    build_list_level(entries, &mut index, base_level)
}

fn build_list_level(
    entries: &[(u32, Vec<TextRun>)],
    index: &mut usize,
    level: u32,
) -> Vec<ListItem> {
    let mut items: Vec<ListItem> = Vec::new();
    while *index < entries.len() {
        let (entry_level, runs) = &entries[*index];
        if *entry_level < level {
            break;
        }
        if *entry_level > level
            && let Some(parent) = items.last_mut()
        {
            parent
                .children
                .extend(build_list_level(entries, index, *entry_level));
            continue;
        }

        items.push(ListItem {
            runs: runs.clone(),
            children: Vec::new(),
        });
        *index += 1;
    }
    items
}

fn append_paragraph(
    mut parsed: ParsedParagraph,
    numbering: &NumberingDefinitions,
    pending: &mut Option<PendingList>,
    blocks: &mut Vec<Block>,
    context: &mut ImportContext,
) {
    let Some(reference) = parsed.numbering.filter(|reference| reference.num_id != "0") else {
        flush_pending_list(pending, blocks);
        context.formats.extend(parsed.format);
        blocks.extend(parsed.blocks);
        return;
    };

    if let Some(level) = numbering.level(&reference.num_id, reference.level)
        && !matches!(level.format.as_str(), "bullet" | "decimal")
    {
        flush_pending_list(pending, blocks);
        if let Some(format) = parsed.format.as_mut() {
            format.numbering_label = numbering.label(&reference, &mut context.numbering_counters);
        }
        context.formats.extend(parsed.format);
        blocks.extend(parsed.blocks);
        return;
    }

    let ordered = numbering.is_ordered(&reference.num_id, reference.level);
    let mut parsed_blocks = parsed.blocks.into_iter();
    let runs = match parsed_blocks.next() {
        Some(Block::Paragraph(paragraph)) => paragraph.runs,
        Some(Block::Heading(heading)) => heading.runs,
        Some(other) => {
            flush_pending_list(pending, blocks);
            blocks.push(other);
            blocks.extend(parsed_blocks);
            return;
        }
        None => return,
    };
    let same_list = pending
        .as_ref()
        .is_some_and(|list| list.num_id == reference.num_id && list.ordered == ordered);
    if !same_list {
        flush_pending_list(pending, blocks);
        *pending = Some(PendingList {
            num_id: reference.num_id.clone(),
            ordered,
            entries: Vec::new(),
        });
    }
    if let Some(list) = pending {
        list.entries.push((reference.level, runs));
    }
    let remaining: Vec<Block> = parsed_blocks.collect();
    if !remaining.is_empty() {
        flush_pending_list(pending, blocks);
        blocks.extend(remaining);
    }
}

fn parse_body(
    xml: &str,
    numbering: &NumberingDefinitions,
    images: &HashMap<String, String>,
    links: &HashMap<String, String>,
    context: &mut ImportContext,
) -> Result<Vec<Block>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut blocks = Vec::new();
    let mut pending_list = None;
    let mut in_body = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                let n = name.as_ref();
                if n == b"body" {
                    in_body = true;
                } else if in_body && n == b"p" {
                    append_paragraph(
                        parse_paragraph(&mut reader, images, links, context)?,
                        numbering,
                        &mut pending_list,
                        &mut blocks,
                        context,
                    );
                } else if in_body && n == b"tbl" {
                    flush_pending_list(&mut pending_list, &mut blocks);
                    blocks.push(parse_table(&mut reader, numbering, images, links, context)?);
                }
            }
            Ok(Event::Empty(e)) if in_body && e.local_name().as_ref() == b"p" => {
                flush_pending_list(&mut pending_list, &mut blocks);
                blocks.push(empty_paragraph());
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"body" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse: {e}")),
            _ => {}
        }
        buf.clear();
    }
    flush_pending_list(&mut pending_list, &mut blocks);
    Ok(blocks)
}

fn empty_paragraph() -> Block {
    Block::Paragraph(Paragraph {
        id: Uuid::new_v4(),
        runs: Vec::new(),
        alignment: Alignment::default(),
        indent_level: 0,
        spacing: Spacing::default(),
    })
}

fn parse_paragraph(
    reader: &mut Reader<&[u8]>,
    images: &HashMap<String, String>,
    links: &HashMap<String, String>,
    context: &mut ImportContext,
) -> Result<ParsedParagraph, String> {
    let mut buf = Vec::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut style = context.styles.paragraph(&ParagraphProperties::default());
    let mut layout = style.paragraph.clone();
    let mut numbering = layout.numbering.clone();
    let mut hyperlink: Option<String> = None;
    let mut image_blocks = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.local_name().as_ref().to_vec();
                if n == b"pPr" {
                    style = context.styles.paragraph(&parse_p_pr(reader)?);
                    layout = style.paragraph.clone();
                    numbering = layout.numbering.clone();
                } else if n == b"hyperlink" {
                    hyperlink = attr_val(&e, b"r:id").and_then(|id| links.get(&id).cloned());
                } else if n == b"r" {
                    let (mut run, run_images) =
                        parse_run(reader, images, &context.styles, &style.run)?;
                    run.link = hyperlink.clone();
                    if !run.text.is_empty() {
                        runs.push(run);
                    }
                    image_blocks.extend(run_images.into_iter().map(Block::Image));
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"hyperlink" => hyperlink = None,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"p" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse paragraph: {e}")),
            _ => {}
        }
        buf.clear();
    }
    let mut blocks = Vec::new();
    if !runs.is_empty() {
        blocks.push(if let Some(level) = layout.heading_level {
            Block::Heading(Heading {
                id: Uuid::new_v4(),
                level,
                runs,
            })
        } else {
            Block::Paragraph(Paragraph {
                id: Uuid::new_v4(),
                runs,
                alignment: layout.alignment.unwrap_or_default(),
                indent_level: (layout.left_indent_pt.unwrap_or(0.0) / 36.0)
                    .round()
                    .clamp(0.0, 255.0) as u8,
                spacing: layout.spacing(),
            })
        });
    }
    blocks.extend(image_blocks);
    if blocks.is_empty() {
        blocks.push(Block::Paragraph(Paragraph {
            id: Uuid::new_v4(),
            runs: Vec::new(),
            alignment: layout.alignment.unwrap_or_default(),
            indent_level: (layout.left_indent_pt.unwrap_or(0.0) / 36.0)
                .round()
                .clamp(0.0, 255.0) as u8,
            spacing: layout.spacing(),
        }));
    }
    // Grid line pitch is not applied inside table cells by default. Explicit
    // exact spacing and an inherited/direct snapToGrid=false also take priority.
    let grid_pitch = if context.table_depth == 0 {
        context.grid_line_pitch_pt
    } else {
        None
    };
    let format = blocks.first().and_then(|block| match block {
        Block::Paragraph(p) => Some(layout.format(p.id, grid_pitch)),
        Block::Heading(h) => Some(layout.format(h.id, grid_pitch)),
        _ => None,
    });
    Ok(ParsedParagraph {
        blocks,
        numbering,
        format,
    })
}

#[derive(Clone, Default)]
struct ParagraphProperties {
    style_id: Option<String>,
    heading_level: Option<u8>,
    numbering: Option<NumberingRef>,
    alignment: Option<Alignment>,
    first_line_indent_pt: Option<f32>,
    first_line_indent_chars: Option<f32>,
    left_indent_pt: Option<f32>,
    right_indent_pt: Option<f32>,
    before: Option<f32>,
    after: Option<f32>,
    line: Option<f32>,
    line_rule: Option<String>,
    snap_to_grid: Option<bool>,
}

impl ParagraphProperties {
    fn overlay(&mut self, other: &Self) {
        macro_rules! merge { ($($field:ident),*) => { $(if other.$field.is_some() { self.$field = other.$field.clone(); })* }; }
        merge!(
            style_id,
            heading_level,
            numbering,
            alignment,
            first_line_indent_pt,
            first_line_indent_chars,
            left_indent_pt,
            right_indent_pt,
            before,
            after,
            line,
            line_rule,
            snap_to_grid
        );
    }
    fn spacing(&self) -> Spacing {
        Spacing {
            before: self.before.unwrap_or(0.0),
            after: self.after.unwrap_or(0.0),
            line_height: if self.line_rule.as_deref().is_none_or(|rule| rule == "auto") {
                self.line.unwrap_or(240.0) / 240.0
            } else {
                1.0
            },
        }
    }
    fn format(&self, block_id: Uuid, grid_line_pitch_pt: Option<f32>) -> ParagraphFormat {
        ParagraphFormat {
            block_id,
            first_line_indent_pt: self.first_line_indent_pt,
            first_line_indent_chars: self.first_line_indent_chars,
            left_indent_pt: self.left_indent_pt,
            right_indent_pt: self.right_indent_pt,
            line_height_pt: self
                .line
                .filter(|_| self.line_rule.as_deref().is_some_and(|rule| rule != "auto"))
                .map(|value| value / 20.0)
                .or_else(|| {
                    (self.snap_to_grid != Some(false)
                        && self.line_rule.as_deref().is_none_or(|rule| rule == "auto"))
                    .then_some(grid_line_pitch_pt)
                    .flatten()
                    .map(|pitch| pitch * self.spacing().line_height)
                }),
            numbering_label: None,
            alignment: self.alignment,
        }
    }
}

fn parse_p_pr(reader: &mut Reader<&[u8]>) -> Result<ParagraphProperties, String> {
    let mut buf = Vec::new();
    let mut properties = ParagraphProperties::default();
    let mut num_id = None;
    let mut list_level = 0;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"rPr" => {
                // Paragraph-mark run properties contain a different snapToGrid
                // property for character spacing; it must not affect line pitch.
                reader
                    .read_to_end(e.name())
                    .map_err(|error| format!("xml parse paragraph mark properties: {error}"))?;
            }
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"pStyle" {
                    properties.style_id = attr_val(&e, b"val");
                    if let Some(val) = properties.style_id.as_ref()
                        && let Some(rest) = val.to_ascii_lowercase().strip_prefix("heading")
                        && let Ok(n) = rest.parse::<u8>()
                    {
                        properties.heading_level = Some(n.clamp(1, 6));
                    }
                } else if e.local_name().as_ref() == b"snapToGrid" {
                    properties.snap_to_grid = Some(run_property_enabled(&e));
                } else if e.local_name().as_ref() == b"numId" {
                    num_id = attr_val(&e, b"val");
                } else if e.local_name().as_ref() == b"ilvl" {
                    list_level = attr_val(&e, b"val")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                } else if e.local_name().as_ref() == b"jc" {
                    properties.alignment = Some(match attr_val(&e, b"val").as_deref() {
                        Some("center") => Alignment::Center,
                        Some("right" | "end") => Alignment::Right,
                        Some("both" | "justify" | "distribute") => Alignment::Justify,
                        _ => Alignment::Left,
                    });
                } else if e.local_name().as_ref() == b"ind" {
                    let number = |key: &[u8]| {
                        attr_val(&e, key)
                            .and_then(|value| value.parse::<f32>().ok())
                            .filter(|value| value.is_finite())
                    };
                    properties.left_indent_pt = number(b"start")
                        .or_else(|| number(b"left"))
                        .map(|value| value / 20.0);
                    properties.right_indent_pt = number(b"end")
                        .or_else(|| number(b"right"))
                        .map(|value| value / 20.0);
                    properties.first_line_indent_pt = number(b"firstLine")
                        .or_else(|| number(b"hanging").map(|value| -value))
                        .map(|value| value / 20.0);
                    properties.first_line_indent_chars = number(b"firstLineChars")
                        .or_else(|| number(b"hangingChars").map(|value| -value))
                        .map(|value| value / 100.0);
                } else if e.local_name().as_ref() == b"spacing" {
                    let number = |key: &[u8]| {
                        attr_val(&e, key)
                            .and_then(|v| v.parse::<f32>().ok())
                            .filter(|v| v.is_finite() && *v >= 0.0)
                    };
                    if let Some(before) = number(b"before") {
                        properties.before = Some(before / 20.0);
                    }
                    if let Some(after) = number(b"after") {
                        properties.after = Some(after / 20.0);
                    }
                    properties.line = number(b"line").filter(|v| *v > 0.0);
                    properties.line_rule = attr_val(&e, b"lineRule");
                } else if e.local_name().as_ref() == b"outlineLvl" {
                    properties.heading_level = attr_val(&e, b"val")
                        .and_then(|value| value.parse::<u8>().ok())
                        .filter(|value| *value < 6)
                        .map(|value| value + 1);
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"pPr" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse pPr: {e}")),
            _ => {}
        }
        buf.clear();
    }
    properties.numbering = num_id.map(|num_id| NumberingRef {
        num_id,
        level: list_level,
    });
    Ok(properties)
}

fn parse_run(
    reader: &mut Reader<&[u8]>,
    images: &HashMap<String, String>,
    styles: &ImportStyles,
    base: &RunProperties,
) -> Result<(TextRun, Vec<ImageBlock>), String> {
    let mut buf = Vec::new();
    let mut run = TextRun::default();
    base.apply(&mut run);
    let mut run_images = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.local_name().as_ref().to_vec();
                if n == b"rPr" {
                    let direct = read_run_properties(reader, &styles.theme)?;
                    styles.run(base, &direct).apply(&mut run);
                } else if n == b"t" {
                    let text = read_text_content(reader, b"t")?;
                    run.text.push_str(&text);
                } else if n == b"tab" {
                    run.text.push('\t');
                } else if n == b"br" || n == b"cr" {
                    run.text.push('\n');
                } else if n == b"drawing"
                    && let Some(image) = parse_drawing(reader, images)?
                {
                    run_images.push(image);
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"tab" => run.text.push('\t'),
                b"br" | b"cr" => run.text.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"r" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse run: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok((run, run_images))
}

fn parse_drawing(
    reader: &mut Reader<&[u8]>,
    images: &HashMap<String, String>,
) -> Result<Option<ImageBlock>, String> {
    let mut buf = Vec::new();
    let mut relationship_id = None;
    let mut width = None;
    let mut height = None;
    let mut alt_text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"blip" => relationship_id = attr_val(&e, b"r:embed"),
                b"extent" => {
                    width = attr_val(&e, b"cx")
                        .and_then(|value| value.parse::<f32>().ok())
                        .map(|value| value / 9525.0);
                    height = attr_val(&e, b"cy")
                        .and_then(|value| value.parse::<f32>().ok())
                        .map(|value| value / 9525.0);
                }
                b"docPr" | b"cNvPr" if alt_text.is_empty() => {
                    alt_text = attr_val(&e, b"descr")
                        .or_else(|| attr_val(&e, b"name"))
                        .unwrap_or_default();
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"drawing" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse drawing: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(relationship_id.and_then(|relationship_id| {
        images.get(&relationship_id).map(|url| ImageBlock {
            id: Uuid::new_v4(),
            url: url.clone(),
            alt_text,
            width,
            height,
            alignment: Default::default(),
        })
    }))
}

fn read_text_content(reader: &mut Reader<&[u8]>, end: &[u8]) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut out = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => {
                out.push_str(&t.decode().map_err(|e| e.to_string())?);
            }
            Ok(Event::CData(t)) => out.push_str(&t.decode().map_err(|e| e.to_string())?),
            Ok(Event::GeneralRef(reference)) => {
                let encoded = format!("&{};", reference.decode().map_err(|e| e.to_string())?);
                out.push_str(&quick_xml::escape::unescape(&encoded).map_err(|e| e.to_string())?);
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == end => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml text: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn run_property_enabled(element: &BytesStart) -> bool {
    !attr_val(element, b"val").is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "none"
        )
    })
}

fn parse_table(
    reader: &mut Reader<&[u8]>,
    numbering: &NumberingDefinitions,
    images: &HashMap<String, String>,
    links: &HashMap<String, String>,
    context: &mut ImportContext,
) -> Result<Block, String> {
    context.table_depth += 1;
    let mut buf = Vec::new();
    let mut rows = Vec::new();
    let mut column_widths = Vec::new();
    let mut in_grid = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"tr" => {
                rows.push(parse_row(reader, numbering, images, links, context)?);
            }
            // A tracked grid change contains a second, historical grid.
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"tblGridChange" => {
                reader
                    .read_to_end(e.name())
                    .map_err(|e| format!("xml parse table grid change: {e}"))?;
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"tblGrid" => in_grid = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"tblGrid" => in_grid = false,
            Ok(Event::Start(e) | Event::Empty(e))
                if in_grid && e.local_name().as_ref() == b"gridCol" =>
            {
                if column_widths.len() >= 10_000 {
                    return Err("DOCX table has too many grid columns".into());
                }
                let twips = attr_val(&e, b"w")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                column_widths.push(twips as f32 / 15.);
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"tbl" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse table: {e}")),
            _ => {}
        }
        buf.clear();
    }
    let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    column_widths.resize(column_widths.len().max(columns), 0.);
    context.table_depth -= 1;
    Ok(Block::Table(Table {
        id: Uuid::new_v4(),
        rows,
        column_widths,
    }))
}

fn parse_row(
    reader: &mut Reader<&[u8]>,
    numbering: &NumberingDefinitions,
    images: &HashMap<String, String>,
    links: &HashMap<String, String>,
    context: &mut ImportContext,
) -> Result<TableRow, String> {
    let mut buf = Vec::new();
    let mut cells: Vec<TableCell> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"tc" {
                    cells.push(parse_cell(reader, numbering, images, links, context)?);
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"tr" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse row: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(TableRow { cells })
}

fn parse_cell(
    reader: &mut Reader<&[u8]>,
    numbering: &NumberingDefinitions,
    images: &HashMap<String, String>,
    links: &HashMap<String, String>,
    context: &mut ImportContext,
) -> Result<TableCell, String> {
    let mut buf = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut pending_list = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.local_name().as_ref().to_vec();
                if n == b"p" {
                    append_paragraph(
                        parse_paragraph(reader, images, links, context)?,
                        numbering,
                        &mut pending_list,
                        &mut blocks,
                        context,
                    );
                } else if n == b"tbl" {
                    flush_pending_list(&mut pending_list, &mut blocks);
                    blocks.push(parse_table(reader, numbering, images, links, context)?);
                }
            }
            Ok(Event::Empty(e)) if e.local_name().as_ref() == b"p" => {
                flush_pending_list(&mut pending_list, &mut blocks);
                blocks.push(empty_paragraph());
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"tc" => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse cell: {e}")),
            _ => {}
        }
        buf.clear();
    }
    flush_pending_list(&mut pending_list, &mut blocks);
    Ok(TableCell {
        blocks,
        col_span: 1,
        row_span: 1,
        background_color: None,
    })
}

fn attr_val(e: &BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        let local = key.rsplit(|byte| *byte == b':').next().unwrap_or(key);
        if a.key.local_name().as_ref() == local {
            return quick_xml::escape::unescape(std::str::from_utf8(&a.value).ok()?)
                .ok()
                .map(|value| value.into_owned());
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
    use syntra_doc_engine::writer::{List, ListItem, Paragraph, TextRun, WriterDocument};

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
    fn embedded_image_is_packaged_and_round_trips() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
        let data_url = format!("data:image/png;base64,{PNG}");
        let mut doc = WriterDocument::new("Image document".into());
        doc.body.push(Block::Image(ImageBlock {
            id: Uuid::new_v4(),
            url: data_url.clone(),
            alt_text: "A tiny pixel".into(),
            width: Some(32.0),
            height: Some(16.0),
            alignment: Default::default(),
        }));

        let bytes = writer_to_docx(&doc).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut media = Vec::new();
        archive
            .by_name("word/media/image1.png")
            .unwrap()
            .read_to_end(&mut media)
            .unwrap();
        assert_eq!(media, BASE64.decode(PNG).unwrap());
        let mut relationships = String::new();
        archive
            .by_name("word/_rels/document.xml.rels")
            .unwrap()
            .read_to_string(&mut relationships)
            .unwrap();
        assert!(relationships.contains("relationships/image"));
        drop(archive);

        let imported = docx_to_writer(&bytes).unwrap();
        let image = imported
            .body
            .iter()
            .find_map(|block| match block {
                Block::Image(image) => Some(image),
                _ => None,
            })
            .expect("embedded image should be restored");
        assert_eq!(image.url, data_url);
        assert_eq!(image.alt_text, "A tiny pixel");
        assert_eq!(image.width, Some(32.0));
        assert_eq!(image.height, Some(16.0));
    }

    #[test]
    fn professional_layout_parts_are_packaged_and_imported() {
        let mut doc = WriterDocument::new("Professional".into());
        doc.add_heading("Overview", 1).add_paragraph("Body");
        doc.professional.header = "Confidential".into();
        doc.professional.footer = "Quarterly report".into();
        doc.professional.page_numbers = true;
        doc.professional.table_of_contents = true;
        doc.professional
            .footnotes
            .push(syntra_doc_engine::writer::DocumentNote {
                id: 1,
                reference_block: 1,
                text: "Supporting detail".into(),
            });
        doc.professional
            .mail_merge_fields
            .push(syntra_doc_engine::writer::MailMergeField {
                name: "CustomerName".into(),
                block_index: 1,
                default_value: Some("Customer".into()),
            });

        let bytes = writer_to_docx(&doc).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        for part in ["word/header1.xml", "word/footer1.xml", "word/footnotes.xml"] {
            assert!(archive.by_name(part).is_ok(), "missing {part}");
        }
        let mut document_xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut document_xml)
            .unwrap();
        assert!(document_xml.contains("MERGEFIELD CustomerName"));
        assert!(document_xml.contains("TOC"));
        drop(archive);

        let imported = docx_to_writer(&bytes).unwrap();
        assert_eq!(imported.professional.header, "Confidential");
        assert!(imported.professional.footer.contains("Quarterly report"));
        assert!(imported.professional.page_numbers);
        assert!(imported.professional.table_of_contents);
        assert_eq!(imported.professional.footnotes[0].text, "Supporting detail");
    }

    #[test]
    fn test_docx_bold_text() {
        let mut doc = WriterDocument::new("Bold Test".into());
        doc.body
            .push(Block::Paragraph(syntra_doc_engine::writer::Paragraph {
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

    #[test]
    fn test_docx_ordered_and_unordered_lists_round_trip() {
        let mut doc = WriterDocument::new("Lists".into());
        doc.body.push(Block::List(List {
            id: Uuid::new_v4(),
            ordered: false,
            items: vec![
                ListItem {
                    runs: vec![TextRun::plain("Bullet one")],
                    children: vec![ListItem {
                        runs: vec![TextRun::plain("Nested bullet")],
                        children: vec![ListItem {
                            runs: vec![TextRun::plain("Deep bullet")],
                            children: Vec::new(),
                        }],
                    }],
                },
                ListItem {
                    runs: vec![TextRun::plain("Bullet two")],
                    children: Vec::new(),
                },
            ],
        }));
        doc.add_paragraph("Between lists");
        doc.body.push(Block::List(List {
            id: Uuid::new_v4(),
            ordered: true,
            items: vec![
                ListItem {
                    runs: vec![TextRun::plain("First")],
                    children: Vec::new(),
                },
                ListItem {
                    runs: vec![TextRun::plain("Second")],
                    children: Vec::new(),
                },
            ],
        }));

        let bytes = writer_to_docx(&doc).unwrap();
        let imported = docx_to_writer(&bytes).unwrap();
        assert_eq!(imported.body.len(), 3);

        let Block::List(bullets) = &imported.body[0] else {
            panic!("expected unordered list");
        };
        assert!(!bullets.ordered);
        assert_eq!(bullets.items.len(), 2);
        assert_eq!(bullets.items[0].runs[0].text, "Bullet one");
        assert_eq!(bullets.items[0].children.len(), 1);
        assert_eq!(bullets.items[0].children[0].runs[0].text, "Nested bullet");
        assert_eq!(
            bullets.items[0].children[0].children[0].runs[0].text,
            "Deep bullet"
        );

        let Block::List(numbers) = &imported.body[2] else {
            panic!("expected ordered list");
        };
        assert!(numbers.ordered);
        assert_eq!(numbers.items.len(), 2);
        assert_eq!(numbers.items[1].runs[0].text, "Second");
    }

    #[test]
    fn numbering_defines_every_ooxml_list_level() {
        let xml = numbering_xml();
        for level in 0..=8 {
            assert!(xml.contains(&format!(r#"w:ilvl="{level}""#)));
        }
    }
}
