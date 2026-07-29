use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------

/// A Slides presentation — similar to PowerPoint / Google Slides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presentation {
    pub id: Uuid,
    pub title: String,
    pub slides: Vec<Slide>,
    pub master: SlideMaster,
    pub metadata: PresentationMetadata,
}

/// Presentation-level metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresentationMetadata {
    pub author: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    /// Slide width in pixels (default 1920).
    pub slide_width: f32,
    /// Slide height in pixels (default 1080).
    pub slide_height: f32,
}

impl Default for PresentationMetadata {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            author: None,
            created_at: now,
            modified_at: now,
            slide_width: 1920.0,
            slide_height: 1080.0,
        }
    }
}

impl Presentation {
    /// Create a new presentation with a default master and one blank title slide.
    pub fn new(title: String) -> Self {
        let master = SlideMaster::default();
        let title_slide = Slide::new(SlideLayout::TitleSlide);
        Self {
            id: Uuid::new_v4(),
            title,
            slides: vec![title_slide],
            master,
            metadata: PresentationMetadata::default(),
        }
    }

    /// Add a new slide with the given layout and return a mutable reference.
    pub fn add_slide(&mut self, layout: SlideLayout) -> &mut Slide {
        self.slides.push(Slide::new(layout));
        self.slides.last_mut().expect("just pushed")
    }

    /// Remove the slide at `index`.
    ///
    /// # Panics
    /// Panics if `index` is out of bounds.
    pub fn remove_slide(&mut self, index: usize) {
        self.slides.remove(index);
    }

    /// Move a slide from position `from` to position `to`.
    ///
    /// # Panics
    /// Panics if either index is out of bounds.
    pub fn reorder_slide(&mut self, from: usize, to: usize) {
        let slide = self.slides.remove(from);
        self.slides.insert(to, slide);
    }

    /// Duplicate the slide at `index`, inserting the copy immediately after.
    ///
    /// Returns the new slide's index, or `None` if `index` is out of bounds.
    pub fn duplicate_slide(&mut self, index: usize) -> Option<usize> {
        if index >= self.slides.len() {
            return None;
        }
        let mut copy = self.slides[index].clone();
        copy.id = Uuid::new_v4();
        let new_index = index + 1;
        self.slides.insert(new_index, copy);
        Some(new_index)
    }

    /// Get the total number of elements across all slides (counting
    /// elements inside groups recursively).
    pub fn total_elements(&self) -> usize {
        self.slides
            .iter()
            .map(|s| count_slide_elements(&s.elements))
            .sum()
    }

    /// Apply a transition to all slides in the presentation.
    pub fn apply_transition_all(&mut self, transition: SlideTransition) {
        let t = Transition {
            transition_type: transition.transition_type,
            duration_ms: transition.duration_ms,
        };
        for slide in &mut self.slides {
            slide.transition = Some(t.clone());
        }
    }

    /// Get speaker notes for all slides as `(slide_index, notes_text)`.
    ///
    /// Only includes slides that have non-empty notes.
    pub fn all_notes(&self) -> Vec<(usize, &str)> {
        self.slides
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.notes.is_empty())
            .map(|(i, s)| (i, s.notes.as_str()))
            .collect()
    }

    /// Find all slides that contain the given text query.
    ///
    /// Returns the indices of matching slides.
    pub fn find_text(&self, query: &str) -> Vec<usize> {
        self.slides
            .iter()
            .enumerate()
            .filter(|(_, slide)| slide_contains_text(slide, query))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Transition specification for `apply_transition_all`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideTransition {
    pub transition_type: TransitionType,
    pub duration_ms: u32,
}

// ---------------------------------------------------------------------------
// Slide
// ---------------------------------------------------------------------------

/// A single slide in a presentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slide {
    pub id: Uuid,
    pub elements: Vec<SlideElement>,
    pub background: Background,
    pub transition: Option<Transition>,
    pub notes: String,
    pub layout: SlideLayout,
}

impl Slide {
    /// Create a new empty slide with the given layout.
    pub fn new(layout: SlideLayout) -> Self {
        Self {
            id: Uuid::new_v4(),
            elements: Vec::new(),
            background: Background::Solid("#FFFFFF".to_owned()),
            transition: None,
            notes: String::new(),
            layout,
        }
    }

    /// Add a text box and return its id.
    pub fn add_text_box(&mut self, x: f32, y: f32, width: f32, height: f32, text: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.elements.push(SlideElement::TextBox(TextBox {
            id,
            x,
            y,
            width,
            height,
            rotation: 0.0,
            content: vec![SlideTextRun::plain(text)],
            background_color: None,
            border: None,
            padding: 8.0,
        }));
        id
    }

    /// Add an image element and return its id.
    pub fn add_image(&mut self, x: f32, y: f32, width: f32, height: f32, url: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.elements.push(SlideElement::Image(SlideImage {
            id,
            x,
            y,
            width,
            height,
            rotation: 0.0,
            url: url.to_owned(),
            alt_text: String::new(),
            crop: None,
        }));
        id
    }

    /// Add a shape element and return its id.
    pub fn add_shape(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        shape_type: ShapeType,
    ) -> Uuid {
        let id = Uuid::new_v4();
        self.elements.push(SlideElement::Shape(Shape {
            id,
            x,
            y,
            width,
            height,
            rotation: 0.0,
            shape_type,
            fill_color: None,
            border: None,
            text: None,
        }));
        id
    }
}

// ---------------------------------------------------------------------------
// Slide elements
// ---------------------------------------------------------------------------

/// A visual element placed on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SlideElement {
    TextBox(TextBox),
    Image(SlideImage),
    Shape(Shape),
    Chart(SlideChart),
    Video(SlideVideo),
    Group(ElementGroup),
}

/// A text box on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBox {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub content: Vec<SlideTextRun>,
    pub background_color: Option<String>,
    pub border: Option<SlideBorder>,
    pub padding: f32,
}

/// An image placed on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideImage {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub url: String,
    pub alt_text: String,
    pub crop: Option<Crop>,
}

/// A geometric shape, optionally containing text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shape {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub shape_type: ShapeType,
    pub fill_color: Option<String>,
    pub border: Option<SlideBorder>,
    pub text: Option<Vec<SlideTextRun>>,
}

/// Supported geometric shape types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeType {
    Rectangle,
    RoundedRectangle,
    Circle,
    Ellipse,
    Triangle,
    Arrow,
    Star,
    Diamond,
    Pentagon,
    Hexagon,
    Line,
    CurvedLine,
}

/// A chart embedded on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideChart {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub chart_type: ChartType,
    pub data: ChartData,
}

/// Supported chart types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartType {
    Bar,
    Line,
    Pie,
    Doughnut,
    Scatter,
    Area,
}

/// Chart data: labels (x-axis) + one or more datasets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartData {
    pub labels: Vec<String>,
    pub datasets: Vec<Dataset>,
}

/// A single series within a chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    pub name: String,
    pub values: Vec<f64>,
    pub color: Option<String>,
}

/// A video element on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideVideo {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub url: String,
    pub autoplay: bool,
}

/// A group of slide elements treated as a single unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementGroup {
    pub id: Uuid,
    pub elements: Vec<SlideElement>,
}

// ---------------------------------------------------------------------------
// Styling & layout helpers
// ---------------------------------------------------------------------------

/// A styled run of text used inside slide elements.
///
/// This mirrors [`crate::writer::TextRun`] but is a standalone type to keep
/// the modules decoupled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlideTextRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub color: Option<String>,
    pub link: Option<String>,
}

impl SlideTextRun {
    /// Create a plain (unformatted) text run.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

/// A border used on slide elements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideBorder {
    pub color: String,
    pub width: f32,
}

/// Crop insets (fractions 0.0 – 1.0 from each edge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crop {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// Slide background.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Background {
    /// A solid colour (CSS hex or named).
    Solid(String),
    /// A linear gradient: start colour, end colour, angle in degrees.
    Gradient(String, String, f32),
    /// A background image URL.
    Image(String),
}

/// A slide transition effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub transition_type: TransitionType,
    /// Duration of the transition in milliseconds.
    pub duration_ms: u32,
}

/// Supported transition effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionType {
    None,
    Fade,
    Slide,
    Zoom,
    Flip,
    Push,
    Dissolve,
}

/// Global slide master: defines default appearance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideMaster {
    pub background: Background,
    pub default_font: String,
    pub default_font_size: f32,
    pub title_style: Option<SlideTextRun>,
    pub body_style: Option<SlideTextRun>,
}

impl Default for SlideMaster {
    fn default() -> Self {
        Self {
            background: Background::Solid("#FFFFFF".to_owned()),
            default_font: "Arial".to_owned(),
            default_font_size: 18.0,
            title_style: None,
            body_style: None,
        }
    }
}

/// Pre-defined slide layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlideLayout {
    Blank,
    TitleSlide,
    TitleAndContent,
    TwoColumn,
    SectionHeader,
    TitleOnly,
}

// ---------------------------------------------------------------------------
// Helpers for Presentation operations
// ---------------------------------------------------------------------------

/// Recursively count elements (groups contain nested elements).
fn count_slide_elements(elements: &[SlideElement]) -> usize {
    elements
        .iter()
        .map(|e| match e {
            SlideElement::Group(group) => 1 + count_slide_elements(&group.elements),
            _ => 1,
        })
        .sum()
}

/// Check whether a slide contains the query text in any text element.
fn slide_contains_text(slide: &Slide, query: &str) -> bool {
    let lower_query = query.to_lowercase();
    // Check notes.
    if slide.notes.to_lowercase().contains(&lower_query) {
        return true;
    }
    // Check elements recursively.
    elements_contain_text(&slide.elements, &lower_query)
}

fn elements_contain_text(elements: &[SlideElement], lower_query: &str) -> bool {
    for element in elements {
        match element {
            SlideElement::TextBox(tb) => {
                for run in &tb.content {
                    if run.text.to_lowercase().contains(lower_query) {
                        return true;
                    }
                }
            }
            SlideElement::Shape(shape) => {
                if let Some(ref runs) = shape.text {
                    for run in runs {
                        if run.text.to_lowercase().contains(lower_query) {
                            return true;
                        }
                    }
                }
            }
            SlideElement::Group(group) if elements_contain_text(&group.elements, lower_query) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_presentation() -> Presentation {
        let mut pres = Presentation::new("Test Pres".into());
        // The first slide is already a TitleSlide.
        pres.slides[0].add_text_box(10.0, 10.0, 400.0, 50.0, "Welcome");
        pres.slides[0].notes = "Opening notes".into();

        let slide1 = pres.add_slide(SlideLayout::TitleAndContent);
        slide1.add_text_box(10.0, 10.0, 400.0, 50.0, "Slide Two Content");

        let slide2 = pres.add_slide(SlideLayout::Blank);
        slide2.add_text_box(20.0, 20.0, 300.0, 40.0, "Hello world");
        slide2.notes = "Blank slide notes".into();

        pres
    }

    #[test]
    fn test_duplicate_slide() {
        let mut pres = make_sample_presentation();
        let original_len = pres.slides.len();
        let new_index = pres.duplicate_slide(0).unwrap();
        assert_eq!(new_index, 1);
        assert_eq!(pres.slides.len(), original_len + 1);
        // The duplicated slide should have a different id.
        assert_ne!(pres.slides[0].id, pres.slides[1].id);
    }

    #[test]
    fn test_duplicate_slide_out_of_bounds() {
        let mut pres = make_sample_presentation();
        assert!(pres.duplicate_slide(100).is_none());
    }

    #[test]
    fn test_total_elements() {
        let pres = make_sample_presentation();
        let total = pres.total_elements();
        // 1 element on first slide + 1 on second + 1 on third = 3
        assert_eq!(total, 3);
    }

    #[test]
    fn test_apply_transition_all() {
        let mut pres = make_sample_presentation();
        pres.apply_transition_all(SlideTransition {
            transition_type: TransitionType::Fade,
            duration_ms: 500,
        });
        for slide in &pres.slides {
            let t = slide.transition.as_ref().unwrap();
            assert_eq!(t.transition_type, TransitionType::Fade);
            assert_eq!(t.duration_ms, 500);
        }
    }

    #[test]
    fn test_all_notes() {
        let pres = make_sample_presentation();
        let notes = pres.all_notes();
        // Slides 0 and 2 have notes.
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].0, 0);
        assert_eq!(notes[0].1, "Opening notes");
        assert_eq!(notes[1].0, 2);
        assert_eq!(notes[1].1, "Blank slide notes");
    }

    #[test]
    fn test_find_text() {
        let pres = make_sample_presentation();
        let results = pres.find_text("welcome");
        assert_eq!(results, vec![0]);

        let results2 = pres.find_text("Hello world");
        assert_eq!(results2, vec![2]);

        let results3 = pres.find_text("nonexistent");
        assert!(results3.is_empty());
    }

    #[test]
    fn test_find_text_in_notes() {
        let pres = make_sample_presentation();
        let results = pres.find_text("Opening");
        assert_eq!(results, vec![0]);
    }
}
