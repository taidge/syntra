//! HTML export for Writer documents.
//!
//! Converts a [`WriterDocument`] into a self-contained HTML string.  The
//! output includes inline styles derived from the document's formatting
//! properties so it can be viewed in any browser without external CSS.

use syntra_doc_engine::writer::{
    Block, Blockquote, CodeBlock, Heading, ImageBlock, List, ListItem, Paragraph, Table, TextRun,
    WriterDocument,
};

/// Convert a [`WriterDocument`] to a complete HTML string.
pub fn writer_to_html(doc: &WriterDocument) -> String {
    let mut html = String::with_capacity(4096);
    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str(&format!("<title>{}</title>\n", escape_html(&doc.title)));
    html.push_str("<style>\n");
    html.push_str(
        "body { font-family: sans-serif; max-width: 800px; margin: 0 auto; padding: 1em; }\n",
    );
    html.push_str("table { border-collapse: collapse; width: 100%; margin: 1em 0; }\n");
    html.push_str("td, th { border: 1px solid #ccc; padding: 0.4em 0.6em; }\n");
    html.push_str("pre { background: #f5f5f5; padding: 1em; overflow-x: auto; }\n");
    html.push_str(
        "blockquote { border-left: 4px solid #ccc; margin: 1em 0; padding: 0.5em 1em; }\n",
    );
    html.push_str("img { max-width: 100%; height: auto; }\n");
    html.push_str("</style>\n");
    html.push_str("</head>\n<body>\n");

    for block in &doc.body {
        block_to_html(block, &mut html);
    }

    html.push_str("</body>\n</html>\n");
    html
}

/// Render a single block-level element into `out`.
fn block_to_html(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => paragraph_to_html(p, out),
        Block::Heading(h) => heading_to_html(h, out),
        Block::List(list) => list_to_html(list, out),
        Block::Table(table) => table_to_html(table, out),
        Block::Image(img) => image_to_html(img, out),
        Block::CodeBlock(cb) => code_block_to_html(cb, out),
        Block::Blockquote(bq) => blockquote_to_html(bq, out),
        Block::HorizontalRule => out.push_str("<hr>\n"),
        Block::PageBreak => {
            out.push_str("<div style=\"page-break-after: always;\"></div>\n");
        }
    }
}

fn paragraph_to_html(p: &Paragraph, out: &mut String) {
    out.push_str("<p>");
    for run in &p.runs {
        text_run_to_html(run, out);
    }
    out.push_str("</p>\n");
}

fn heading_to_html(h: &Heading, out: &mut String) {
    let level = h.level.clamp(1, 6);
    out.push_str(&format!("<h{level}>"));
    for run in &h.runs {
        text_run_to_html(run, out);
    }
    out.push_str(&format!("</h{level}>\n"));
}

fn list_to_html(list: &List, out: &mut String) {
    let tag = if list.ordered { "ol" } else { "ul" };
    out.push_str(&format!("<{tag}>\n"));
    for item in &list.items {
        list_item_to_html(item, list.ordered, out);
    }
    out.push_str(&format!("</{tag}>\n"));
}

fn list_item_to_html(item: &ListItem, ordered: bool, out: &mut String) {
    out.push_str("<li>");
    for run in &item.runs {
        text_run_to_html(run, out);
    }
    if !item.children.is_empty() {
        let tag = if ordered { "ol" } else { "ul" };
        out.push_str(&format!("\n<{tag}>\n"));
        for child in &item.children {
            list_item_to_html(child, ordered, out);
        }
        out.push_str(&format!("</{tag}>\n"));
    }
    out.push_str("</li>\n");
}

fn table_to_html(table: &Table, out: &mut String) {
    out.push_str("<table>\n");
    for row in &table.rows {
        out.push_str("<tr>");
        for cell in &row.cells {
            let mut attrs = String::new();
            if cell.col_span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", cell.col_span));
            }
            if cell.row_span > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
            }
            if let Some(bg) = &cell.background_color {
                attrs.push_str(&format!(" style=\"background-color: {bg}\""));
            }
            out.push_str(&format!("<td{attrs}>"));
            for b in &cell.blocks {
                block_to_html(b, out);
            }
            out.push_str("</td>");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

fn image_to_html(img: &ImageBlock, out: &mut String) {
    let mut style = String::new();
    if let Some(w) = img.width {
        style.push_str(&format!("width:{w}px;"));
    }
    if let Some(h) = img.height {
        style.push_str(&format!("height:{h}px;"));
    }
    let style_attr = if style.is_empty() {
        String::new()
    } else {
        format!(" style=\"{style}\"")
    };
    out.push_str(&format!(
        "<img src=\"{}\" alt=\"{}\"{style_attr}>\n",
        escape_html(&img.url),
        escape_html(&img.alt_text),
    ));
}

fn code_block_to_html(cb: &CodeBlock, out: &mut String) {
    let lang_attr = match &cb.language {
        Some(lang) => format!(" class=\"language-{}\"", escape_html(lang)),
        None => String::new(),
    };
    out.push_str(&format!(
        "<pre><code{lang_attr}>{}</code></pre>\n",
        escape_html(&cb.code)
    ));
}

fn blockquote_to_html(bq: &Blockquote, out: &mut String) {
    out.push_str("<blockquote>\n");
    for b in &bq.blocks {
        block_to_html(b, out);
    }
    out.push_str("</blockquote>\n");
}

/// Render a single styled [`TextRun`] as inline HTML.
fn text_run_to_html(run: &TextRun, out: &mut String) {
    if run.text.is_empty() {
        return;
    }

    // Build an inline style attribute from the run's formatting.
    let mut style_parts: Vec<String> = Vec::new();
    if let Some(color) = &run.color {
        style_parts.push(format!("color:{color}"));
    }
    if let Some(bg) = &run.background_color {
        style_parts.push(format!("background-color:{bg}"));
    }
    if let Some(family) = &run.font_family {
        style_parts.push(format!("font-family:{family}"));
    }
    if let Some(size) = run.font_size {
        style_parts.push(format!("font-size:{size}pt"));
    }

    let mut open_tags: Vec<&str> = Vec::new();

    // Wrap in a <span> if there are any inline styles.
    if !style_parts.is_empty() {
        let style_str = style_parts.join(";");
        out.push_str(&format!("<span style=\"{style_str}\">"));
        open_tags.push("span");
    }

    if run.bold {
        out.push_str("<strong>");
        open_tags.push("strong");
    }
    if run.italic {
        out.push_str("<em>");
        open_tags.push("em");
    }
    if run.underline {
        out.push_str("<u>");
        open_tags.push("u");
    }
    if run.strikethrough {
        out.push_str("<s>");
        open_tags.push("s");
    }
    if run.superscript {
        out.push_str("<sup>");
        open_tags.push("sup");
    }
    if run.subscript {
        out.push_str("<sub>");
        open_tags.push("sub");
    }

    // Links wrap the innermost text.
    let has_link = run.link.is_some();
    if let Some(href) = &run.link {
        out.push_str(&format!("<a href=\"{}\">", escape_html(href)));
    }

    out.push_str(&escape_html(&run.text));

    if has_link {
        out.push_str("</a>");
    }

    // Close tags in reverse order.
    for tag in open_tags.iter().rev() {
        out.push_str(&format!("</{tag}>"));
    }
}

/// Escape characters that are special in HTML.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use syntra_doc_engine::writer::WriterDocument;

    #[test]
    fn empty_document_produces_valid_html() {
        let doc = WriterDocument::new("Test".to_string());
        let html = writer_to_html(&doc);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Test</title>"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn paragraph_and_heading() {
        let mut doc = WriterDocument::new("Demo".to_string());
        doc.add_heading("Hello", 1);
        doc.add_paragraph("World");
        let html = writer_to_html(&doc);
        assert!(html.contains("<h1>Hello</h1>"));
        assert!(html.contains("<p>World</p>"));
    }

    #[test]
    fn escape_html_special_chars() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
    }
}
