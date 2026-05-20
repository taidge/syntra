//! Markdown export for Writer documents.
//!
//! Converts a [`WriterDocument`] into a CommonMark-compatible Markdown string.

use contrix_doc_engine::writer::{
    Block, Blockquote, CodeBlock, Heading, ImageBlock, List, ListItem, Paragraph, Table, TextRun,
    WriterDocument,
};

/// Convert a [`WriterDocument`] to a Markdown string.
pub fn writer_to_markdown(doc: &WriterDocument) -> String {
    let mut md = String::with_capacity(2048);

    // Emit the document title as a top-level heading if non-empty.
    if !doc.title.is_empty() {
        md.push_str(&format!("# {}\n\n", doc.title));
    }

    for block in &doc.body {
        block_to_markdown(block, &mut md);
        md.push('\n');
    }

    // Trim trailing whitespace but keep a single trailing newline.
    let trimmed = md.trim_end().to_string();
    if trimmed.is_empty() {
        return trimmed;
    }
    format!("{trimmed}\n")
}

/// Render a single block-level element into `out`.
fn block_to_markdown(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => paragraph_to_md(p, out),
        Block::Heading(h) => heading_to_md(h, out),
        Block::List(list) => list_to_md(list, out),
        Block::Table(table) => table_to_md(table, out),
        Block::Image(img) => image_to_md(img, out),
        Block::CodeBlock(cb) => code_block_to_md(cb, out),
        Block::Blockquote(bq) => blockquote_to_md(bq, out),
        Block::HorizontalRule => out.push_str("---\n"),
        Block::PageBreak => out.push_str("---\n"),
    }
}

fn paragraph_to_md(p: &Paragraph, out: &mut String) {
    for run in &p.runs {
        text_run_to_md(run, out);
    }
    out.push('\n');
}

fn heading_to_md(h: &Heading, out: &mut String) {
    let level = h.level.clamp(1, 6) as usize;
    for _ in 0..level {
        out.push('#');
    }
    out.push(' ');
    for run in &h.runs {
        text_run_to_md(run, out);
    }
    out.push('\n');
}

fn list_to_md(list: &List, out: &mut String) {
    for (i, item) in list.items.iter().enumerate() {
        list_item_to_md(item, list.ordered, i + 1, 0, out);
    }
}

fn list_item_to_md(item: &ListItem, ordered: bool, number: usize, depth: usize, out: &mut String) {
    // Indent nested items.
    for _ in 0..depth {
        out.push_str("    ");
    }

    if ordered {
        out.push_str(&format!("{number}. "));
    } else {
        out.push_str("- ");
    }

    for run in &item.runs {
        text_run_to_md(run, out);
    }
    out.push('\n');

    for (i, child) in item.children.iter().enumerate() {
        list_item_to_md(child, ordered, i + 1, depth + 1, out);
    }
}

fn table_to_md(table: &Table, out: &mut String) {
    if table.rows.is_empty() {
        return;
    }

    // Determine column count from the widest row.
    let col_count = table.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);

    if col_count == 0 {
        return;
    }

    // Collect all cell texts for width calculation.
    let cell_texts: Vec<Vec<String>> = table
        .rows
        .iter()
        .map(|row| {
            let mut cells: Vec<String> = row
                .cells
                .iter()
                .map(|cell| {
                    let mut s = String::new();
                    for b in &cell.blocks {
                        block_inline_text(b, &mut s);
                    }
                    s
                })
                .collect();
            // Pad to col_count.
            cells.resize(col_count, String::new());
            cells
        })
        .collect();

    // Compute column widths (min 3 for the separator line).
    let mut widths: Vec<usize> = vec![3; col_count];
    for row_texts in &cell_texts {
        for (j, text) in row_texts.iter().enumerate() {
            widths[j] = widths[j].max(text.len());
        }
    }

    // First row = header.
    out.push('|');
    for (j, text) in cell_texts[0].iter().enumerate() {
        out.push_str(&format!(" {:<width$} |", text, width = widths[j]));
    }
    out.push('\n');

    // Separator.
    out.push('|');
    for w in &widths {
        out.push(' ');
        for _ in 0..*w {
            out.push('-');
        }
        out.push_str(" |");
    }
    out.push('\n');

    // Data rows.
    for row_texts in cell_texts.iter().skip(1) {
        out.push('|');
        for (j, text) in row_texts.iter().enumerate() {
            out.push_str(&format!(" {:<width$} |", text, width = widths[j]));
        }
        out.push('\n');
    }
}

/// Extract inline plain text from a block (used for table cells in Markdown
/// where we cannot nest full block-level elements).
fn block_inline_text(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => {
            for run in &p.runs {
                text_run_to_md(run, out);
            }
        }
        Block::Heading(h) => {
            for run in &h.runs {
                text_run_to_md(run, out);
            }
        }
        Block::CodeBlock(cb) => {
            out.push('`');
            out.push_str(&cb.code);
            out.push('`');
        }
        _ => {} // Complex blocks inside table cells are simplified away.
    }
}

fn image_to_md(img: &ImageBlock, out: &mut String) {
    out.push_str(&format!("![{}]({})\n", img.alt_text, img.url));
}

fn code_block_to_md(cb: &CodeBlock, out: &mut String) {
    out.push_str("```");
    if let Some(lang) = &cb.language {
        out.push_str(lang);
    }
    out.push('\n');
    out.push_str(&cb.code);
    if !cb.code.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");
}

fn blockquote_to_md(bq: &Blockquote, out: &mut String) {
    // Render inner blocks into a temporary buffer, then prefix each line with "> ".
    let mut inner = String::new();
    for b in &bq.blocks {
        block_to_markdown(b, &mut inner);
    }
    for line in inner.lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
}

/// Render a single styled [`TextRun`] as inline Markdown.
fn text_run_to_md(run: &TextRun, out: &mut String) {
    if run.text.is_empty() {
        return;
    }

    // Determine which markers to wrap around the text.  Order matters:
    // outermost first so they nest correctly.
    let mut prefix = String::new();
    let mut suffix = String::new();

    if run.bold && run.italic {
        prefix.push_str("***");
        suffix.insert_str(0, "***");
    } else {
        if run.bold {
            prefix.push_str("**");
            suffix.insert_str(0, "**");
        }
        if run.italic {
            prefix.push('*');
            suffix.insert_str(0, "*");
        }
    }

    if run.strikethrough {
        prefix.push_str("~~");
        suffix.insert_str(0, "~~");
    }

    // Inline code: we do not nest other formatting inside backticks, so we
    // check this last and only when no other formatting is active.
    let is_code = prefix.is_empty() && suffix.is_empty() && run.link.is_none();

    if let Some(href) = &run.link {
        out.push('[');
        out.push_str(&prefix);
        out.push_str(&run.text);
        out.push_str(&suffix);
        out.push_str(&format!("]({})", href));
    } else if is_code && run.font_family.as_deref() == Some("monospace") {
        out.push('`');
        out.push_str(&run.text);
        out.push('`');
    } else {
        out.push_str(&prefix);
        out.push_str(&run.text);
        out.push_str(&suffix);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use contrix_doc_engine::writer::WriterDocument;

    #[test]
    fn empty_document() {
        let doc = WriterDocument::new(String::new());
        let md = writer_to_markdown(&doc);
        assert!(md.is_empty() || md.trim().is_empty());
    }

    #[test]
    fn heading_and_paragraph() {
        let mut doc = WriterDocument::new(String::new());
        doc.add_heading("Title", 2);
        doc.add_paragraph("Hello world.");
        let md = writer_to_markdown(&doc);
        assert!(md.contains("## Title"));
        assert!(md.contains("Hello world."));
    }

    #[test]
    fn bold_italic_rendering() {
        let run = TextRun {
            text: "both".to_string(),
            bold: true,
            italic: true,
            ..TextRun::default()
        };
        let mut out = String::new();
        text_run_to_md(&run, &mut out);
        assert_eq!(out, "***both***");
    }
}
