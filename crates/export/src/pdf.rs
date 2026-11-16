//! Self-contained, print-ready documents for the server's isolated PDF renderer.
//!
//! Rendering never resolves a URL: images must already be embedded data. Keeping
//! layout in HTML/SVG also keeps Chinese text selectable in the resulting PDF.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

#[path = "pdf_calc.rs"]
mod calc;
#[path = "pdf_canvas.rs"]
mod canvas;
#[path = "pdf_writer.rs"]
mod writer;

const MAX_HTML_BYTES: usize = 48 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECTS: usize = 100_000;

/// CSS page dimensions accompany the HTML so a renderer can use the same size.
#[derive(Debug)]
pub struct PrintDocument {
    pub html: String,
    pub width_mm: f64,
    pub height_mm: f64,
}

/// Create print HTML for Writer, Calc, Slides, or Whiteboard content.
pub fn document_to_print_html(content: &Value, kind: &str) -> Result<PrintDocument, String> {
    let document = match kind {
        "writer" => writer::render(content),
        "calc" => calc::render(content),
        "slides" | "presentation" => canvas::slides(content),
        "whiteboard" => canvas::whiteboard(content),
        _ => Err(format!(
            "PDF export is not supported for document type '{kind}'"
        )),
    }?;
    if document.html.len() > MAX_HTML_BYTES {
        return Err("PDF print layout exceeds the rendering budget".into());
    }
    Ok(document)
}

fn html(title: &str, css: &str, body: &str, width_mm: f64, height_mm: f64) -> PrintDocument {
    PrintDocument {
        html: format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'\"><title>{}</title><style>*{{box-sizing:border-box}}html,body{{margin:0;padding:0}}body{{font-family:Arial,'Office CJK','Microsoft YaHei','Noto Sans CJK SC',sans-serif;color:#111827;font-size:11pt;-webkit-print-color-adjust:exact;print-color-adjust:exact}}img{{max-width:100%}}{css}</style></head><body>{body}</body></html>",
            escape(title)
        ),
        width_mm,
        height_mm,
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn css_string(text: &str) -> String {
    let mut result = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || character == ' ' || !character.is_ascii() {
            result.push(character);
        } else {
            result.push_str(&format!("\\{:x} ", character as u32));
        }
    }
    result
}

fn font(text: &str) -> String {
    format!(
        "'{}',Arial,'Office CJK','Microsoft YaHei','Noto Sans CJK SC',sans-serif",
        escape(&css_string(text))
    )
}

fn color(value: Option<&str>, fallback: &str) -> String {
    let Some(value) = value.map(str::trim) else {
        return fallback.into();
    };
    let hex = value.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let name = !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_alphabetic());
    let function = (value.starts_with("rgb(")
        || value.starts_with("rgba(")
        || value.starts_with("hsl(")
        || value.starts_with("hsla("))
        && value.ends_with(')')
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"(),.% +-".contains(&byte));
    if hex || name || function {
        value.into()
    } else {
        fallback.into()
    }
}

fn finite(value: f64, fallback: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn number(value: &Value, fallback: f64) -> f64 {
    finite(
        value.as_f64().unwrap_or(fallback),
        fallback,
        -1_000_000.,
        1_000_000.,
    )
}

fn image(source: &str) -> Result<String, String> {
    let (prefix, data) = source.split_once(',').ok_or_else(|| "PDF images must contain embedded PNG, JPEG or GIF data; linked images must be embedded before export".to_owned())?;
    if !matches!(
        prefix,
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/jpg;base64"
            | "data:image/gif;base64"
    ) {
        return Err("PDF export cannot load external, local-file, or SVG image resources; embed a PNG or JPEG image".into());
    }
    if data.len() > MAX_IMAGE_BYTES * 4 / 3 + 4 {
        return Err("PDF image exceeds the embedded image budget".into());
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| "PDF image contains invalid base64 data".to_owned())?;
    let valid = match prefix {
        "data:image/png;base64" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "data:image/jpeg;base64" | "data:image/jpg;base64" => {
            bytes.starts_with(&[0xff, 0xd8, 0xff])
        }
        "data:image/gif;base64" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        _ => false,
    };
    if !valid || bytes.len() > MAX_IMAGE_BYTES {
        return Err("PDF image data does not match its declared bitmap format".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| "PDF image format could not be decoded")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(
        |_| "PDF embedded image is damaged, unsupported, or exceeds safe decoding dimensions",
    )?;
    if decoded.width() == 0
        || decoded.height() == 0
        || u64::from(decoded.width()) * u64::from(decoded.height()) > 64 * 1024 * 1024
    {
        return Err("PDF image exceeds safe decoding dimensions".into());
    }
    Ok(escape(source))
}

fn budget(remaining: &mut usize) -> Result<(), String> {
    *remaining = remaining
        .checked_sub(1)
        .ok_or("PDF layout exceeds the object budget")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn print_document_rejects_network_images_and_escapes_document_content() {
        let mut document = contrix_doc_engine::WriterDocument::new("<script>文稿</script>".into());
        document.body.push(contrix_doc_engine::writer::Block::Image(
            contrix_doc_engine::writer::ImageBlock {
                id: uuid::Uuid::new_v4(),
                url: "http://127.0.0.1/private".into(),
                alt_text: "test".into(),
                width: None,
                height: None,
                alignment: contrix_doc_engine::writer::Alignment::Left,
            },
        ));
        assert!(
            document_to_print_html(&serde_json::to_value(&document).unwrap(), "writer")
                .unwrap_err()
                .contains("embedded")
        );
        document.body.clear();
        document
            .body
            .push(contrix_doc_engine::writer::Block::Paragraph(
                contrix_doc_engine::writer::Paragraph {
                    id: uuid::Uuid::new_v4(),
                    runs: vec![contrix_doc_engine::writer::TextRun::plain(
                        "<script>alert(1)</script> 中文",
                    )],
                    alignment: Default::default(),
                    indent_level: 0,
                    spacing: Default::default(),
                },
            ));
        let html = document_to_print_html(&serde_json::to_value(&document).unwrap(), "writer")
            .unwrap()
            .html;
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;") && html.contains("中文"));
        assert!(html.contains("default-src 'none'"));
        assert!(!color(Some("red;background:url(file:///x)"), "black").contains("url"));
    }

    #[test]
    fn empty_documents_still_produce_a_printable_page() {
        for (kind, content) in [
            (
                "writer",
                serde_json::to_value(contrix_doc_engine::WriterDocument::new("文稿".into()))
                    .unwrap(),
            ),
            (
                "calc",
                serde_json::to_value(contrix_doc_engine::Workbook::new("工作簿".into())).unwrap(),
            ),
            (
                "slides",
                serde_json::to_value(contrix_doc_engine::Presentation::new("演示文稿".into()))
                    .unwrap(),
            ),
            (
                "whiteboard",
                serde_json::to_value(contrix_doc_engine::Whiteboard::new("白板".into())).unwrap(),
            ),
        ] {
            let printed = document_to_print_html(&content, kind).unwrap();
            assert!(printed.html.contains("@page"), "{kind}");
            assert!(printed.width_mm > 0. && printed.height_mm > 0.);
        }
        assert!(document_to_print_html(&json!({}), "unknown").is_err());
    }
}
