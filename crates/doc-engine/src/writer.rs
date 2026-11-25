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
    #[serde(default)]
    pub professional: ProfessionalLayout,
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
    /// Imported list paragraphs have spacing/indentation just like body text.
    /// Old native lists retain their existing default appearance.
    #[serde(default)]
    pub layout: Option<ListItemLayout>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListItemLayout {
    pub spacing: Spacing,
    pub format: ParagraphFormat,
    pub label_font_family: Option<String>,
    pub label_font_size: Option<f32>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<TableCellLayout>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableCellLayout {
    /// Top, right, bottom, left cell margins in points. None inherits the table.
    pub padding: [Option<f32>; 4],
    #[serde(default)]
    pub vertical_alignment: CellVerticalAlignment,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum CellVerticalAlignment {
    #[default]
    Top,
    Center,
    Bottom,
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
#[serde(default)]
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

/// Print-oriented features that are kept separate from the main body so old
/// documents remain compatible while professional layout metadata can round
/// trip through DOCX.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfessionalLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub east_asia_language: Option<String>,
    /// Compatibility revision of an external DOCX import. Old saved models
    /// default to zero and may recover layout from their retained original.
    #[serde(default)]
    pub docx_import_revision: u8,
    /// Measured font line metrics used by Office proportional line spacing.
    /// Empty for older/native documents, which retain their existing defaults.
    #[serde(default)]
    pub font_metrics: std::collections::BTreeMap<String, FontMetrics>,
    /// Layout details not represented by the compact legacy paragraph model.
    /// Entries refer to stable block identities and remain optional in old JSON.
    #[serde(default)]
    pub paragraph_formats: Vec<ParagraphFormat>,
    pub header: String,
    pub footer: String,
    #[serde(default)]
    pub header_alignment: Option<Alignment>,
    #[serde(default)]
    pub footer_alignment: Option<Alignment>,
    pub page_numbers: bool,
    pub different_first_page: bool,
    pub sections: Vec<DocumentSection>,
    pub footnotes: Vec<DocumentNote>,
    pub endnotes: Vec<DocumentNote>,
    pub table_of_contents: bool,
    pub bibliography: Vec<BibliographyEntry>,
    pub mail_merge_fields: Vec<MailMergeField>,
    pub revisions: Vec<DocumentRevision>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ParagraphFormat {
    pub block_id: Uuid,
    pub first_line_indent_pt: Option<f32>,
    pub first_line_indent_chars: Option<f32>,
    pub left_indent_pt: Option<f32>,
    pub right_indent_pt: Option<f32>,
    pub line_height_pt: Option<f32>,
    /// DOCX atLeast spacing allows each line to expand for the text it contains.
    pub line_height_at_least: bool,
    pub numbering_label: Option<String>,
    pub alignment: Option<Alignment>,
    pub contextual_spacing: Option<bool>,
    /// Headings do not carry the legacy Paragraph's Spacing record.
    pub spacing: Option<Spacing>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    /// OS/2 reports CP932/936/949/950; Word compatibility expands the font's
    /// ascent and height to 127% before adding its external leading.
    pub cjk: bool,
}

#[derive(Debug, Clone)]
pub struct FontLineBox {
    pub font_family: String,
    pub font_size: f32,
    pub line_height_pt: f32,
    pub baseline_pt: f32,
}

/// Resolve Office auto line spacing using font metrics, in integer twips.
/// Explicit/exact line spacing wins. No metrics means the legacy renderer
/// remains in charge, instead of guessing an arbitrary font-height ratio.
pub fn office_line_box(
    runs: &[TextRun],
    spacing: Spacing,
    format: Option<&ParagraphFormat>,
    metrics: &std::collections::BTreeMap<String, FontMetrics>,
) -> Option<FontLineBox> {
    if metrics.is_empty() || format.is_some_and(|format| format.line_height_at_least) {
        return None;
    }
    let mut selected: Option<(u64, String, f32)> = None;
    let mut max_descent = 0;
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let family = run.font_family.as_deref()?;
        let raw_size = run.font_size.unwrap_or(12.);
        if !raw_size.is_finite() {
            return None;
        }
        let size = raw_size.clamp(1., 1000.);
        let normalized = family.trim().trim_matches(['\'', '"']).to_lowercase();
        let metric = metrics.get(&normalized).or_else(|| metrics.get(family))?;
        if ![metric.ascent, metric.descent, metric.line_gap]
            .iter()
            .all(|value| value.is_finite() && (0. ..=4.).contains(value))
        {
            return None;
        }
        let ascent = (metric.ascent * size * 20.).round() as u64;
        let descent = (metric.descent * size * 20.).round() as u64;
        let gap = (metric.line_gap * size * 20.).round() as u64;
        let height = if metric.cjk {
            (ascent + descent) * 127 / 100
        } else {
            ascent + descent
        } + gap;
        let ascent = if metric.cjk {
            ascent * 127 / 100
        } else {
            ascent
        } + gap;
        max_descent = max_descent.max(height - ascent);
        if selected
            .as_ref()
            .is_none_or(|(current, _, _)| ascent > *current)
        {
            selected = Some((ascent, family.to_owned(), size));
        }
    }
    let (ascent, font_family, font_size) = selected?;
    let height = ascent + max_descent;
    let spacing = format.and_then(|format| format.spacing).unwrap_or(spacing);
    let exact = format
        .and_then(|format| format.line_height_pt)
        .filter(|value| value.is_finite() && *value > 0.)
        .map(|value| value.clamp(1., 1000.));
    let line_height_pt = exact.unwrap_or_else(|| {
        let multiplier = if spacing.line_height.is_finite() {
            spacing.line_height.clamp(0.5, 10.)
        } else {
            1.15
        };
        let units = (multiplier * 240.).round() as u64;
        ((height * units / 240).max(20) as f32 / 20.).min(1000.)
    });
    let baseline_pt = if exact.is_some() {
        ascent as f32 / 20.
    } else {
        (ascent as f32 + line_height_pt * 20. - height as f32) / 20.
    }
    .clamp(0., line_height_pt);
    Some(FontLineBox {
        font_family,
        font_size,
        line_height_pt,
        baseline_pt,
    })
}

/// Drawing runs are top-aligned to the shared line box so browser font struts
/// cannot enlarge its height. Their relative offset restores one common Office
/// baseline without moving the line boxes or changing wrapping.
pub fn office_run_offset(
    run: &TextRun,
    line: &FontLineBox,
    metrics: &std::collections::BTreeMap<String, FontMetrics>,
) -> Option<f32> {
    let family = run.font_family.as_deref()?;
    let normalized = family.trim().trim_matches(['\'', '"']).to_lowercase();
    let metric = metrics.get(&normalized).or_else(|| metrics.get(family))?;
    if ![metric.ascent, metric.descent]
        .iter()
        .all(|v| v.is_finite() && (0. ..=4.).contains(v))
    {
        return None;
    }
    let size = run.font_size.unwrap_or(line.font_size);
    if !size.is_finite() {
        return None;
    }
    let size = size.clamp(1., 1000.);
    let browser_baseline =
        metric.ascent * size + (line.line_height_pt - (metric.ascent + metric.descent) * size) / 2.;
    Some((line.baseline_pt - browser_baseline).clamp(-8000., 8000.))
}

#[cfg(test)]
mod office_font_line_tests {
    use super::*;
    fn metrics() -> std::collections::BTreeMap<String, FontMetrics> {
        [
            (
                "simsun".into(),
                FontMetrics {
                    ascent: 220. / 256.,
                    descent: 36. / 256.,
                    line_gap: 36. / 256.,
                    cjk: true,
                },
            ),
            (
                "times new roman".into(),
                FontMetrics {
                    ascent: 1825. / 2048.,
                    descent: 443. / 2048.,
                    line_gap: 87. / 2048.,
                    cjk: false,
                },
            ),
        ]
        .into()
    }
    fn run(text: &str, family: &str) -> TextRun {
        TextRun {
            text: text.into(),
            font_family: Some(family.into()),
            font_size: Some(12.),
            ..Default::default()
        }
    }
    #[test]
    fn office_auto_spacing_uses_twips_and_per_run_ascent_descent_maxima() {
        let spacing = Spacing {
            before: 0.,
            after: 6.,
            line_height: 1.2,
        };
        assert_eq!(
            office_line_box(&[run("中文", "SimSun")], spacing, None, &metrics())
                .unwrap()
                .line_height_pt,
            20.25
        );
        assert_eq!(
            office_line_box(&[run("中文", "SimSun")], spacing, None, &metrics())
                .unwrap()
                .baseline_pt,
            18.1
        );
        assert_eq!(
            office_line_box(
                &[run("Latin", "Times New Roman")],
                spacing,
                None,
                &metrics()
            )
            .unwrap()
            .line_height_pt,
            16.55
        );
        assert_eq!(
            office_line_box(
                &[run("中文", "SimSun"), run("1.", "Times New Roman")],
                spacing,
                None,
                &metrics()
            )
            .unwrap()
            .line_height_pt,
            20.8
        );
        let exact = ParagraphFormat {
            line_height_pt: Some(20.),
            ..Default::default()
        };
        assert_eq!(
            office_line_box(&[run("中文", "SimSun")], spacing, Some(&exact), &metrics())
                .unwrap()
                .line_height_pt,
            20.
        );
        assert_eq!(
            office_line_box(&[run("中文", "SimSun")], spacing, Some(&exact), &metrics())
                .unwrap()
                .baseline_pt,
            14.75
        );
    }
    #[test]
    fn unknown_native_and_invalid_metric_data_use_the_legacy_fallback() {
        let run = run("中文", "SimSun");
        assert!(
            office_line_box(
                &[run.clone()],
                Spacing::default(),
                None,
                &Default::default()
            )
            .is_none()
        );
        for value in [-1., 5., f32::INFINITY, f32::NAN] {
            let mut metrics = metrics();
            metrics.get_mut("simsun").unwrap().ascent = value;
            assert!(office_line_box(&[run.clone()], Spacing::default(), None, &metrics).is_none());
        }
        let huge = ParagraphFormat {
            line_height_pt: Some(1e30),
            ..Default::default()
        };
        assert_eq!(
            office_line_box(&[run], Spacing::default(), Some(&huge), &metrics())
                .unwrap()
                .line_height_pt,
            1000.
        );
    }

    #[test]
    fn symbol_bullet_with_cjk_text_keeps_the_office_line_box() {
        let mut metrics = metrics();
        metrics.insert(
            "symbol".into(),
            FontMetrics {
                ascent: 1.00537109375,
                descent: 0.2197265625,
                line_gap: 0.,
                cjk: false,
            },
        );
        let line = office_line_box(
            &[run("中文", "SimSun"), run("•", "Symbol")],
            Spacing {
                before: 0.,
                after: 6.,
                line_height: 1.2,
            },
            None,
            &metrics,
        )
        .unwrap();
        assert_eq!(line.line_height_pt, 20.85);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSection {
    pub start_block: usize,
    pub page_settings: PageSettings,
    pub header: Option<String>,
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentNote {
    pub id: u32,
    pub reference_block: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BibliographyEntry {
    pub id: String,
    pub author: String,
    pub title: String,
    pub year: Option<u32>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMergeField {
    pub name: String,
    pub block_index: usize,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRevision {
    pub id: Uuid,
    pub author: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub block_index: usize,
    pub kind: RevisionKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevisionKind {
    Insert,
    Delete,
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
            professional: ProfessionalLayout::default(),
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
            professional: self.professional.clone(),
            styles: self.styles.clone(),
            metadata: DocumentMetadata::default(),
        };

        let second = WriterDocument {
            id: Uuid::new_v4(),
            title: format!("{} (part 2)", self.title),
            body: second_body,
            page_settings: self.page_settings.clone(),
            professional: self.professional.clone(),
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

// Shared compatibility adapter for legacy plain-text collaboration messages.
#[derive(Deserialize)]
struct WriterDocLite {
    #[serde(default)]
    body: Vec<serde_json::Value>,
}

pub fn extract_run_text(runs: &[serde_json::Value]) -> String {
    runs.iter()
        .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
        .collect::<String>()
}

pub fn writer_content_to_plain(content: &serde_json::Value) -> String {
    let doc: WriterDocLite = match serde_json::from_value(content.clone()) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    doc.body
        .iter()
        .filter_map(|block| {
            if block["type"] == "PageBreak" {
                Some("\u{000c}".to_string())
            } else {
                block
                    .get("runs")?
                    .as_array()
                    .map(|runs| extract_run_text(runs))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn plain_writer_run(text: &str) -> serde_json::Value {
    serde_json::json!({
        "text": text, "bold": false, "italic": false, "underline": false,
        "strikethrough": false, "font_family": null, "font_size": null,
        "color": null, "background_color": null, "link": null,
        "superscript": false, "subscript": false
    })
}

/// Splice the changed character range while retaining formatting on unaffected runs.
pub fn replace_writer_runs(runs: &[serde_json::Value], text: &str) -> Vec<serde_json::Value> {
    let old = extract_run_text(runs).chars().collect::<Vec<_>>();
    let new = text.chars().collect::<Vec<_>>();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let replacement: String = new[prefix..new.len() - suffix].iter().collect();
    let mut result = Vec::new();
    let mut offset = 0;
    let mut inserted = false;
    for run in runs {
        let chars = run["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .collect::<Vec<_>>();
        let end = offset + chars.len();
        let mut value: String = chars[..prefix.saturating_sub(offset).min(chars.len())]
            .iter()
            .collect();
        if !inserted && end >= prefix {
            value.push_str(&replacement);
            inserted = true;
        }
        value.extend(chars[(old.len() - suffix).saturating_sub(offset).min(chars.len())..].iter());
        if !value.is_empty() {
            let mut updated = run.clone();
            updated["text"] = serde_json::Value::String(value);
            result.push(updated);
        }
        offset = end;
    }
    if result.is_empty() {
        result.push(plain_writer_run(text));
    }
    result
}

pub fn writer_content_with_text(base: &serde_json::Value, text: &str) -> serde_json::Value {
    // A metadata-only save must never rewrite rich blocks or their stable IDs.
    if writer_content_to_plain(base) == text {
        return base.clone();
    }
    let mut content = base.clone();
    let Some(object) = content.as_object_mut() else {
        return serde_json::json!({ "body": [] });
    };
    let mut replacement = Vec::new();
    for line in text.split('\n') {
        for (index, part) in line.split('\u{000c}').enumerate() {
            if index > 0 {
                replacement.push(serde_json::json!({ "type": "PageBreak" }));
            }
            if part.is_empty() && line.contains('\u{000c}') {
                continue;
            }
            replacement.push(serde_json::json!({
                "type": "Paragraph",
                "id": uuid::Uuid::new_v4(),
                "runs": [plain_writer_run(part)],
                "alignment": "Left",
                "indent_level": 0,
                "spacing": { "before": 0.0, "after": 8.0, "line_height": 1.15 }
            }));
        }
    }
    let mut replacement = replacement.into_iter();
    let mut blocks = Vec::new();
    for original in base["body"].as_array().into_iter().flatten() {
        if original.get("runs").is_none() && original["type"] != "PageBreak" {
            // Tables, images, lists and other rich blocks are not represented by
            // the plain-text surface. Keep them rather than silently deleting them.
            blocks.push(original.clone());
            continue;
        }
        if let Some(mut next) = replacement.next() {
            if let (Some(runs), Some(next_runs)) =
                (original["runs"].as_array(), next["runs"].as_array())
            {
                let line = extract_run_text(next_runs);
                next = original.clone();
                if extract_run_text(runs) != line {
                    next["runs"] = serde_json::Value::Array(replace_writer_runs(runs, &line));
                }
            }
            blocks.push(next);
        }
    }
    blocks.extend(replacement);
    object.insert("body".into(), serde_json::Value::Array(blocks));
    content
}
