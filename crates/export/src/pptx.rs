//! PPTX export and import for Slides presentations.
//!
//! Converts a [`Presentation`] into a `.pptx` byte vector (ZIP archive with
//! Open XML) and parses a `.pptx` byte slice back into a [`Presentation`].

use std::io::{Cursor, Read, Write};

use contrix_doc_engine::slides::{
    Presentation, Slide, SlideElement, SlideLayout, SlideTextRun, TextBox,
};
use quick_xml::Reader;
use quick_xml::events::Event;
use uuid::Uuid;
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Convert a [`Presentation`] to a `.pptx` byte vector (ZIP archive with Open XML).
pub fn presentation_to_pptx(pres: &Presentation) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>"#,
    );
    for (i, _) in pres.slides.iter().enumerate() {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            i + 1
        ));
    }
    ct.push_str("\n</Types>");
    write!(zip, "{}", ct).map_err(|e| e.to_string())?;

    // _rels/.rels
    zip.start_file("_rels/.rels", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#
    )
    .map_err(|e| e.to_string())?;

    // ppt/_rels/presentation.xml.rels
    zip.start_file("ppt/_rels/presentation.xml.rels", options)
        .map_err(|e| e.to_string())?;
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (i, _) in pres.slides.iter().enumerate() {
        rels.push_str(&format!(
            "\n  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
            i + 1,
            i + 1
        ));
    }
    rels.push_str("\n</Relationships>");
    write!(zip, "{}", rels).map_err(|e| e.to_string())?;

    // ppt/presentation.xml
    zip.start_file("ppt/presentation.xml", options)
        .map_err(|e| e.to_string())?;
    let mut p = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst>"#,
    );
    for (i, _) in pres.slides.iter().enumerate() {
        p.push_str(&format!(
            "\n    <p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            256 + i as u32,
            i + 1
        ));
    }
    p.push_str("\n  </p:sldIdLst>\n</p:presentation>");
    write!(zip, "{}", p).map_err(|e| e.to_string())?;

    // docProps/core.xml — carries the title
    zip.start_file("docProps/core.xml", options)
        .map_err(|e| e.to_string())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>{}</dc:title>
</cp:coreProperties>"#,
        escape_xml(&pres.title)
    )
    .map_err(|e| e.to_string())?;

    // ppt/slides/slide{N}.xml
    for (i, slide) in pres.slides.iter().enumerate() {
        zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), options)
            .map_err(|e| e.to_string())?;
        let xml = slide_to_xml(slide);
        write!(zip, "{}", xml).map_err(|e| e.to_string())?;
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

fn slide_to_xml(slide: &Slide) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>"#,
    );

    let mut sp_id: u32 = 2;
    for element in &slide.elements {
        if let SlideElement::TextBox(tb) = element {
            text_box_to_xml(tb, sp_id, &mut xml);
            sp_id += 1;
        }
    }

    xml.push_str("</p:spTree></p:cSld></p:sld>");
    xml
}

fn text_box_to_xml(tb: &TextBox, sp_id: u32, xml: &mut String) {
    let x_emu = (tb.x * 9525.0) as i64;
    let y_emu = (tb.y * 9525.0) as i64;
    let w_emu = (tb.width * 9525.0) as i64;
    let h_emu = (tb.height * 9525.0) as i64;

    xml.push_str(&format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"TextBox {}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>",
        sp_id, sp_id
    ));
    xml.push_str(&format!(
        "<p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>",
        x_emu, y_emu, w_emu, h_emu
    ));
    xml.push_str("<p:txBody><a:bodyPr/><a:lstStyle/><a:p>");
    for run in &tb.content {
        run_to_xml(run, xml);
    }
    xml.push_str("</a:p></p:txBody></p:sp>");
}

fn run_to_xml(run: &SlideTextRun, xml: &mut String) {
    xml.push_str("<a:r>");
    let has_props = run.bold
        || run.italic
        || run.underline
        || run.strikethrough
        || run.font_size.is_some()
        || run.color.is_some()
        || run.font_family.is_some();
    if has_props {
        xml.push_str("<a:rPr lang=\"en-US\"");
        if run.bold {
            xml.push_str(" b=\"1\"");
        }
        if run.italic {
            xml.push_str(" i=\"1\"");
        }
        if run.underline {
            xml.push_str(" u=\"sng\"");
        }
        if run.strikethrough {
            xml.push_str(" strike=\"sngStrike\"");
        }
        if let Some(size) = run.font_size {
            xml.push_str(&format!(" sz=\"{}\"", (size * 100.0) as u32));
        }
        xml.push('>');
        if let Some(ref color) = run.color {
            let c = color.trim_start_matches('#');
            xml.push_str(&format!(
                "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
                escape_xml(c)
            ));
        }
        if let Some(ref font) = run.font_family {
            xml.push_str(&format!(
                "<a:latin typeface=\"{}\"/>",
                escape_xml(font)
            ));
        }
        xml.push_str("</a:rPr>");
    } else {
        xml.push_str("<a:rPr lang=\"en-US\"/>");
    }
    xml.push_str(&format!("<a:t>{}</a:t>", escape_xml(&run.text)));
    xml.push_str("</a:r>");
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parse a `.pptx` byte slice into a [`Presentation`].
pub fn pptx_to_presentation(bytes: &[u8]) -> Result<Presentation, String> {
    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader).map_err(|e| e.to_string())?;

    let title = read_title(&mut archive).unwrap_or_else(|| "Imported Presentation".to_string());

    let mut slide_names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .map_err(|e| e.to_string())?
            .name()
            .to_string();
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            slide_names.push(name);
        }
    }
    slide_names.sort_by_key(|n| slide_index(n).unwrap_or(u32::MAX));

    let mut pres = Presentation {
        id: Uuid::new_v4(),
        title,
        slides: Vec::new(),
        master: Default::default(),
        metadata: Default::default(),
    };

    for name in &slide_names {
        let mut xml = String::new();
        archive
            .by_name(name)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut xml)
            .map_err(|e| e.to_string())?;
        let texts = parse_slide_texts(&xml)?;
        let mut slide = Slide::new(SlideLayout::Blank);
        for (idx, text) in texts.into_iter().enumerate() {
            let y = 50.0 + (idx as f32) * 110.0;
            slide.elements.push(SlideElement::TextBox(TextBox {
                id: Uuid::new_v4(),
                x: 50.0,
                y,
                width: 600.0,
                height: 100.0,
                rotation: 0.0,
                content: vec![SlideTextRun::plain(text)],
                background_color: None,
                border: None,
                padding: 8.0,
            }));
        }
        pres.slides.push(slide);
    }

    Ok(pres)
}

fn slide_index(name: &str) -> Option<u32> {
    let stem = name
        .strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?;
    stem.parse().ok()
}

fn read_title(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Option<String> {
    let mut xml = String::new();
    archive
        .by_name("docProps/core.xml")
        .ok()?
        .read_to_string(&mut xml)
        .ok()?;
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_title = false;
    let mut title = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"dc:title" {
                    in_title = true;
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"dc:title" {
                    in_title = false;
                }
            }
            Ok(Event::Text(t)) => {
                if in_title {
                    title.push_str(&t.unescape().ok()?);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    if title.is_empty() { None } else { Some(title) }
}

fn parse_slide_texts(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut sp_depth: i32 = 0;
    let mut in_txbody = false;
    let mut in_t = false;
    let mut current = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"p:sp" => {
                    sp_depth += 1;
                    current.clear();
                }
                b"p:txBody" => {
                    if sp_depth > 0 {
                        in_txbody = true;
                    }
                }
                b"a:t" => {
                    if in_txbody {
                        in_t = true;
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"p:sp" => {
                    if sp_depth > 0 {
                        sp_depth -= 1;
                        if !current.is_empty() {
                            texts.push(std::mem::take(&mut current));
                        }
                    }
                }
                b"p:txBody" => {
                    in_txbody = false;
                }
                b"a:t" => {
                    in_t = false;
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_t {
                    let s = t.unescape().map_err(|e| e.to_string())?;
                    current.push_str(&s);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
        buf.clear();
    }

    Ok(texts)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pptx_starts_with_zip_magic() {
        let pres = Presentation::new("Magic".into());
        let bytes = presentation_to_pptx(&pres).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[0..2], b"PK");
    }

    #[test]
    fn test_pptx_contains_expected_parts() {
        let mut pres = Presentation::new("Parts".into());
        pres.slides[0].add_text_box(10.0, 10.0, 400.0, 50.0, "Hello");
        let bytes = presentation_to_pptx(&pres).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slides/slide1.xml",
        ] {
            assert!(archive.by_name(name).is_ok(), "missing: {}", name);
        }
    }

    #[test]
    fn test_pptx_round_trip() {
        let mut pres = Presentation::new("Trip".into());
        // Replace default first slide content with our text.
        pres.slides[0]
            .elements
            .clear();
        pres.slides[0].add_text_box(10.0, 10.0, 400.0, 50.0, "First slide text");

        let slide2 = pres.add_slide(SlideLayout::Blank);
        slide2.add_text_box(20.0, 20.0, 300.0, 40.0, "Second slide text");
        // The default constructor adds a TitleSlide; remove it so we have exactly 2.
        let bytes = presentation_to_pptx(&pres).unwrap();

        let parsed = pptx_to_presentation(&bytes).unwrap();
        assert_eq!(parsed.slides.len(), pres.slides.len());
        assert_eq!(parsed.slides.len(), 2);

        let first_texts: Vec<String> = parsed.slides[0]
            .elements
            .iter()
            .filter_map(|e| match e {
                SlideElement::TextBox(tb) => Some(
                    tb.content
                        .iter()
                        .map(|r| r.text.clone())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        assert!(
            first_texts.iter().any(|t| t.contains("First slide text")),
            "first slide text not preserved, got: {:?}",
            first_texts
        );
    }

    #[test]
    fn test_pptx_title_round_trip() {
        let pres = Presentation::new("My Title".into());
        let bytes = presentation_to_pptx(&pres).unwrap();
        let parsed = pptx_to_presentation(&bytes).unwrap();
        assert_eq!(parsed.title, "My Title");
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<a&b>"), "&lt;a&amp;b&gt;");
    }
}
