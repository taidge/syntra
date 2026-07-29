use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Whiteboard
// ---------------------------------------------------------------------------

/// A Whiteboard — freeform drawing canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Whiteboard {
    pub id: Uuid,
    pub title: String,
    pub layers: Vec<Layer>,
    pub canvas_width: f32,
    pub canvas_height: f32,
    pub background_color: String,
}

impl Whiteboard {
    /// Create a new whiteboard with one default layer.
    pub fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            layers: vec![Layer::new("Layer 1".to_owned())],
            canvas_width: 1920.0,
            canvas_height: 1080.0,
            background_color: "#FFFFFF".to_owned(),
        }
    }

    /// Add a new named layer and return a mutable reference to it.
    pub fn add_layer(&mut self, name: String) -> &mut Layer {
        self.layers.push(Layer::new(name));
        self.layers.last_mut().expect("just pushed")
    }

    /// Flatten all layers into a single layer named "Flattened".
    ///
    /// All elements from every layer are collected into one layer. The
    /// original layers are replaced.
    pub fn flatten_layers(&mut self) {
        let mut all_elements = Vec::new();
        for layer in &self.layers {
            all_elements.extend(layer.elements.clone());
        }
        let mut merged = Layer::new("Flattened".to_owned());
        merged.elements = all_elements;
        self.layers = vec![merged];
    }

    /// Compute the axis-aligned bounding box that encloses all elements
    /// across all layers.
    ///
    /// Returns `(min_x, min_y, max_x, max_y)`.  If the whiteboard is
    /// empty the canvas bounds `(0, 0, canvas_width, canvas_height)` are
    /// returned.
    pub fn bounding_box(&self) -> (f64, f64, f64, f64) {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        let mut found = false;

        for layer in &self.layers {
            for elem in &layer.elements {
                let (x1, y1, x2, y2) = match elem {
                    DrawElement::Rectangle(r) => (
                        r.x as f64,
                        r.y as f64,
                        (r.x + r.width) as f64,
                        (r.y + r.height) as f64,
                    ),
                    DrawElement::Ellipse(e) => (
                        (e.x - e.rx) as f64,
                        (e.y - e.ry) as f64,
                        (e.x + e.rx) as f64,
                        (e.y + e.ry) as f64,
                    ),
                    DrawElement::Line(l) => (
                        l.start.x.min(l.end.x) as f64,
                        l.start.y.min(l.end.y) as f64,
                        l.start.x.max(l.end.x) as f64,
                        l.start.y.max(l.end.y) as f64,
                    ),
                    DrawElement::Text(t) => (
                        t.x as f64,
                        t.y as f64,
                        t.x as f64 + 100.0,
                        t.y as f64 + 20.0,
                    ),
                    DrawElement::Image(img) => (
                        img.x as f64,
                        img.y as f64,
                        (img.x + img.width) as f64,
                        (img.y + img.height) as f64,
                    ),
                    DrawElement::Path(p) => {
                        let xs: Vec<f64> = p.points.iter().map(|pt| pt.x as f64).collect();
                        let ys: Vec<f64> = p.points.iter().map(|pt| pt.y as f64).collect();
                        (
                            xs.iter().cloned().fold(f64::MAX, f64::min),
                            ys.iter().cloned().fold(f64::MAX, f64::min),
                            xs.iter().cloned().fold(f64::MIN, f64::max),
                            ys.iter().cloned().fold(f64::MIN, f64::max),
                        )
                    }
                };
                if x1 < min_x {
                    min_x = x1;
                }
                if y1 < min_y {
                    min_y = y1;
                }
                if x2 > max_x {
                    max_x = x2;
                }
                if y2 > max_y {
                    max_y = y2;
                }
                found = true;
            }
        }

        if !found {
            return (
                0.0,
                0.0,
                self.canvas_width as f64,
                self.canvas_height as f64,
            );
        }
        (min_x, min_y, max_x, max_y)
    }

    /// Count the total number of elements across all layers.
    pub fn total_elements(&self) -> usize {
        self.layers.iter().map(|l| l.elements.len()).sum()
    }

    /// Remove all elements of a specific type (by variant name) from every
    /// layer.
    ///
    /// Accepted type names: `"Path"`, `"Rectangle"`, `"Ellipse"`, `"Line"`,
    /// `"Text"`, `"Image"`.
    pub fn remove_elements_by_type(&mut self, element_type: &str) {
        for layer in &mut self.layers {
            layer.elements.retain(|elem| {
                let name = match elem {
                    DrawElement::Path(_) => "Path",
                    DrawElement::Rectangle(_) => "Rectangle",
                    DrawElement::Ellipse(_) => "Ellipse",
                    DrawElement::Line(_) => "Line",
                    DrawElement::Text(_) => "Text",
                    DrawElement::Image(_) => "Image",
                };
                name != element_type
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------

/// A layer inside a whiteboard; contains drawing elements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: Uuid,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub elements: Vec<DrawElement>,
}

impl Layer {
    /// Create a new, visible, unlocked, fully opaque layer.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            visible: true,
            locked: false,
            opacity: 1.0,
            elements: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing elements
// ---------------------------------------------------------------------------

/// A drawing primitive placed on a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DrawElement {
    Path(DrawPath),
    Rectangle(DrawRect),
    Ellipse(DrawEllipse),
    Line(DrawLine),
    Text(DrawText),
    Image(DrawImage),
}

/// A freeform path (pen / brush stroke).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawPath {
    pub id: Uuid,
    pub points: Vec<Point>,
    pub stroke_color: String,
    pub stroke_width: f32,
    pub fill_color: Option<String>,
    pub closed: bool,
}

/// A 2-D point with optional pressure (for stylus input).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub pressure: Option<f32>,
}

impl Point {
    /// Create a point without pressure data.
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            pressure: None,
        }
    }

    /// Create a point with pressure data.
    pub fn with_pressure(x: f32, y: f32, pressure: f32) -> Self {
        Self {
            x,
            y,
            pressure: Some(pressure),
        }
    }
}

/// A rectangle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawRect {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub fill_color: Option<String>,
    pub stroke_color: String,
    pub stroke_width: f32,
    pub corner_radius: f32,
}

/// An ellipse (or circle when rx == ry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawEllipse {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub rx: f32,
    pub ry: f32,
    pub rotation: f32,
    pub fill_color: Option<String>,
    pub stroke_color: String,
    pub stroke_width: f32,
}

/// A straight line segment with optional arrowheads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawLine {
    pub id: Uuid,
    pub start: Point,
    pub end: Point,
    pub stroke_color: String,
    pub stroke_width: f32,
    pub arrow_start: bool,
    pub arrow_end: bool,
}

/// A text label placed on the canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawText {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub font_size: f32,
    pub font_family: String,
    pub color: String,
    pub bold: bool,
    pub italic: bool,
}

/// An image placed on the canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawImage {
    pub id: Uuid,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub url: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_whiteboard() -> Whiteboard {
        let mut wb = Whiteboard::new("Test Board".into());

        // Add a rectangle to the default layer.
        wb.layers[0].elements.push(DrawElement::Rectangle(DrawRect {
            id: Uuid::new_v4(),
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
            rotation: 0.0,
            fill_color: Some("#FF0000".into()),
            stroke_color: "#000000".into(),
            stroke_width: 2.0,
            corner_radius: 0.0,
        }));

        // Add a text element.
        wb.layers[0].elements.push(DrawElement::Text(DrawText {
            id: Uuid::new_v4(),
            x: 200.0,
            y: 100.0,
            text: "Hello".into(),
            font_size: 16.0,
            font_family: "Arial".into(),
            color: "#000000".into(),
            bold: false,
            italic: false,
        }));

        // Add a second layer with an ellipse.
        let layer2 = wb.add_layer("Layer 2".into());
        layer2.elements.push(DrawElement::Ellipse(DrawEllipse {
            id: Uuid::new_v4(),
            x: 300.0,
            y: 300.0,
            rx: 50.0,
            ry: 30.0,
            rotation: 0.0,
            fill_color: None,
            stroke_color: "#0000FF".into(),
            stroke_width: 1.5,
        }));

        wb
    }

    #[test]
    fn test_total_elements() {
        let wb = make_sample_whiteboard();
        assert_eq!(wb.total_elements(), 3);
    }

    #[test]
    fn test_flatten_layers() {
        let mut wb = make_sample_whiteboard();
        assert_eq!(wb.layers.len(), 2);
        wb.flatten_layers();
        assert_eq!(wb.layers.len(), 1);
        assert_eq!(wb.layers[0].name, "Flattened");
        assert_eq!(wb.layers[0].elements.len(), 3);
    }

    #[test]
    fn test_bounding_box() {
        let wb = make_sample_whiteboard();
        let (min_x, min_y, max_x, max_y) = wb.bounding_box();
        // Rectangle: (10, 20) -> (110, 70)
        // Text: (200, 100) -> ~(248, 119.2)
        // Ellipse: (250, 270) -> (350, 330)
        assert!(min_x <= 10.0);
        assert!(min_y <= 20.0);
        assert!(max_x >= 110.0);
        assert!(max_y >= 70.0);
    }

    #[test]
    fn test_bounding_box_empty() {
        let mut wb = Whiteboard::new("Empty".into());
        wb.layers[0].elements.clear();
        let (min_x, min_y, max_x, max_y) = wb.bounding_box();
        assert_eq!(min_x, 0.0);
        assert_eq!(min_y, 0.0);
        assert_eq!(max_x, 1920.0);
        assert_eq!(max_y, 1080.0);
    }

    #[test]
    fn test_remove_elements_by_type() {
        let mut wb = make_sample_whiteboard();
        assert_eq!(wb.total_elements(), 3);
        wb.remove_elements_by_type("Text");
        assert_eq!(wb.total_elements(), 2);
        // Verify no Text elements remain.
        for layer in &wb.layers {
            for elem in &layer.elements {
                assert!(!matches!(elem, DrawElement::Text(_)));
            }
        }
    }

    #[test]
    fn test_remove_elements_by_type_all_rectangles() {
        let mut wb = make_sample_whiteboard();
        wb.remove_elements_by_type("Rectangle");
        for layer in &wb.layers {
            for elem in &layer.elements {
                assert!(!matches!(elem, DrawElement::Rectangle(_)));
            }
        }
    }

    #[test]
    fn test_remove_elements_by_type_unknown() {
        let mut wb = make_sample_whiteboard();
        let before = wb.total_elements();
        wb.remove_elements_by_type("UnknownType");
        assert_eq!(wb.total_elements(), before);
    }
}
