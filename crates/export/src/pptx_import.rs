//! Import real DrawingML objects from relationship-addressed presentation parts.
use crate::ooxml_import::{self as xml, Archive, Node, Relationship, warn};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{GenericImageView, ImageFormat, ImageReader, imageops::FilterType};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use syntra_doc_engine::slides::*;
use uuid::Uuid;
const EMU: f64 = 9525.0;
#[cfg(test)]
#[path = "pptx_media_tests.rs"]
mod media_tests;
#[derive(Clone, Default)]
struct Theme {
    colors: BTreeMap<String, String>,
    major: String,
    minor: String,
    fills: Vec<Node>,
    background_fills: Vec<Node>,
}
fn theme(root: Option<&Node>) -> Theme {
    let mut result = Theme {
        major: "Arial".into(),
        minor: "Arial".into(),
        ..Default::default()
    };
    if let Some(root) = root {
        result.fills = root
            .desc("fillStyleLst")
            .map(|n| n.children.clone())
            .unwrap_or_default();
        result.background_fills = root
            .desc("bgFillStyleLst")
            .map(|n| n.children.clone())
            .unwrap_or_default();
        if let Some(scheme) = root.desc("clrScheme") {
            for entry in &scheme.children {
                if let Some(value) = entry
                    .children
                    .first()
                    .and_then(|c| c.attr("lastClr").or_else(|| c.attr("val")))
                {
                    result
                        .colors
                        .insert(entry.name.clone(), format!("#{value}"));
                }
            }
        }
        result.major = root
            .desc("majorFont")
            .and_then(|n| n.child("latin"))
            .and_then(|n| n.attr("typeface"))
            .unwrap_or("Arial")
            .into();
        result.minor = root
            .desc("minorFont")
            .and_then(|n| n.child("latin"))
            .and_then(|n| n.attr("typeface"))
            .unwrap_or("Arial")
            .into();
    }
    for (key, value) in [
        ("bg1", "lt1"),
        ("tx1", "dk1"),
        ("bg2", "lt2"),
        ("tx2", "dk2"),
    ] {
        if let Some(color) = result.colors.get(value).cloned() {
            result.colors.insert(key.into(), color);
        }
    }
    result
}
fn apply_color_map(theme: &mut Theme, mapping: Option<&Node>) {
    let Some(mapping) = mapping else { return };
    let colors = theme.colors.clone();
    for (alias, name) in &mapping.attrs {
        if let Some(value) = colors.get(name) {
            theme.colors.insert(alias.clone(), value.clone());
        }
    }
}

/// Resolve bgRef through the theme style matrix before reading its fill.
/// The reference colour substitutes phClr inside the selected style.
fn theme_background(background: &Node, theme: &Theme) -> Option<(Node, Theme)> {
    let reference = background.child("bgRef")?;
    let index = reference.num("idx")? as usize;
    if matches!(index, 0 | 1000) {
        return Some((
            Node {
                name: "bgPr".into(),
                children: vec![Node {
                    name: "noFill".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            theme.clone(),
        ));
    }
    let fill = match index {
        1..=999 => theme.fills.get(index - 1),
        1001.. => theme.background_fills.get(index - 1001),
        _ => None,
    };
    let mut theme = theme.clone();
    if let Some(value) = reference.children.iter().find_map(|n| color(n, &theme)) {
        theme.colors.insert("phClr".into(), value);
    }
    let fill = fill.cloned().or_else(|| {
        theme.colors.get("phClr").map(|value| Node {
            name: "solidFill".into(),
            children: vec![Node {
                name: "srgbClr".into(),
                attrs: [("val".into(), value.trim_start_matches('#').into())].into(),
                ..Default::default()
            }],
            ..Default::default()
        })
    })?;
    Some((
        Node {
            name: "bgPr".into(),
            children: vec![fill],
            ..Default::default()
        },
        theme,
    ))
}
fn color(node: &Node, theme: &Theme) -> Option<String> {
    let value = match node.name.as_str() {
        "srgbClr" => format!("#{}", node.attr("val")?),
        "sysClr" => format!("#{}", node.attr("lastClr")?),
        "schemeClr" => theme.colors.get(node.attr("val")?)?.clone(),
        "solidFill" => return node.children.iter().find_map(|c| color(c, theme)),
        _ => return None,
    };
    let hex = value.trim_start_matches('#');
    if hex.len() != 6 || !hex.is_ascii() || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut rgb = [
        u8::from_str_radix(&hex[..2], 16).ok()? as f64,
        u8::from_str_radix(&hex[2..4], 16).ok()? as f64,
        u8::from_str_radix(&hex[4..], 16).ok()? as f64,
    ];
    for adjustment in &node.children {
        let factor = adjustment.num("val").unwrap_or(100000.0) / 100000.0;
        for channel in &mut rgb {
            *channel = match adjustment.name.as_str() {
                "tint" => *channel + (255.0 - *channel) * factor,
                "shade" | "lumMod" => *channel * factor,
                "lumOff" => *channel + 255.0 * factor,
                _ => *channel,
            };
        }
    }
    Some(format!(
        "#{:02X}{:02X}{:02X}",
        rgb[0].clamp(0.0, 255.0) as u8,
        rgb[1].clamp(0.0, 255.0) as u8,
        rgb[2].clamp(0.0, 255.0) as u8
    ))
}
fn fill(node: &Node, theme: &Theme) -> Option<String> {
    node.child("solidFill").and_then(|n| color(n, theme))
}
fn border(properties: &Node, theme: &Theme) -> Option<SlideBorder> {
    let line = properties.child("ln")?;
    if line.child("noFill").is_some() {
        return None;
    }
    Some(SlideBorder {
        color: fill(line, theme).unwrap_or_else(|| "#000000".into()),
        width: (line.num("w").unwrap_or(9525.0) / EMU) as f32,
    })
}
#[derive(Clone, Copy)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    x: f64,
    y: f64,
}
impl Default for Matrix {
    fn default() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            x: 0.0,
            y: 0.0,
        }
    }
}
impl Matrix {
    fn point(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.x,
            self.b * x + self.d * y + self.y,
        )
    }
    fn then(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            x: self.a * other.x + self.c * other.y + self.x,
            y: self.b * other.x + self.d * other.y + self.y,
        }
    }
}
#[derive(Clone, Copy)]
struct Bounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rotation: f32,
}
fn properties(node: &Node) -> Option<&Node> {
    node.child("spPr").or_else(|| node.child("grpSpPr"))
}
fn transform(node: &Node) -> Option<&Node> {
    properties(node)
        .and_then(|p| p.child("xfrm"))
        .or_else(|| node.child("xfrm"))
}
fn bounds(
    node: &Node,
    fallback: Option<&Node>,
    matrix: Matrix,
    warnings: &mut Vec<String>,
) -> Bounds {
    let transform = transform(node).or_else(|| fallback.and_then(transform));
    let off = transform.and_then(|n| n.child("off"));
    let ext = transform.and_then(|n| n.child("ext"));
    if transform.is_none() {
        warn(
            warnings,
            "An object has no transform or inherited placeholder bounds; default bounds were used. Download the original for exact layout.",
        );
    }
    let x = off.and_then(|n| n.num("x")).unwrap_or(476250.0) / EMU;
    let y = off.and_then(|n| n.num("y")).unwrap_or(476250.0) / EMU;
    let width = ext.and_then(|n| n.num("cx")).unwrap_or(5715000.0) / EMU;
    let height = ext.and_then(|n| n.num("cy")).unwrap_or(952500.0) / EMU;
    let (x, y) = matrix.point(x, y);
    let rotation = transform.and_then(|n| n.num("rot")).unwrap_or(0.0) / 60000.0
        + matrix.b.atan2(matrix.a).to_degrees();
    if transform.is_some_and(|n| n.flag("flipH") == Some(true) || n.flag("flipV") == Some(true)) {
        warn(
            warnings,
            "Mirrored slide objects retain their bounds but mirroring is not represented; the original preserves it.",
        );
    }
    let bounded = |n: f64, min: f64, max: f64| {
        if n.is_finite() {
            n.clamp(min, max) as f32
        } else {
            0.0
        }
    };
    let width = (width * matrix.a.hypot(matrix.b)).abs();
    let height = (height * matrix.c.hypot(matrix.d)).abs();
    if [x, y, width, height, rotation]
        .iter()
        .any(|n| !n.is_finite() || n.abs() > 10_000_000.0)
    {
        warn(
            warnings,
            "Extreme or invalid slide geometry was bounded; the original retains exact geometry.",
        );
    }
    Bounds {
        x: bounded(x, -10_000_000., 10_000_000.),
        y: bounded(y, -10_000_000., 10_000_000.),
        width: bounded(width, 0., 10_000_000.),
        height: bounded(height, 0., 10_000_000.),
        rotation: bounded(rotation % 360., -360., 360.),
    }
}
fn group_matrix(node: &Node, parent: Matrix) -> Matrix {
    let Some(t) = transform(node) else {
        return parent;
    };
    let get = |child: &str, key: &str, default: f64| {
        t.child(child).and_then(|n| n.num(key)).unwrap_or(default)
    };
    let width = get("ext", "cx", EMU);
    let height = get("ext", "cy", EMU);
    let sx = width / get("chExt", "cx", width).max(1.0);
    let sy = height / get("chExt", "cy", height).max(1.0);
    let x = get("off", "x", 0.0) / EMU;
    let y = get("off", "y", 0.0) / EMU;
    let ox = get("chOff", "x", 0.0) / EMU;
    let oy = get("chOff", "y", 0.0) / EMU;
    let angle = t.num("rot").unwrap_or(0.0) / 60000.0;
    let (sin, cos) = angle.to_radians().sin_cos();
    let center_x = x + width / EMU / 2.0;
    let center_y = y + height / EMU / 2.0;
    parent.then(Matrix {
        a: cos * sx,
        b: sin * sx,
        c: -sin * sy,
        d: cos * sy,
        x: center_x + cos * (x - center_x) - sin * (y - center_y) - cos * sx * ox + sin * sy * oy,
        y: center_y + sin * (x - center_x) + cos * (y - center_y) - sin * sx * ox - cos * sy * oy,
    })
}
fn placeholder<'a>(
    node: &Node,
    layout: Option<&'a Node>,
    master: Option<&'a Node>,
) -> Option<&'a Node> {
    let marker = node.desc("ph")?;
    for root in [layout, master].into_iter().flatten() {
        for shape in root.descendants("sp") {
            if let Some(ph) = shape.desc("ph") {
                let matches = if let Some(idx) = marker.attr("idx") {
                    ph.attr("idx").unwrap_or("0") == idx
                } else {
                    ph.attr("type").unwrap_or("body") == marker.attr("type").unwrap_or("body")
                };
                if matches {
                    return Some(shape);
                }
            }
        }
    }
    None
}
fn apply_run(
    run: &mut SlideTextRun,
    properties: &Node,
    theme: &Theme,
    rels: &BTreeMap<String, Relationship>,
) {
    if let Some(value) = properties.flag("b") {
        run.bold = value;
    }
    if let Some(value) = properties.flag("i") {
        run.italic = value;
    }
    if let Some(value) = properties.attr("u") {
        run.underline = value != "none";
    }
    if let Some(value) = properties.attr("strike") {
        run.strikethrough = value != "noStrike";
    }
    if let Some(size) = properties.num("sz") {
        run.font_size = Some((size / 100.0).clamp(1.0, 10000.0) as f32);
    }
    if let Some(family) = properties.child("latin").and_then(|n| n.attr("typeface")) {
        run.font_family = Some(match family {
            "+mj-lt" => theme.major.clone(),
            "+mn-lt" => theme.minor.clone(),
            other => other.into(),
        });
    }
    if let Some(value) = fill(properties, theme) {
        run.color = Some(value);
    }
    if let Some(link) = properties
        .child("hlinkClick")
        .and_then(|n| n.attr("r:id"))
        .and_then(|id| rels.get(id))
        .filter(|r| r.external && xml::safe_url(&r.target))
    {
        run.link = Some(link.target.clone());
    }
}
fn runs(
    node: &Node,
    fallback: Option<&Node>,
    theme: &Theme,
    rels: &BTreeMap<String, Relationship>,
    warnings: &mut Vec<String>,
) -> Vec<SlideTextRun> {
    let Some(body) = node.child("txBody").or_else(|| node.child("txPr")) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    if body
        .descendants("hlinkClick")
        .iter()
        .filter_map(|n| n.attr("r:id"))
        .filter_map(|id| rels.get(id))
        .any(|r| !r.external || !xml::safe_url(&r.target))
    {
        warn(
            warnings,
            "An unsafe or internal-action hyperlink was omitted; the original retains its action metadata.",
        );
    }
    let inherited = fallback
        .and_then(|n| n.child("txBody"))
        .and_then(|n| n.desc("defRPr"));
    for (pi, paragraph) in body.children.iter().filter(|n| n.name == "p").enumerate() {
        let mut default = SlideTextRun::plain("");
        default.font_family = Some(theme.minor.clone());
        default.paragraph = Some(paragraph_style(paragraph, pi));
        if let Some(props) = inherited {
            apply_run(&mut default, props, theme, rels);
        }
        if let Some(props) = paragraph.child("pPr") {
            if let Some(run) = props.child("defRPr") {
                apply_run(&mut default, run, theme, rels);
            }
            if let Some(bullet) = props.child("buChar").and_then(|n| n.attr("char")) {
                let mut run = default.clone();
                run.text = format!("{bullet} ");
                result.push(run);
            }
        }
        if pi > 0 {
            let mut run = default.clone();
            run.text = "\n".into();
            run.paragraph_break = true;
            result.push(run);
        }
        for child in &paragraph.children {
            match child.name.as_str() {
                "r" | "fld" => {
                    let mut run = default.clone();
                    if let Some(props) = child.child("rPr") {
                        apply_run(&mut run, props, theme, rels);
                    }
                    run.text = child.child("t").map(Node::all_text).unwrap_or_default();
                    result.push(run);
                }
                "br" => {
                    let mut run = default.clone();
                    run.text = "\n".into();
                    result.push(run);
                }
                _ => {}
            }
        }
    }
    result
}

fn paragraph_style(paragraph: &Node, index: usize) -> SlideParagraphStyle {
    let props = paragraph.child("pPr");
    let spacing = |name: &str, child: &str, divisor: f64| {
        props
            .and_then(|n| n.child(name))
            .and_then(|n| n.child(child))
            .and_then(|n| n.num("val"))
            .map(|n| (n / divisor) as f32)
    };
    SlideParagraphStyle {
        index: index as u32,
        alignment: match props.and_then(|n| n.attr("algn")) {
            Some("ctr") => "center",
            Some("r") => "right",
            Some("just" | "dist" | "thaiDist") => "justify",
            _ => "left",
        }
        .into(),
        line_height: spacing("lnSpc", "spcPct", 100000.0),
        line_height_points: spacing("lnSpc", "spcPts", 100.0),
        space_before_points: spacing("spcBef", "spcPts", 100.0),
        space_after_points: spacing("spcAft", "spcPts", 100.0),
    }
}

fn text_layout(node: &Node, fallback: Option<&Node>) -> Option<SlideTextLayout> {
    let body = node.child("txBody").and_then(|n| n.child("bodyPr"));
    let inherited = fallback
        .and_then(|n| n.child("txBody"))
        .and_then(|n| n.child("bodyPr"));
    let attr = |name| {
        body.and_then(|n| n.attr(name))
            .or_else(|| inherited.and_then(|n| n.attr(name)))
    };
    let inset = |name, default: f32| {
        attr(name)
            .and_then(|n| n.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .map(|n| (n / EMU) as f32)
            .unwrap_or(default)
    };
    Some(SlideTextLayout {
        wrap: attr("wrap") != Some("none"),
        vertical_alignment: match attr("anchor") {
            Some("ctr") => "center",
            Some("b") => "bottom",
            _ => "top",
        }
        .into(),
        inset_left: inset("lIns", 9.6),
        inset_right: inset("rIns", 9.6),
        inset_top: inset("tIns", 4.8),
        inset_bottom: inset("bIns", 4.8),
    })
}
fn shape_type(name: &str) -> Option<ShapeType> {
    Some(match name {
        "rect" => ShapeType::Rectangle,
        "roundRect" => ShapeType::RoundedRectangle,
        "ellipse" => ShapeType::Ellipse,
        "triangle" | "rtTriangle" => ShapeType::Triangle,
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" => ShapeType::Arrow,
        "star5" | "star4" => ShapeType::Star,
        "diamond" => ShapeType::Diamond,
        "pentagon" => ShapeType::Pentagon,
        "hexagon" => ShapeType::Hexagon,
        "line" => ShapeType::Line,
        "arc" | "curvedConnector3" | "curvedConnector2" => ShapeType::CurvedLine,
        _ => return None,
    })
}
const MAX_SOURCE_MEDIA_BYTES: usize = 32 * 1024 * 1024;
const MAX_INLINE_MEDIA_BYTES: usize = 2 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_PIXELS: u64 = 32 * 1024 * 1024;
const OPTIMIZE_IMAGE_BYTES: usize = 256 * 1024;

/// Source bytes, decoded pixels and serialized data URLs have separate budgets.
/// A reused picture is decoded once, but each inline occurrence consumes JSON space.
struct MediaImporter {
    cache: BTreeMap<String, Result<String, String>>,
    source_remaining: usize,
    pixels_remaining: u64,
    inline_remaining: usize,
    max_side: u32,
}
impl MediaImporter {
    fn new(slide_width: f32, slide_height: f32) -> Self {
        Self {
            cache: BTreeMap::new(),
            source_remaining: syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES * 4,
            pixels_remaining: MAX_IMAGE_PIXELS * 4,
            // Leave room for editable objects; model_budget checks the complete JSON.
            inline_remaining: syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES * 3 / 5,
            max_side: slide_width.max(slide_height).ceil().clamp(1024.0, 2048.0) as u32,
        }
    }

    fn load(
        &mut self,
        archive: &mut Archive<'_>,
        relation: &Relationship,
    ) -> Result<String, String> {
        let extension = relation
            .target
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "mp4" | "m4v" => "video/mp4",
            "webm" => "video/webm",
            "ogg" | "ogv" => "video/ogg",
            _ => return Err(format!("Unsupported embedded media format .{extension}")),
        };
        let source_size = archive
            .by_name(&relation.target)
            .map_err(|_| "Embedded media part is missing or unreadable".to_string())?
            .size();
        if source_size > MAX_SOURCE_MEDIA_BYTES as u64 {
            return Err("Embedded media exceeds the bounded source size".into());
        }
        if source_size > self.source_remaining as u64 {
            return Err("Embedded media exceeds the total source-media budget".into());
        }
        let bytes = xml::bytes_limit(
            archive,
            &relation.target,
            MAX_SOURCE_MEDIA_BYTES.min(self.source_remaining),
        )
        .map_err(|_| {
            "Embedded media could not be read within the bounded source size".to_string()
        })?;
        self.source_remaining -= bytes.len();
        let bytes = if matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
            let (optimized_mime, optimized_bytes) = self.raster(bytes)?;
            mime = optimized_mime;
            optimized_bytes
        } else {
            // Preserve animation and formats that are displayed directly by the browser.
            bytes
        };
        if bytes.len() > MAX_INLINE_MEDIA_BYTES {
            return Err(
                "Embedded media exceeds the per-object editing budget after optimization".into(),
            );
        }
        Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
    }

    fn raster(&mut self, bytes: Vec<u8>) -> Result<(&'static str, Vec<u8>), String> {
        let format = image::guess_format(&bytes)
            .map_err(|_| "Embedded image format could not be decoded".to_string())?;
        let mime = match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            _ => {
                return Err(
                    "Embedded image data does not match a supported PNG/JPEG format".into(),
                );
            }
        };
        let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
            .into_dimensions()
            .map_err(|_| "Embedded image dimensions could not be decoded".to_string())?;
        let pixels = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width > MAX_IMAGE_DIMENSION
            || height > MAX_IMAGE_DIMENSION
            || pixels > MAX_IMAGE_PIXELS
        {
            return Err("Embedded image exceeds safe decoding dimensions".into());
        }
        if bytes.len() <= OPTIMIZE_IMAGE_BYTES && width.max(height) <= self.max_side {
            return Ok((mime, bytes));
        }
        if pixels > self.pixels_remaining {
            return Err("Embedded images exceed the total decoding-pixel budget".into());
        }
        self.pixels_remaining -= pixels;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
        limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
        limits.max_alloc = Some(256 * 1024 * 1024);
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|_| {
            "Embedded image could not be decoded within safe memory limits".to_string()
        })?;
        let mut decoded = if width.max(height) > self.max_side {
            decoded.resize(self.max_side, self.max_side, FilterType::Lanczos3)
        } else {
            decoded
        };
        let transparent = decoded.color().has_alpha()
            && decoded.pixels().any(|(_, _, pixel)| pixel[3] != u8::MAX);
        loop {
            let mut output = Cursor::new(Vec::new());
            let mut output_mime = "image/png";
            if transparent || format == ImageFormat::Png {
                decoded
                    .write_to(&mut output, ImageFormat::Png)
                    .map_err(|_| "Embedded image could not be optimized as PNG".to_string())?;
            }
            // Keep diagrams and screenshots lossless when the resized PNG is compact.
            if !transparent
                && (format != ImageFormat::Png || output.get_ref().len() > OPTIMIZE_IMAGE_BYTES)
            {
                output = Cursor::new(Vec::new());
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 90)
                    .encode_image(&image::DynamicImage::ImageRgb8(decoded.to_rgb8()))
                    .map_err(|_| "Embedded image could not be optimized as JPEG".to_string())?;
                output_mime = "image/jpeg";
            }
            let output = output.into_inner();
            if output.len() <= MAX_INLINE_MEDIA_BYTES {
                // Keep small lossless originals when recompression would only enlarge them.
                if width.max(height) <= self.max_side && bytes.len() <= output.len() {
                    return Ok((mime, bytes));
                }
                return Ok((output_mime, output));
            }
            let next_side = decoded.width().max(decoded.height()) * 3 / 4;
            if next_side < 256 {
                return Err(
                    "Embedded image exceeds the editing budget after bounded resizing".into(),
                );
            }
            decoded = decoded.resize(next_side, next_side, FilterType::Lanczos3);
        }
    }
}

fn media(
    archive: &mut Archive<'_>,
    relation: &Relationship,
    warnings: &mut Vec<String>,
    media_remaining: &mut MediaImporter,
) -> Option<String> {
    if relation.external {
        warn(
            warnings,
            "External media is not fetched or loaded automatically; download the original to retain its link.",
        );
        return None;
    }
    if !media_remaining.cache.contains_key(&relation.target) {
        let result = media_remaining.load(archive, relation);
        media_remaining
            .cache
            .insert(relation.target.clone(), result);
    }
    match &media_remaining.cache[&relation.target] {
        Ok(url) if url.len() <= media_remaining.inline_remaining => {
            media_remaining.inline_remaining -= url.len();
            Some(url.clone())
        }
        Ok(_) => {
            warn(
                warnings,
                "Embedded media exceeds the serialized editing-content budget; the original retains it.",
            );
            None
        }
        Err(reason) => {
            warn(warnings, format!("{reason}; the original retains it."));
            None
        }
    }
}
fn cached_points(node: &Node, remaining: &mut usize) -> Option<Vec<String>> {
    let mut points: BTreeMap<usize, String> = BTreeMap::new();
    for point in node.descendants("pt") {
        let index = point.attr("idx")?.parse::<usize>().ok()?;
        if index >= 100_000 {
            return None;
        }
        points.insert(
            index,
            point.child("v").map(Node::all_text).unwrap_or_default(),
        );
    }
    let length = points.last_key_value().map(|(i, _)| i + 1).unwrap_or(0);
    *remaining = remaining.checked_sub(length)?;
    Some(
        (0..length)
            .map(|i| points.remove(&i).unwrap_or_default())
            .collect(),
    )
}
pub(crate) fn chart_data(
    root: &Node,
    warnings: &mut Vec<String>,
    points_remaining: &mut usize,
) -> Option<(ChartType, ChartData)> {
    chart_data_with_theme(root, warnings, points_remaining, &Theme::default())
}
fn chart_data_with_theme(
    root: &Node,
    warnings: &mut Vec<String>,
    points_remaining: &mut usize,
    theme: &Theme,
) -> Option<(ChartType, ChartData)> {
    let plot = root.desc("plotArea")?;
    let charts: Vec<_> = plot
        .children
        .iter()
        .filter(|n| n.name.ends_with("Chart"))
        .collect();
    if charts.len() > 1 {
        warn(
            warnings,
            "Combination chart axes/types are flattened to the first supported chart type; original chart styling is retained in the uploaded file.",
        );
    }
    let chart = charts.first()?;
    if chart.name.contains("3D")
        || chart
            .child("grouping")
            .is_some_and(|n| !matches!(n.attr("val"), Some("standard" | "clustered")))
    {
        warn(
            warnings,
            "3D or stacked chart layout is approximated as a basic 2D chart; the original retains exact chart orientation/grouping.",
        );
    }
    if plot.desc("logBase").is_some() {
        warn(
            warnings,
            "Logarithmic chart axes are not represented by this model; original axis settings retained in the upload.",
        );
    }

    let kind = match chart.name.as_str() {
        "barChart" | "bar3DChart" => ChartType::Bar,
        "lineChart" | "line3DChart" => ChartType::Line,
        "pieChart" | "pie3DChart" => ChartType::Pie,
        "doughnutChart" => ChartType::Doughnut,
        "scatterChart" => ChartType::Scatter,
        "areaChart" | "area3DChart" => ChartType::Area,
        _ => {
            warn(
                warnings,
                format!("Unsupported chart type {}; original retained.", chart.name),
            );
            return None;
        }
    };
    let mut labels = Vec::new();
    let mut datasets = Vec::new();
    let mut category_source = None;
    for (index, series) in chart
        .children
        .iter()
        .filter(|n| n.name == "ser")
        .enumerate()
    {
        let category = series.child("cat").or_else(|| series.child("xVal"));
        if let Some(category) = category {
            let source = category.all_text();
            if category_source
                .as_ref()
                .is_some_and(|first| first != &source)
            {
                warn(
                    warnings,
                    "Series with different category/X axes share the first imported label axis; the original retains per-series axes.",
                );
            }
            category_source.get_or_insert(source);
        }
        if labels.is_empty()
            && let Some(category) = category
        {
            labels=cached_points(category,points_remaining).or_else(||{warn(warnings,"Chart point expansion budget exceeded; the original retains the complete chart.");None})?;
        }
        let value = series.child("val").or_else(|| series.child("yVal"));
        let raw = if let Some(value) = value {
            cached_points(value,points_remaining).or_else(||{warn(warnings,"Chart point expansion budget exceeded; the original retains the complete chart.");None})?
        } else {
            Vec::new()
        };
        let mut values = Vec::new();
        for raw in raw {
            match raw.parse::<f64>().ok().filter(|n| n.is_finite()) {
                Some(value) => values.push(value),
                None => {
                    values.push(0.0);
                    warn(
                        warnings,
                        "Missing or nonnumeric chart points are represented as zero; the original preserves gaps and errors.",
                    );
                }
            }
        }
        if values.is_empty() {
            warn(
                warnings,
                "Chart has no cached numeric data; external/embedded data bindings are not fully available in the current model.",
            );
        }
        let name = series
            .child("tx")
            .map(|n| {
                n.desc("v")
                    .map(Node::all_text)
                    .unwrap_or_else(|| xml::text(n, "t"))
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("Series {}", index + 1));
        datasets.push(Dataset {
            name,
            values,
            color: series
                .child("spPr")
                .and_then(|p| fill(p, theme))
                .or_else(|| {
                    theme
                        .colors
                        .get(&format!("accent{}", index % 6 + 1))
                        .cloned()
                }),
        });
    }
    if labels.is_empty() {
        let length = datasets.iter().map(|s| s.values.len()).max().unwrap_or(0);
        labels = (1..=length).map(|i| i.to_string()).collect();
    }
    let presentation = chart_presentation(root, chart, theme);
    Some((
        kind,
        ChartData {
            labels,
            datasets,
            presentation: Some(presentation),
        },
    ))
}

fn chart_title(node: &Node) -> Option<String> {
    let title = node.child("title")?;
    let rich = title
        .descendants("t")
        .iter()
        .map(|n| n.all_text())
        .collect::<Vec<_>>()
        .join("");
    let text = if rich.is_empty() {
        title.desc("v").map(Node::all_text).unwrap_or_default()
    } else {
        rich
    };
    (!text.is_empty()).then_some(text)
}
fn chart_presentation(root: &Node, chart: &Node, theme: &Theme) -> ChartPresentation {
    let container = root.desc("chart").unwrap_or(root);
    let text = root.child("txPr").and_then(|n| n.desc("defRPr"));
    let labels = chart
        .child("dLbls")
        .or_else(|| chart.child("ser").and_then(|n| n.child("dLbls")));
    let flag = |name| {
        labels
            .and_then(|n| n.child(name))
            .and_then(|n| n.flag("val"))
            .unwrap_or(false)
    };
    let axis = |name: &str| {
        let n = container.desc(name);
        let grid = n.and_then(|n| n.child("majorGridlines"));
        let line = grid
            .and_then(|n| n.child("spPr"))
            .and_then(|n| n.child("ln"));
        ChartAxis {
            title: n.and_then(chart_title),
            minimum: n
                .and_then(|n| n.child("scaling"))
                .and_then(|n| n.child("min"))
                .and_then(|n| n.num("val")),
            maximum: n
                .and_then(|n| n.child("scaling"))
                .and_then(|n| n.child("max"))
                .and_then(|n| n.num("val")),
            major_unit: n
                .and_then(|n| n.child("majorUnit"))
                .and_then(|n| n.num("val")),
            hidden: n
                .and_then(|n| n.child("delete"))
                .and_then(|n| n.flag("val"))
                .unwrap_or(false),
            number_format: n
                .and_then(|n| n.child("numFmt"))
                .and_then(|n| n.attr("formatCode"))
                .map(str::to_owned),
            major_gridlines: grid.is_some(),
            grid_color: line.and_then(|n| fill(n, theme)),
            grid_width: line.and_then(|n| n.num("w")).map(|n| (n / EMU) as f32),
        }
    };
    let legend = container.child("legend");
    let mut point_colors = Vec::new();
    for series in chart.children.iter().filter(|n| n.name == "ser") {
        let mut colors = Vec::new();
        for point in series.children.iter().filter(|n| n.name == "dPt") {
            if let Some(index) = point
                .child("idx")
                .and_then(|n| n.num("val"))
                .filter(|n| *n >= 0. && *n < 10000.)
                .map(|n| n as usize)
            {
                colors.resize(colors.len().max(index + 1), None);
                colors[index] = point.child("spPr").and_then(|n| fill(n, theme));
            }
        }
        point_colors.push(colors);
    }
    ChartPresentation {
        title: chart_title(container),
        show_legend: legend.is_some(),
        legend_position: legend
            .and_then(|n| n.child("legendPos"))
            .and_then(|n| n.attr("val"))
            .unwrap_or("b")
            .into(),
        labels: ChartLabels {
            show_value: flag("showVal"),
            show_percent: flag("showPercent"),
            show_category: flag("showCatName"),
            show_series: flag("showSerName"),
            position: labels
                .and_then(|n| n.child("dLblPos"))
                .and_then(|n| n.attr("val"))
                .unwrap_or("")
                .into(),
            number_format: labels
                .and_then(|n| n.child("numFmt"))
                .and_then(|n| n.attr("formatCode"))
                .map(str::to_owned),
        },
        category_axis: axis("catAx"),
        value_axis: axis("valAx"),
        font_family: text
            .and_then(|n| n.child("latin"))
            .and_then(|n| n.attr("typeface"))
            .map(str::to_owned),
        font_size: text
            .and_then(|n| n.num("sz"))
            .map(|n| (n / 75.) as f32)
            .or(Some(40. / 3.)),
        title_font_size: container
            .child("title")
            .and_then(|n| n.desc("defRPr").or_else(|| n.desc("rPr")))
            .and_then(|n| n.num("sz"))
            .map(|n| (n / 75.) as f32),
        point_colors,
        vary_colors: chart
            .child("varyColors")
            .and_then(|n| n.flag("val"))
            .unwrap_or(false),
        horizontal: chart.child("barDir").and_then(|n| n.attr("val")) == Some("bar"),
        grouping: chart
            .child("grouping")
            .and_then(|n| n.attr("val"))
            .map(str::to_owned),
        gap_width: chart
            .child("gapWidth")
            .and_then(|n| n.num("val"))
            .map(|n| n.clamp(0., 500.) as f32),
        first_slice_angle: chart
            .child("firstSliceAng")
            .and_then(|n| n.num("val"))
            .map(|n| n as f32),
        hole_size: chart
            .child("holeSize")
            .and_then(|n| n.num("val"))
            .map(|n| n as f32),
    }
}
struct Context<'a, 'b> {
    archive: &'a mut Archive<'b>,
    rels: BTreeMap<String, Relationship>,
    theme: Theme,
    layout: Option<Node>,
    master: Option<Node>,
    warnings: &'a mut Vec<String>,
    ids: BTreeMap<String, Uuid>,
    media_remaining: &'a mut MediaImporter,
    points_remaining: &'a mut usize,
}
impl Context<'_, '_> {
    fn objects(&mut self, tree: &Node, matrix: Matrix) -> Vec<SlideElement> {
        let mut elements = Vec::new();
        for node in &tree.children {
            if let Some(element) = self.object(node, matrix) {
                elements.push(element);
            }
        }
        elements
    }
    fn object(&mut self, node: &Node, matrix: Matrix) -> Option<SlideElement> {
        if !matches!(
            node.name.as_str(),
            "sp" | "cxnSp" | "pic" | "graphicFrame" | "grpSp" | "AlternateContent"
        ) {
            return None;
        }
        if node.name == "AlternateContent" {
            warn(
                self.warnings,
                "Alternate-content objects may use a fallback representation; download the original for unsupported extension content.",
            );
            return node
                .child("Fallback")
                .or_else(|| node.child("Choice"))
                .map(|n| {
                    SlideElement::Group(ElementGroup {
                        id: Uuid::new_v4(),
                        elements: self.objects(n, matrix),
                    })
                });
        }
        let id = Uuid::new_v4();
        if let Some(source) = node.desc("cNvPr").and_then(|n| n.attr("id")) {
            self.ids.insert(source.into(), id);
        }
        if node.name == "grpSp" {
            let transform = group_matrix(node, matrix);
            return Some(SlideElement::Group(ElementGroup {
                id,
                elements: self.objects(node, transform),
            }));
        }
        let fallback = placeholder(node, self.layout.as_ref(), self.master.as_ref()).cloned();
        let mut b = bounds(node, fallback.as_ref(), matrix, self.warnings);
        if node.name == "pic" {
            if let Some(video) = node.desc("videoFile").or_else(|| node.desc("media")) {
                let relation = video
                    .attr("r:link")
                    .or_else(|| video.attr("r:embed"))
                    .and_then(|id| self.rels.get(id))
                    .cloned();
                if let Some(relation) = relation
                    && let Some(url) =
                        media(self.archive, &relation, self.warnings, self.media_remaining)
                {
                    warn(
                        self.warnings,
                        "Embedded video media and geometry are preserved for export; the current editing canvas shows a video placeholder.",
                    );
                    return Some(SlideElement::Video(SlideVideo {
                        id,
                        x: b.x,
                        y: b.y,
                        width: b.width,
                        height: b.height,
                        url,
                        autoplay: false,
                    }));
                }
            }
            if node.desc("audioFile").is_some() {
                warn(
                    self.warnings,
                    "Audio playback settings are not represented; the uploaded original retains the audio.",
                );
            }
            let relation = node
                .desc("blip")
                .and_then(|n| n.attr("r:embed").or_else(|| n.attr("r:link")))
                .and_then(|id| self.rels.get(id))
                .cloned();
            let Some(relation) = relation else {
                warn(
                    self.warnings,
                    "An image relationship is missing; the original retains the image.",
                );
                return None;
            };
            let url = media(self.archive, &relation, self.warnings, self.media_remaining)?;
            let crop = node.desc("srcRect").map(|n| Crop {
                left: (n.num("l").unwrap_or(0.0) / 100000.0) as f32,
                top: (n.num("t").unwrap_or(0.0) / 100000.0) as f32,
                right: (n.num("r").unwrap_or(0.0) / 100000.0) as f32,
                bottom: (n.num("b").unwrap_or(0.0) / 100000.0) as f32,
            });
            let alt_text = node
                .desc("cNvPr")
                .and_then(|n| n.attr("descr").or_else(|| n.attr("name")))
                .unwrap_or_default()
                .into();
            return Some(SlideElement::Image(SlideImage {
                id,
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
                rotation: b.rotation,
                url,
                alt_text,
                crop,
            }));
        }
        if node.name == "graphicFrame" {
            if let Some(chart) = node
                .desc("chart")
                .and_then(|n| n.attr("r:id"))
                .and_then(|id| self.rels.get(id))
                .cloned()
                .filter(|chart| !chart.external)
                && let Ok(root) = xml::part(self.archive, &chart.target)
                && let Some((chart_type, data)) =
                    chart_data_with_theme(&root, self.warnings, self.points_remaining, &self.theme)
            {
                return Some(SlideElement::Chart(SlideChart {
                    id,
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                    chart_type,
                    data,
                }));
            }
            if let Some(table) = node.desc("tbl") {
                if table.descendants("gridCol").len() > 1000
                    || table.children.iter().filter(|n| n.name == "tr").count() > 1000
                {
                    warn(
                        self.warnings,
                        "DrawingML table exceeds the supported grid budget; the entire table is omitted from editing and the original retains the complete table.",
                    );
                    return None;
                }
                let columns: Vec<f32> = table
                    .descendants("gridCol")
                    .iter()
                    .map(|n| (n.num("w").unwrap_or(EMU) / EMU).clamp(0., 10_000_000.) as f32)
                    .collect();
                let mut row_heights = Vec::new();
                let mut cells = Vec::new();
                for (ri, row) in table.children.iter().filter(|n| n.name == "tr").enumerate() {
                    row_heights.push(
                        (row.num("h").unwrap_or(EMU * 20.0) / EMU).clamp(0., 10_000_000.) as f32,
                    );
                    for (ci, cell) in row.children.iter().filter(|n| n.name == "tc").enumerate() {
                        if ci >= columns.len() {
                            warn(
                                self.warnings,
                                "DrawingML table cells exceed the declared grid budget; the entire table is omitted from editing and the original retains the complete table.",
                            );
                            return None;
                        }
                        if cell.flag("hMerge") == Some(true) || cell.flag("vMerge") == Some(true) {
                            continue;
                        }
                        if cells.len() >= 50000 {
                            warn(
                                self.warnings,
                                "DrawingML table exceeds the supported cell budget; the entire table is omitted from editing and the original retains the complete table.",
                            );
                            return None;
                        }
                        let props = cell.child("tcPr");
                        let mut layout = text_layout(cell, None).unwrap();
                        let margin = |name: &str, default: f32| {
                            props
                                .and_then(|n| n.num(name))
                                .map(|n| (n / EMU) as f32)
                                .unwrap_or(default)
                        };
                        layout.inset_left = margin("marL", 9.6);
                        layout.inset_right = margin("marR", 9.6);
                        layout.inset_top = margin("marT", 4.8);
                        layout.inset_bottom = margin("marB", 4.8);
                        layout.vertical_alignment = match props.and_then(|n| n.attr("anchor")) {
                            Some("ctr") => "center",
                            Some("b") => "bottom",
                            _ => "top",
                        }
                        .into();
                        let edge = |name: &str| {
                            props
                                .and_then(|n| n.child(name))
                                .filter(|n| n.child("noFill").is_none())
                                .map(|n| SlideBorder {
                                    color: fill(n, &self.theme).unwrap_or_else(|| "#000000".into()),
                                    width: (n.num("w").unwrap_or(9525.) / EMU) as f32,
                                })
                        };
                        let content = runs(cell, None, &self.theme, &self.rels, self.warnings);
                        cells.push(SlideTableCell {
                            row: ri,
                            column: ci,
                            row_span: cell.num("rowSpan").unwrap_or(1.).clamp(1., 1000.) as usize,
                            column_span: cell
                                .num("gridSpan")
                                .unwrap_or(1.)
                                .clamp(1., columns.len().max(1) as f64)
                                as usize,
                            content,
                            background_color: props.and_then(|n| fill(n, &self.theme)),
                            text_layout: layout,
                            borders: TableCellBorders {
                                top: edge("lnT"),
                                right: edge("lnR"),
                                bottom: edge("lnB"),
                                left: edge("lnL"),
                            },
                        });
                    }
                }
                return Some(SlideElement::Table(SlideTable {
                    id,
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                    rotation: b.rotation,
                    column_widths: columns,
                    row_heights,
                    cells,
                }));
            }
            warn(
                self.warnings,
                "Unsupported graphic frame (such as SmartArt or an embedded object); the original retains it.",
            );
            return None;
        }
        let props = properties(node).or_else(|| fallback.as_ref().and_then(properties));
        let content = runs(
            node,
            fallback.as_ref(),
            &self.theme,
            &self.rels,
            self.warnings,
        );
        if props.is_some_and(|p| {
            p.child("gradFill").is_some()
                || p.child("blipFill").is_some()
                || p.child("effectLst").is_some_and(|n| !n.children.is_empty())
        }) {
            warn(
                self.warnings,
                "Shape gradient/picture fills and visual effects are not represented exactly; original appearance remains available in the upload.",
            );
        }
        let preset = props
            .and_then(|p| p.child("prstGeom"))
            .and_then(|p| p.attr("prst"));
        if props.is_some_and(|p| p.child("custGeom").is_some()) {
            warn(
                self.warnings,
                "Custom vector paths are approximated by a rectangle; original paths remain in the uploaded file.",
            );
        }
        let text_box = node.desc("cNvSpPr").and_then(|p| p.flag("txBox")) == Some(true)
            || preset.is_none()
            || (!content.is_empty() && preset == Some("rect"));
        if text_box {
            return Some(SlideElement::TextBox(TextBox {
                id,
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
                rotation: b.rotation,
                content,
                background_color: props.and_then(|p| fill(p, &self.theme)),
                border: props.and_then(|p| border(p, &self.theme)),
                padding: node
                    .child("txBody")
                    .and_then(|n| n.child("bodyPr"))
                    .and_then(|n| n.num("lIns"))
                    .map(|n| (n / EMU) as f32)
                    .unwrap_or(8.0),
                text_layout: text_layout(node, fallback.as_ref()),
            }));
        }
        let adjustment = match preset {
            Some("leftArrow") => 180.0,
            Some("upArrow") => -90.0,
            Some("downArrow") => 90.0,
            _ => 0.0,
        };
        if adjustment == 90.0 || adjustment == -90.0 {
            let cx = b.x + b.width / 2.;
            let cy = b.y + b.height / 2.;
            std::mem::swap(&mut b.width, &mut b.height);
            b.x = cx - b.width / 2.;
            b.y = cy - b.height / 2.;
        }
        b.rotation = (b.rotation + adjustment) % 360.;
        if matches!(
            preset,
            Some("star4" | "rtTriangle" | "arc" | "curvedConnector2" | "curvedConnector3")
        ) {
            warn(
                self.warnings,
                "A shape preset is approximated by the closest supported geometry; the original retains its exact path.",
            );
        }
        let shape_type = preset.and_then(shape_type).unwrap_or_else(|| {
            warn(
                self.warnings,
                format!(
                    "Unsupported preset shape {}; approximated as a rectangle.",
                    preset.unwrap_or("custom")
                ),
            );
            ShapeType::Rectangle
        });
        Some(SlideElement::Shape(Shape {
            id,
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
            rotation: b.rotation,
            shape_type,
            fill_color: props.and_then(|p| fill(p, &self.theme)).or_else(|| {
                node.child("style")
                    .and_then(|n| n.child("fillRef"))
                    .and_then(|n| n.children.iter().find_map(|c| color(c, &self.theme)))
            }),
            border: props.and_then(|p| border(p, &self.theme)).or_else(|| {
                node.child("style")
                    .and_then(|n| n.child("lnRef"))
                    .and_then(|n| n.children.iter().find_map(|c| color(c, &self.theme)))
                    .map(|color| SlideBorder { color, width: 1.0 })
            }),
            text: (!content.is_empty()).then_some(content),
        }))
    }
}
fn animations(
    node: &Node,
    ids: &BTreeMap<String, Uuid>,
    out: &mut Vec<Animation>,
    warnings: &mut Vec<String>,
) {
    if ((node.name == "par"
        && node.child("cTn").is_some_and(|n| {
            n.attr("presetID").is_some()
                || matches!(
                    n.attr("nodeType"),
                    Some("clickEffect" | "withEffect" | "afterEffect")
                )
        }))
        || matches!(node.name.as_str(), "animEffect" | "animScale" | "set"))
        && let Some(effect) = node
            .desc("animEffect")
            .or_else(|| node.desc("animScale"))
            .or_else(|| node.desc("set"))
            .or_else(|| {
                matches!(node.name.as_str(), "animEffect" | "animScale" | "set").then_some(node)
            })
    {
        let target = effect
            .desc("spTgt")
            .and_then(|n| n.attr("spid"))
            .and_then(|id| ids.get(id))
            .copied();
        if let Some(target_id) = target {
            let timing = node.child("cTn");
            let preset = timing.and_then(|n| n.attr("presetID")).unwrap_or_default();
            let filter = effect.attr("filter").unwrap_or_default();
            let kind = if effect.name == "set" {
                AnimationEffect::Appear
            } else if filter.contains("fade") {
                AnimationEffect::Fade
            } else if filter.contains("zoom") || effect.name == "animScale" {
                AnimationEffect::Zoom
            } else if filter.contains("fly") || filter.contains("slide") {
                AnimationEffect::FlyIn
            } else if filter.contains("pulse") {
                AnimationEffect::Pulse
            } else {
                match preset {
                    "1" => AnimationEffect::Appear,
                    "10" => AnimationEffect::Fade,
                    "2" => AnimationEffect::FlyIn,
                    _ => {
                        warn(
                            warnings,
                            "Unsupported animation effects are approximated; complex timing paths remain available in the original.",
                        );
                        AnimationEffect::Appear
                    }
                }
            };
            let kind = if effect.attr("transition") == Some("out") {
                AnimationEffect::Disappear
            } else {
                kind
            };
            let duration = effect
                .descendants("cTn")
                .iter()
                .find_map(|n| n.num("dur"))
                .or_else(|| timing.and_then(|n| n.num("dur")))
                .unwrap_or(500.0);
            let trigger = match timing
                .and_then(|n| n.attr("nodeType"))
                .unwrap_or("clickEffect")
            {
                "withEffect" => AnimationTrigger::WithPrevious,
                "afterEffect" => AnimationTrigger::AfterPrevious,
                _ => AnimationTrigger::OnClick,
            };
            let delay = timing
                .and_then(|n| n.child("stCondLst"))
                .and_then(|n| n.desc("cond"))
                .and_then(|n| n.num("delay"))
                .unwrap_or(0.0);
            out.push(Animation {
                target_id,
                effect: kind,
                trigger,
                duration_ms: duration.clamp(0.0, u32::MAX as f64) as u32,
                delay_ms: delay.clamp(0.0, u32::MAX as f64) as u32,
                order: out.len() as u32,
            });
            return;
        }
    }
    for child in &node.children {
        animations(child, ids, out, warnings);
    }
}
pub(super) fn import(
    archive: &mut Archive<'_>,
    paths: &[String],
    title: String,
) -> Result<Presentation, String> {
    let presentation = xml::part(archive, "ppt/presentation.xml")?;
    let mut result = Presentation::new(title);
    result.slides.clear();
    if let Some(size) = presentation.child("sldSz") {
        result.metadata.slide_width = (size.num("cx").unwrap_or(18288000.0) / EMU) as f32;
        result.metadata.slide_height = (size.num("cy").unwrap_or(10287000.0) / EMU) as f32;
    }
    if ![result.metadata.slide_width, result.metadata.slide_height]
        .iter()
        .all(|n| n.is_finite() && *n > 0.0 && *n <= 100_000.0)
    {
        return Err("Unsupported presentation page dimensions".into());
    }
    let mut used_themes = BTreeSet::new();
    result.metadata.pptx_import_revision = 3;
    let mut media_remaining =
        MediaImporter::new(result.metadata.slide_width, result.metadata.slide_height);
    let mut points_remaining = syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES / 200;
    for path in paths {
        let root = xml::part(archive, path)?;
        let rels = xml::relationships(archive, path)?;
        let layout_part = rels
            .values()
            .find(|r| r.kind.ends_with("/slideLayout") && !r.external)
            .map(|r| r.target.clone());
        let layout = layout_part
            .as_deref()
            .map(|p| xml::optional(archive, p))
            .transpose()?
            .flatten();
        let layout_rels = layout_part
            .as_deref()
            .map(|p| xml::relationships(archive, p))
            .transpose()?
            .unwrap_or_default();
        let master_part = layout_rels
            .values()
            .find(|r| r.kind.ends_with("/slideMaster") && !r.external)
            .map(|r| r.target.clone());
        let master = master_part
            .as_deref()
            .map(|p| xml::optional(archive, p))
            .transpose()?
            .flatten();
        let master_rels = master_part
            .as_deref()
            .map(|p| xml::relationships(archive, p))
            .transpose()?
            .unwrap_or_default();
        let theme_part = master_rels
            .values()
            .chain(rels.values())
            .find(|r| r.kind.ends_with("/theme") && !r.external)
            .map(|r| r.target.clone());
        let theme_node = theme_part
            .as_deref()
            .map(|p| xml::optional(archive, p))
            .transpose()?
            .flatten();
        let mut theme = theme(theme_node.as_ref());
        apply_color_map(&mut theme, master.as_ref().and_then(|n| n.child("clrMap")));
        apply_color_map(
            &mut theme,
            layout.as_ref().and_then(|n| n.desc("overrideClrMapping")),
        );
        apply_color_map(&mut theme, root.desc("overrideClrMapping"));
        if let Some(path) = &theme_part {
            used_themes.insert(path.clone());
        }
        result.theme.heading_font = theme.major.clone();
        result.theme.body_font = theme.minor.clone();
        result.theme.accent_colors = (1..=6)
            .filter_map(|i| theme.colors.get(&format!("accent{i}")).cloned())
            .collect();
        let mut slide = Slide::new(
            match layout
                .as_ref()
                .and_then(|n| n.attr("type"))
                .unwrap_or("blank")
            {
                "title" => SlideLayout::TitleSlide,
                "obj" | "tx" => SlideLayout::TitleAndContent,
                "twoObj" | "twoColTx" => SlideLayout::TwoColumn,
                "secHead" => SlideLayout::SectionHeader,
                "titleOnly" => SlideLayout::TitleOnly,
                _ => SlideLayout::Blank,
            },
        );
        let (background, background_rels) =
            if let Some(bg) = root.desc("cSld").and_then(|n| n.child("bg")) {
                (Some(bg), &rels)
            } else if let Some(bg) = layout.as_ref().and_then(|n| n.desc("bg")) {
                (Some(bg), &layout_rels)
            } else {
                (master.as_ref().and_then(|n| n.desc("bg")), &master_rels)
            };
        let themed_background = background.and_then(|n| theme_background(n, &theme));
        let background_theme = themed_background
            .as_ref()
            .map(|(_, theme)| theme)
            .unwrap_or(&theme);
        let background = themed_background
            .as_ref()
            .map(|(node, _)| node)
            .or(background);
        let theme_background_rels = if themed_background.is_some() {
            theme_part
                .as_deref()
                .map(|part| xml::relationships(archive, part))
                .transpose()?
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        let background_rels = if themed_background.is_some() {
            &theme_background_rels
        } else {
            background_rels
        };
        if let Some(background) = background {
            if let Some(blip) = background.desc("blip") {
                if let Some(relation) = blip
                    .attr("r:embed")
                    .or_else(|| blip.attr("r:link"))
                    .and_then(|id| background_rels.get(id))
                {
                    if let Some(url) = media(
                        archive,
                        relation,
                        &mut result.metadata.import_warnings,
                        &mut media_remaining,
                    ) {
                        slide.background = Background::Image(url);
                    }
                } else {
                    warn(
                        &mut result.metadata.import_warnings,
                        "Background image relationship is missing; the original retains the image.",
                    );
                }
                if background.desc("srcRect").is_some() {
                    warn(
                        &mut result.metadata.import_warnings,
                        "Background image crop/tiling is approximated as a stretched image; original retains its exact fill.",
                    );
                }
            }
            if let Some(value) = background
                .desc("solidFill")
                .and_then(|n| color(n, background_theme))
            {
                slide.background = Background::Solid(value);
            } else if let Some(gradient) = background.desc("gradFill") {
                let stops: Vec<_> = gradient
                    .descendants("gs")
                    .iter()
                    .filter_map(|n| n.children.iter().find_map(|c| color(c, background_theme)))
                    .collect();
                if let (Some(first), Some(last)) = (stops.first(), stops.last()) {
                    slide.background = Background::Gradient(
                        first.clone(),
                        last.clone(),
                        ((gradient
                            .desc("lin")
                            .and_then(|n| n.num("ang"))
                            .unwrap_or(0.0)
                            / 60000.0)
                            .rem_euclid(360.0)) as f32,
                    );
                }
            }
        }
        let mut inherited_elements = Vec::new();
        if root.flag("showMasterSp") != Some(false) {
            for (kind, (inherited, inherited_rels)) in [
                (master.as_ref(), &master_rels),
                (layout.as_ref(), &layout_rels),
            ]
            .into_iter()
            .enumerate()
            {
                if kind == 0
                    && layout
                        .as_ref()
                        .is_some_and(|n| n.flag("showMasterSp") == Some(false))
                {
                    continue;
                }
                if let Some(tree) = inherited.and_then(|n| n.desc("spTree")) {
                    let mut inherited_context = Context {
                        archive,
                        rels: inherited_rels.clone(),
                        theme: theme.clone(),
                        layout: None,
                        master: None,
                        warnings: &mut result.metadata.import_warnings,
                        ids: BTreeMap::new(),
                        media_remaining: &mut media_remaining,
                        points_remaining: &mut points_remaining,
                    };
                    for node in tree.children.iter().filter(|n| n.desc("ph").is_none()) {
                        if let Some(element) = inherited_context.object(node, Matrix::default()) {
                            inherited_elements.push(element);
                        }
                    }
                }
            }
        }
        let tree = root
            .desc("spTree")
            .ok_or("Presentation slide has no shape tree")?;
        let mut context = Context {
            archive,
            rels: rels.clone(),
            theme,
            layout,
            master,
            warnings: &mut result.metadata.import_warnings,
            ids: BTreeMap::new(),
            media_remaining: &mut media_remaining,
            points_remaining: &mut points_remaining,
        };
        inherited_elements.extend(context.objects(tree, Matrix::default()));
        slide.elements = inherited_elements;
        if root.desc("timing").is_some() {
            warn(
                context.warnings,
                "Representable animation timing is preserved for export; the editing canvas does not play the full PowerPoint animation timeline.",
            );
            animations(&root, &context.ids, &mut slide.animations, context.warnings);
            if slide.animations.is_empty() {
                warn(
                    context.warnings,
                    "Slide animation timing is outside the supported subset; use the original for its exact playback.",
                );
            }
        }
        if let Some(transition) = root.child("transition") {
            let effect = transition
                .children
                .first()
                .map(|n| n.name.as_str())
                .unwrap_or("none");
            let kind = match effect {
                "fade" => TransitionType::Fade,
                "push" | "cover" | "pull" => TransitionType::Push,
                "zoom" => TransitionType::Zoom,
                "dissolve" => TransitionType::Dissolve,
                "flip" => TransitionType::Flip,
                "wipe" => TransitionType::Slide,
                _ => {
                    warn(
                        context.warnings,
                        format!("Unsupported slide transition {effect}; the original retains it."),
                    );
                    TransitionType::None
                }
            };
            let duration = transition
                .num("dur")
                .unwrap_or_else(|| match transition.attr("spd") {
                    Some("slow") => 1000.0,
                    Some("fast") => 200.0,
                    _ => 500.0,
                });
            slide.transition = Some(Transition {
                transition_type: kind,
                duration_ms: duration.clamp(0.0, u32::MAX as f64) as u32,
            });
            if transition.attr("advTm").is_some() {
                warn(
                    context.warnings,
                    "Automatic slide-advance timing is not represented by the current transition model; the original retains it.",
                );
            }
        }
        if let Some(notes) = rels
            .values()
            .find(|r| r.kind.ends_with("/notesSlide") && !r.external)
            && let Some(notes) = xml::optional(context.archive, &notes.target)?
        {
            slide.notes = notes
                .descendants("sp")
                .into_iter()
                .filter(|n| {
                    !n.desc("ph").is_some_and(|p| {
                        matches!(p.attr("type"), Some("sldNum" | "dt" | "hdr" | "ftr"))
                    })
                })
                .map(|n| xml::text(n, "t"))
                .collect::<Vec<_>>()
                .join("\n");
        }
        if !root.descendants("oleObj").is_empty() || !root.descendants("custDataLst").is_empty() {
            warn(
                context.warnings,
                "Embedded OLE/custom application content is not editable in this model; download the original to preserve it.",
            );
        }
        result.slides.push(slide);
    }
    if used_themes.len() > 1 {
        warn(
            &mut result.metadata.import_warnings,
            "Multiple slide master themes are flattened to explicit imported colors/fonts; theme switching may differ from the original.",
        );
    }
    xml::model_budget(&result)?;
    Ok(result)
}

#[cfg(test)]
#[path = "pptx_text_background_tests.rs"]
mod text_background_tests;
