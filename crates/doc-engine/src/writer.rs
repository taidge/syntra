use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core document
// ---------------------------------------------------------------------------

/// A Writer document — rich text document similar to Word / Google Docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterDocument {
    pub id: Uuid,
    pub title: String,
    pub body: Vec<Block>,
    pub page_settings: PageSettings,
    pub styles: Vec<Style>,
    pub metadata: DocumentMetadata,
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

/// Top-level content block inside a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Block {
    Paragraph(Paragraph),
    Heading(Heading),
    List(List),
    Table(Table),
    Image(ImageBlock),
    HorizontalRule,
    PageBreak,
    CodeBlock(CodeBlock),
    Blockquote(Blockquote),
}

/// A paragraph — one or more styled text runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub id: Uuid,
    pub runs: Vec<TextRun>,
    pub alignment: Alignment,
    pub indent_level: u8,
    pub spacing: Spacing,
}

/// A heading (h1–h6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heading {
    pub id: Uuid,
    pub level: u8,
    pub runs: Vec<TextRun>,
}

/// An ordered or unordered list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct List {
    pub id: Uuid,
    pub ordered: bool,
    pub items: Vec<ListItem>,
}

/// A single item inside a [`List`]; may contain nested children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItem {
    pub runs: Vec<TextRun>,
    pub children: Vec<ListItem>,
}

/// A table (rows × columns).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub id: Uuid,
    pub rows: Vec<TableRow>,
    pub column_widths: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableCell {
    pub blocks: Vec<Block>,
    pub col_span: u32,
    pub row_span: u32,
    pub background_color: Option<String>,
}

/// An inline/block image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageBlock {
    pub id: Uuid,
    pub url: String,
    pub alt_text: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub alignment: Alignment,
}

/// A fenced code block with optional language tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    pub id: Uuid,
    pub language: Option<String>,
    pub code: String,
}

/// A blockquote containing nested blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blockquote {
    pub id: Uuid,
    pub blocks: Vec<Block>,
}

// ---------------------------------------------------------------------------
// Inline styling
// ---------------------------------------------------------------------------

/// A contiguous run of text sharing the same formatting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub link: Option<String>,
    pub superscript: bool,
    pub subscript: bool,
}

impl TextRun {
    /// Create a plain (unformatted) text run.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Page & style settings
// ---------------------------------------------------------------------------

/// Physical page dimensions and margins (in millimetres).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSettings {
    pub width_mm: f32,
    pub height_mm: f32,
    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub orientation: Orientation,
}

impl Default for PageSettings {
    /// ISO A4 (210 × 297 mm) with 25.4 mm (1 in) margins, portrait.
    fn default() -> Self {
        Self {
            width_mm: 210.0,
            height_mm: 297.0,
            margin_top: 25.4,
            margin_right: 25.4,
            margin_bottom: 25.4,
            margin_left: 25.4,
            orientation: Orientation::Portrait,
        }
    }
}

/// Page orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Text alignment within a block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Vertical spacing around a block (in points).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Spacing {
    pub before: f32,
    pub after: f32,
    pub line_height: f32,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            before: 0.0,
            after: 8.0,
            line_height: 1.15,
        }
    }
}

/// A named text style (e.g. "Normal", "Heading 1").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub color: Option<String>,
}

/// Document-level metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub author: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    pub word_count: usize,
    pub char_count: usize,
    pub page_count: usize,
}

impl Default for DocumentMetadata {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            author: None,
            created_at: now,
            modified_at: now,
            word_count: 0,
            char_count: 0,
            page_count: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// WriterDocument impls
// ---------------------------------------------------------------------------

impl Default for WriterDocument {
    /// Create an empty document with A4 page settings.
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            title: String::new(),
            body: Vec::new(),
            page_settings: PageSettings::default(),
            styles: Vec::new(),
            metadata: DocumentMetadata::default(),
        }
    }
}

impl WriterDocument {
    /// Create a new, empty document with the given title.
    pub fn new(title: String) -> Self {
        Self {
            title,
            ..Default::default()
        }
    }

    /// Append a simple paragraph with plain text.
    pub fn add_paragraph(&mut self, text: &str) -> &mut Self {
        self.body.push(Block::Paragraph(Paragraph {
            id: Uuid::new_v4(),
            runs: vec![TextRun::plain(text)],
            alignment: Alignment::default(),
            indent_level: 0,
            spacing: Spacing::default(),
        }));
        self
    }

    /// Append a heading at the given level (1–6) with plain text.
    pub fn add_heading(&mut self, text: &str, level: u8) -> &mut Self {
        let level = level.clamp(1, 6);
        self.body.push(Block::Heading(Heading {
            id: Uuid::new_v4(),
            level,
            runs: vec![TextRun::plain(text)],
        }));
        self
    }

    /// Count every word across all blocks (recursively).
    pub fn word_count(&self) -> usize {
        self.body.iter().map(block_word_count).sum()
    }

    /// Extract plain text from every block, separated by newlines.
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.body {
            block_to_plain_text(block, &mut out);
        }
        // Trim the trailing newline if present
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }

    /// Find and replace text across the entire document.
    ///
    /// Returns the number of replacements made.
    pub fn find_replace(&mut self, find: &str, replace: &str) -> u32 {
        let mut count = 0u32;
        for block in &mut self.body {
            count += block_find_replace(block, find, replace);
        }
        count
    }

    /// Get all headings in the document as `(level, text)` pairs — useful
    /// for building a table of contents.
    pub fn get_headings(&self) -> Vec<(u8, String)> {
        let mut headings = Vec::new();
        for block in &self.body {
            collect_headings(block, &mut headings);
        }
        headings
    }

    /// Insert a block at a specific index in the document body.
    ///
    /// If `index` is greater than the body length the block is appended.
    pub fn insert_block(&mut self, index: usize, block: Block) {
        if index >= self.body.len() {
            self.body.push(block);
        } else {
            self.body.insert(index, block);
        }
    }

    /// Remove the block at `index` and return it, or `None` if out of bounds.
    pub fn remove_block(&mut self, index: usize) -> Option<Block> {
        if index < self.body.len() {
            Some(self.body.remove(index))
        } else {
            None
        }
    }

    /// Move a block from position `from` to position `to`.
    ///
    /// Both indices must be within bounds; otherwise this is a no-op.
    pub fn move_block(&mut self, from: usize, to: usize) {
        if from >= self.body.len() || to >= self.body.len() || from == to {
            return;
        }
        let block = self.body.remove(from);
        self.body.insert(to, block);
    }

    /// Compute comprehensive document statistics.
    pub fn statistics(&self) -> DocumentStats {
        let mut stats = DocumentStats {
            block_count: self.body.len(),
            paragraph_count: 0,
            heading_count: 0,
            list_count: 0,
            table_count: 0,
            image_count: 0,
            word_count: 0,
            char_count: 0,
            char_count_no_spaces: 0,
        };

        for block in &self.body {
            accumulate_stats(block, &mut stats);
        }

        stats
    }

    /// Split the document at block index `index`.
    ///
    /// Returns two new documents: the first contains blocks `[0, index)`,
    /// the second contains blocks `[index, len)`.  Both inherit the page
    /// settings and styles of the original.
    pub fn split_at(&self, index: usize) -> (WriterDocument, WriterDocument) {
        let clamped = index.min(self.body.len());
        let first_body = self.body[..clamped].to_vec();
        let second_body = self.body[clamped..].to_vec();

        let first = WriterDocument {
            id: Uuid::new_v4(),
            title: format!("{} (part 1)", self.title),
            body: first_body,
            page_settings: self.page_settings.clone(),
            styles: self.styles.clone(),
            metadata: DocumentMetadata::default(),
        };

        let second = WriterDocument {
            id: Uuid::new_v4(),
            title: format!("{} (part 2)", self.title),
            body: second_body,
            page_settings: self.page_settings.clone(),
            styles: self.styles.clone(),
            metadata: DocumentMetadata::default(),
        };

        (first, second)
    }

    /// Merge another document's blocks into this document.
    pub fn merge(&mut self, other: &WriterDocument) {
        self.body.extend(other.body.clone());
    }

    /// Extract plain text from blocks in the range `[start, end)`.
    pub fn extract_text_range(&self, start: usize, end: usize) -> String {
        let clamped_start = start.min(self.body.len());
        let clamped_end = end.min(self.body.len());

        let mut out = String::new();
        for block in &self.body[clamped_start..clamped_end] {
            block_to_plain_text(block, &mut out);
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

/// Comprehensive statistics about a writer document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentStats {
    pub block_count: usize,
    pub paragraph_count: usize,
    pub heading_count: usize,
    pub list_count: usize,
    pub table_count: usize,
    pub image_count: usize,
    pub word_count: usize,
    pub char_count: usize,
    pub char_count_no_spaces: usize,
}

// ---------------------------------------------------------------------------
// Helpers (recursive block visitors)
// ---------------------------------------------------------------------------

fn runs_word_count(runs: &[TextRun]) -> usize {
    runs.iter().map(|r| r.text.split_whitespace().count()).sum()
}

fn runs_plain_text(runs: &[TextRun]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect::<String>()
}

fn block_word_count(block: &Block) -> usize {
    match block {
        Block::Paragraph(p) => runs_word_count(&p.runs),
        Block::Heading(h) => runs_word_count(&h.runs),
        Block::List(list) => list.items.iter().map(list_item_word_count).sum(),
        Block::Table(table) => table
            .rows
            .iter()
            .flat_map(|row| &row.cells)
            .flat_map(|cell| &cell.blocks)
            .map(block_word_count)
            .sum(),
        Block::CodeBlock(cb) => cb.code.split_whitespace().count(),
        Block::Blockquote(bq) => bq.blocks.iter().map(block_word_count).sum(),
        Block::Image(_) | Block::HorizontalRule | Block::PageBreak => 0,
    }
}

fn list_item_word_count(item: &ListItem) -> usize {
    let own: usize = runs_word_count(&item.runs);
    let children: usize = item.children.iter().map(list_item_word_count).sum();
    own + children
}

fn block_to_plain_text(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(p) => {
            out.push_str(&runs_plain_text(&p.runs));
            out.push('\n');
        }
        Block::Heading(h) => {
            out.push_str(&runs_plain_text(&h.runs));
            out.push('\n');
        }
        Block::List(list) => {
            for item in &list.items {
                list_item_to_plain_text(item, out, 0);
            }
        }
        Block::Table(table) => {
            for row in &table.rows {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .map(|cell| {
                        let mut s = String::new();
                        for b in &cell.blocks {
                            block_to_plain_text(b, &mut s);
                        }
                        s.trim_end().to_owned()
                    })
                    .collect();
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
        }
        Block::CodeBlock(cb) => {
            out.push_str(&cb.code);
            out.push('\n');
        }
        Block::Blockquote(bq) => {
            for b in &bq.blocks {
                block_to_plain_text(b, out);
            }
        }
        Block::Image(img) => {
            if !img.alt_text.is_empty() {
                out.push_str(&img.alt_text);
                out.push('\n');
            }
        }
        Block::HorizontalRule => {
            out.push_str("---\n");
        }
        Block::PageBreak => {}
    }
}

fn list_item_to_plain_text(item: &ListItem, out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(&runs_plain_text(&item.runs));
    out.push('\n');
    for child in &item.children {
        list_item_to_plain_text(child, out, depth + 1);
    }
}

// ---------------------------------------------------------------------------
// Find-replace helpers
// ---------------------------------------------------------------------------

fn runs_find_replace(runs: &mut [TextRun], find: &str, replace: &str) -> u32 {
    let mut count = 0u32;
    for run in runs.iter_mut() {
        let matches = run.text.matches(find).count() as u32;
        if matches > 0 {
            run.text = run.text.replace(find, replace);
            count += matches;
        }
    }
    count
}

fn list_item_find_replace(item: &mut ListItem, find: &str, replace: &str) -> u32 {
    let mut count = runs_find_replace(&mut item.runs, find, replace);
    for child in &mut item.children {
        count += list_item_find_replace(child, find, replace);
    }
    count
}

fn block_find_replace(block: &mut Block, find: &str, replace: &str) -> u32 {
    match block {
        Block::Paragraph(p) => runs_find_replace(&mut p.runs, find, replace),
        Block::Heading(h) => runs_find_replace(&mut h.runs, find, replace),
        Block::List(list) => list
            .items
            .iter_mut()
            .map(|item| list_item_find_replace(item, find, replace))
            .sum(),
        Block::Table(table) => table
            .rows
            .iter_mut()
            .flat_map(|row| &mut row.cells)
            .flat_map(|cell| &mut cell.blocks)
            .map(|b| block_find_replace(b, find, replace))
            .sum(),
        Block::CodeBlock(cb) => {
            let matches = cb.code.matches(find).count() as u32;
            if matches > 0 {
                cb.code = cb.code.replace(find, replace);
            }
            matches
        }
        Block::Blockquote(bq) => bq
            .blocks
            .iter_mut()
            .map(|b| block_find_replace(b, find, replace))
            .sum(),
        Block::Image(_) | Block::HorizontalRule | Block::PageBreak => 0,
    }
}

// ---------------------------------------------------------------------------
// Heading collection helpers
// ---------------------------------------------------------------------------

fn collect_headings(block: &Block, headings: &mut Vec<(u8, String)>) {
    match block {
        Block::Heading(h) => {
            headings.push((h.level, runs_plain_text(&h.runs)));
        }
        Block::Blockquote(bq) => {
            for b in &bq.blocks {
                collect_headings(b, headings);
            }
        }
        Block::Table(table) => {
            for row in &table.rows {
                for cell in &row.cells {
                    for b in &cell.blocks {
                        collect_headings(b, headings);
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Statistics helpers
// ---------------------------------------------------------------------------

fn runs_char_counts(runs: &[TextRun]) -> (usize, usize) {
    let mut total = 0usize;
    let mut no_spaces = 0usize;
    for run in runs {
        total += run.text.len();
        no_spaces += run.text.chars().filter(|c| !c.is_whitespace()).count();
    }
    (total, no_spaces)
}

fn list_item_stats(item: &ListItem, stats: &mut DocumentStats) {
    let wc = runs_word_count(&item.runs);
    let (cc, ccns) = runs_char_counts(&item.runs);
    stats.word_count += wc;
    stats.char_count += cc;
    stats.char_count_no_spaces += ccns;
    for child in &item.children {
        list_item_stats(child, stats);
    }
}

fn accumulate_stats(block: &Block, stats: &mut DocumentStats) {
    match block {
        Block::Paragraph(p) => {
            stats.paragraph_count += 1;
            stats.word_count += runs_word_count(&p.runs);
            let (cc, ccns) = runs_char_counts(&p.runs);
            stats.char_count += cc;
            stats.char_count_no_spaces += ccns;
        }
        Block::Heading(h) => {
            stats.heading_count += 1;
            stats.word_count += runs_word_count(&h.runs);
            let (cc, ccns) = runs_char_counts(&h.runs);
            stats.char_count += cc;
            stats.char_count_no_spaces += ccns;
        }
        Block::List(list) => {
            stats.list_count += 1;
            for item in &list.items {
                list_item_stats(item, stats);
            }
        }
        Block::Table(table) => {
            stats.table_count += 1;
            for row in &table.rows {
                for cell in &row.cells {
                    for b in &cell.blocks {
                        accumulate_stats(b, stats);
                    }
                }
            }
        }
        Block::Image(_) => {
            stats.image_count += 1;
        }
        Block::CodeBlock(cb) => {
            stats.word_count += cb.code.split_whitespace().count();
            stats.char_count += cb.code.len();
            stats.char_count_no_spaces += cb.code.chars().filter(|c| !c.is_whitespace()).count();
        }
        Block::Blockquote(bq) => {
            for b in &bq.blocks {
                accumulate_stats(b, stats);
            }
        }
        Block::HorizontalRule | Block::PageBreak => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_doc() -> WriterDocument {
        let mut doc = WriterDocument::new("Test Doc".into());
        doc.add_heading("Introduction", 1);
        doc.add_paragraph("Hello world, this is a test document.");
        doc.add_heading("Details", 2);
        doc.add_paragraph("More content here with some words.");
        doc.add_paragraph("Final paragraph.");
        doc
    }

    #[test]
    fn test_find_replace() {
        let mut doc = make_sample_doc();
        let count = doc.find_replace("world", "universe");
        assert_eq!(count, 1);
        let text = doc.to_plain_text();
        assert!(text.contains("universe"));
        assert!(!text.contains("world"));
    }

    #[test]
    fn test_find_replace_multiple() {
        let mut doc = WriterDocument::new("Test".into());
        doc.add_paragraph("foo bar foo baz foo");
        let count = doc.find_replace("foo", "qux");
        assert_eq!(count, 3);
        let text = doc.to_plain_text();
        assert!(!text.contains("foo"));
        assert_eq!(text.matches("qux").count(), 3);
    }

    #[test]
    fn test_get_headings() {
        let doc = make_sample_doc();
        let headings = doc.get_headings();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0], (1, "Introduction".to_string()));
        assert_eq!(headings[1], (2, "Details".to_string()));
    }

    #[test]
    fn test_insert_block() {
        let mut doc = make_sample_doc();
        let original_len = doc.body.len();
        doc.insert_block(
            1,
            Block::Paragraph(Paragraph {
                id: Uuid::new_v4(),
                runs: vec![TextRun::plain("Inserted")],
                alignment: Alignment::default(),
                indent_level: 0,
                spacing: Spacing::default(),
            }),
        );
        assert_eq!(doc.body.len(), original_len + 1);
        // The new block should be at index 1.
        if let Block::Paragraph(p) = &doc.body[1] {
            assert_eq!(p.runs[0].text, "Inserted");
        } else {
            panic!("Expected Paragraph at index 1");
        }
    }

    #[test]
    fn test_insert_block_beyond_end() {
        let mut doc = make_sample_doc();
        let len = doc.body.len();
        doc.insert_block(
            100,
            Block::Paragraph(Paragraph {
                id: Uuid::new_v4(),
                runs: vec![TextRun::plain("Appended")],
                alignment: Alignment::default(),
                indent_level: 0,
                spacing: Spacing::default(),
            }),
        );
        assert_eq!(doc.body.len(), len + 1);
    }

    #[test]
    fn test_remove_block() {
        let mut doc = make_sample_doc();
        let original_len = doc.body.len();
        let removed = doc.remove_block(0);
        assert!(removed.is_some());
        assert_eq!(doc.body.len(), original_len - 1);
    }

    #[test]
    fn test_remove_block_out_of_bounds() {
        let mut doc = make_sample_doc();
        let removed = doc.remove_block(999);
        assert!(removed.is_none());
    }

    #[test]
    fn test_move_block() {
        let mut doc = make_sample_doc();
        // Move the first block (Heading "Introduction") to position 2.
        doc.move_block(0, 2);
        if let Block::Heading(h) = &doc.body[2] {
            assert_eq!(runs_plain_text(&h.runs), "Introduction");
        } else {
            panic!("Expected heading at index 2");
        }
    }

    #[test]
    fn test_move_block_noop() {
        let mut doc = make_sample_doc();
        let text_before = doc.to_plain_text();
        doc.move_block(0, 0);
        assert_eq!(doc.to_plain_text(), text_before);
    }

    #[test]
    fn test_statistics() {
        let doc = make_sample_doc();
        let stats = doc.statistics();
        assert_eq!(stats.block_count, 5);
        assert_eq!(stats.heading_count, 2);
        assert_eq!(stats.paragraph_count, 3);
        assert!(stats.word_count > 0);
        assert!(stats.char_count > 0);
        assert!(stats.char_count_no_spaces > 0);
        assert!(stats.char_count_no_spaces <= stats.char_count);
    }

    #[test]
    fn test_split_at() {
        let doc = make_sample_doc();
        let (first, second) = doc.split_at(2);
        assert_eq!(first.body.len(), 2);
        assert_eq!(second.body.len(), 3);
        assert!(first.title.contains("part 1"));
        assert!(second.title.contains("part 2"));
    }

    #[test]
    fn test_split_at_zero() {
        let doc = make_sample_doc();
        let (first, second) = doc.split_at(0);
        assert_eq!(first.body.len(), 0);
        assert_eq!(second.body.len(), doc.body.len());
    }

    #[test]
    fn test_split_at_end() {
        let doc = make_sample_doc();
        let len = doc.body.len();
        let (first, second) = doc.split_at(len);
        assert_eq!(first.body.len(), len);
        assert_eq!(second.body.len(), 0);
    }

    #[test]
    fn test_merge() {
        let mut doc1 = WriterDocument::new("Part A".into());
        doc1.add_paragraph("A content");
        let mut doc2 = WriterDocument::new("Part B".into());
        doc2.add_paragraph("B content");
        doc2.add_paragraph("B more");

        doc1.merge(&doc2);
        assert_eq!(doc1.body.len(), 3);
    }

    #[test]
    fn test_extract_text_range() {
        let mut doc = WriterDocument::new("Test".into());
        doc.add_paragraph("Line one");
        doc.add_paragraph("Line two");
        doc.add_paragraph("Line three");

        let text = doc.extract_text_range(1, 3);
        assert!(text.contains("Line two"));
        assert!(text.contains("Line three"));
        assert!(!text.contains("Line one"));
    }

    #[test]
    fn test_extract_text_range_clamped() {
        let mut doc = WriterDocument::new("Test".into());
        doc.add_paragraph("Only line");
        let text = doc.extract_text_range(0, 100);
        assert!(text.contains("Only line"));
    }
}
