//! PPTX export and import for Slides presentations.
//!
//! Converts a [`Presentation`] into a `.pptx` byte vector (ZIP archive with
//! Open XML) and parses a `.pptx` byte slice back into a [`Presentation`].

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use contrix_doc_engine::slides::{
    ChartType, ElementGroup, Presentation, Shape, ShapeType, Slide, SlideChart, SlideElement,
    SlideImage, SlideLayout, SlideTextRun, SlideVideo, TextBox,
};
use quick_xml::Reader;
use quick_xml::events::Event;
use uuid::Uuid;
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

const MAX_MEDIA_BYTES: usize = 50 * 1024 * 1024;

#[derive(Clone)]
struct MediaPart {
    relationship_id: String,
    file_name: String,
    bytes: Vec<u8>,
    external_url: Option<String>,
}

#[derive(Default)]
struct SlideResources {
    images: HashMap<Uuid, MediaPart>,
    videos: HashMap<Uuid, MediaPart>,
    charts: HashMap<Uuid, (String, usize)>,
}

/// Convert a [`Presentation`] to a `.pptx` byte vector (ZIP archive with Open XML).
pub fn presentation_to_pptx(pres: &Presentation) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut buf);
    let options = SimpleFileOptions::default();
    let mut media_index = 0usize;
    let mut chart_index = 0usize;
    let resources: Vec<SlideResources> = pres
        .slides
        .iter()
        .map(|slide| collect_slide_resources(slide, &mut media_index, &mut chart_index))
        .collect();

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| e.to_string())?;
    let mut ct = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="jpg" ContentType="image/jpeg"/>
  <Default Extension="jpeg" ContentType="image/jpeg"/>
  <Default Extension="gif" ContentType="image/gif"/>
  <Default Extension="mp4" ContentType="video/mp4"/>
  <Override PartName="/customXml/contrix-presentation.json" ContentType="application/json"/>
  <Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
  <Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
  <Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>"#,
    );
    for (i, _) in pres.slides.iter().enumerate() {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            i + 1
        ));
        if !pres.slides[i].notes.is_empty() {
            ct.push_str(&format!(
                "\n  <Override PartName=\"/ppt/notesSlides/notesSlide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/>",
                i + 1
            ));
        }
    }
    for chart_number in 1..=chart_index {
        ct.push_str(&format!(
            "\n  <Override PartName=\"/ppt/charts/chart{chart_number}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>"
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
    rels.push_str("\n  <Relationship Id=\"rIdMaster\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>");
    rels.push_str("\n</Relationships>");
    write!(zip, "{}", rels).map_err(|e| e.to_string())?;

    // ppt/presentation.xml
    zip.start_file("ppt/presentation.xml", options)
        .map_err(|e| e.to_string())?;
    let mut p = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdMaster"/></p:sldMasterIdLst>
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

    // Preserve the complete native model for lossless Contrix round-trips.
    // Office applications ignore this namespaced package part while rendering
    // the standards-based shapes/media/chart parts below.
    zip.start_file("customXml/contrix-presentation.json", options)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut zip, pres).map_err(|e| e.to_string())?;

    zip.start_file("ppt/theme/theme1.xml", options)
        .map_err(|e| e.to_string())?;
    write!(zip, "{}", theme_xml(pres)).map_err(|e| e.to_string())?;

    zip.start_file("ppt/slideMasters/slideMaster1.xml", options)
        .map_err(|e| e.to_string())?;
    write!(zip, "{}", slide_master_xml(pres)).map_err(|e| e.to_string())?;
    zip.start_file("ppt/slideMasters/_rels/slideMaster1.xml.rels", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdTheme" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/><Relationship Id="rIdLayout" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#.as_bytes()).map_err(|e| e.to_string())?;
    zip.start_file("ppt/slideLayouts/slideLayout1.xml", options)
        .map_err(|e| e.to_string())?;
    write!(zip, "{}", slide_layout_xml()).map_err(|e| e.to_string())?;
    zip.start_file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdMaster" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#.as_bytes()).map_err(|e| e.to_string())?;

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
    for (i, (slide, slide_resources)) in pres.slides.iter().zip(resources.iter()).enumerate() {
        zip.start_file(format!("ppt/slides/_rels/slide{}.xml.rels", i + 1), options)
            .map_err(|e| e.to_string())?;
        write!(
            zip,
            "{}",
            slide_relationships_xml(slide_resources, !slide.notes.is_empty(), i + 1)
        )
        .map_err(|e| e.to_string())?;

        zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), options)
            .map_err(|e| e.to_string())?;
        let xml = slide_to_xml(slide, slide_resources);
        write!(zip, "{}", xml).map_err(|e| e.to_string())?;

        if !slide.notes.is_empty() {
            zip.start_file(format!("ppt/notesSlides/notesSlide{}.xml", i + 1), options)
                .map_err(|e| e.to_string())?;
            write!(zip, "{}", notes_slide_xml(&slide.notes)).map_err(|e| e.to_string())?;
        }
    }

    for slide_resources in &resources {
        for media in slide_resources
            .images
            .values()
            .chain(slide_resources.videos.values())
        {
            if !media.bytes.is_empty() {
                zip.start_file(format!("ppt/media/{}", media.file_name), options)
                    .map_err(|e| e.to_string())?;
                zip.write_all(&media.bytes).map_err(|e| e.to_string())?;
            }
        }
    }
    for (slide, slide_resources) in pres.slides.iter().zip(resources.iter()) {
        write_chart_parts(&mut zip, options, &slide.elements, slide_resources)?;
    }

    zip.finish().map_err(|e| e.to_string())?;
    crate::native_package::seal(buf.into_inner(), "customXml/contrix-presentation.json")
}

fn decode_data_url(url: &str) -> Option<(&'static str, Vec<u8>)> {
    let (metadata, encoded) = url.strip_prefix("data:")?.split_once(',')?;
    if !metadata.ends_with(";base64") {
        return None;
    }
    let extension = match metadata
        .trim_end_matches(";base64")
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        _ => return None,
    };
    let bytes = BASE64.decode(encoded).ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_MEDIA_BYTES).then_some((extension, bytes))
}

fn collect_slide_resources(
    slide: &Slide,
    media_index: &mut usize,
    chart_index: &mut usize,
) -> SlideResources {
    fn visit(
        elements: &[SlideElement],
        resources: &mut SlideResources,
        relationship_index: &mut usize,
        media_index: &mut usize,
        chart_index: &mut usize,
    ) {
        for element in elements {
            match element {
                SlideElement::Image(image) => {
                    *relationship_index += 1;
                    *media_index += 1;
                    let relationship_id = format!("rId{relationship_index}");
                    let part = if let Some((extension, bytes)) = decode_data_url(&image.url) {
                        MediaPart {
                            relationship_id,
                            file_name: format!("image{media_index}.{extension}"),
                            bytes,
                            external_url: None,
                        }
                    } else {
                        MediaPart {
                            relationship_id,
                            file_name: String::new(),
                            bytes: Vec::new(),
                            external_url: image.url.starts_with("http").then(|| image.url.clone()),
                        }
                    };
                    resources.images.insert(image.id, part);
                }
                SlideElement::Video(video) => {
                    *relationship_index += 1;
                    *media_index += 1;
                    let relationship_id = format!("rId{relationship_index}");
                    let part = if let Some((extension, bytes)) = decode_data_url(&video.url) {
                        MediaPart {
                            relationship_id,
                            file_name: format!("video{media_index}.{extension}"),
                            bytes,
                            external_url: None,
                        }
                    } else {
                        MediaPart {
                            relationship_id,
                            file_name: String::new(),
                            bytes: Vec::new(),
                            external_url: video.url.starts_with("http").then(|| video.url.clone()),
                        }
                    };
                    resources.videos.insert(video.id, part);
                }
                SlideElement::Chart(chart) => {
                    *relationship_index += 1;
                    *chart_index += 1;
                    resources
                        .charts
                        .insert(chart.id, (format!("rId{relationship_index}"), *chart_index));
                }
                SlideElement::Group(group) => visit(
                    &group.elements,
                    resources,
                    relationship_index,
                    media_index,
                    chart_index,
                ),
                SlideElement::TextBox(_) | SlideElement::Shape(_) => {}
            }
        }
    }

    let mut resources = SlideResources::default();
    let mut relationship_index = 0usize;
    visit(
        &slide.elements,
        &mut resources,
        &mut relationship_index,
        media_index,
        chart_index,
    );
    resources
}

fn slide_relationships_xml(
    resources: &SlideResources,
    has_notes: bool,
    slide_number: usize,
) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    xml.push_str("<Relationship Id=\"rIdLayout\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>");
    for media in resources.images.values() {
        if let Some(url) = &media.external_url {
            xml.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\" TargetMode=\"External\"/>",
                media.relationship_id,
                escape_xml(url)
            ));
        } else if !media.bytes.is_empty() {
            xml.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{}\"/>",
                media.relationship_id, media.file_name
            ));
        }
    }
    for media in resources.videos.values() {
        if let Some(url) = &media.external_url {
            xml.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/video\" Target=\"{}\" TargetMode=\"External\"/>",
                media.relationship_id,
                escape_xml(url)
            ));
        } else if !media.bytes.is_empty() {
            xml.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/video\" Target=\"../media/{}\"/>",
                media.relationship_id, media.file_name
            ));
        }
    }
    for (relationship_id, chart_number) in resources.charts.values() {
        xml.push_str(&format!(
            "<Relationship Id=\"{relationship_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"../charts/chart{chart_number}.xml\"/>"
        ));
    }
    if has_notes {
        xml.push_str(&format!(
            "<Relationship Id=\"rIdNotes\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide{slide_number}.xml\"/>"
        ));
    }
    xml.push_str("</Relationships>");
    xml
}

fn theme_xml(pres: &Presentation) -> String {
    let accents = (0..6)
        .map(|index| {
            pres.theme
                .accent_colors
                .get(index)
                .cloned()
                .unwrap_or_else(|| pres.theme.primary_color.clone())
                .trim_start_matches('#')
                .to_string()
        })
        .collect::<Vec<_>>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{}"><a:themeElements><a:clrScheme name="{}"><a:dk1><a:srgbClr val="{}"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="{}"/></a:dk2><a:lt2><a:srgbClr val="F1F5F9"/></a:lt2><a:accent1><a:srgbClr val="{}"/></a:accent1><a:accent2><a:srgbClr val="{}"/></a:accent2><a:accent3><a:srgbClr val="{}"/></a:accent3><a:accent4><a:srgbClr val="{}"/></a:accent4><a:accent5><a:srgbClr val="{}"/></a:accent5><a:accent6><a:srgbClr val="{}"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Contrix"><a:majorFont><a:latin typeface="{}"/></a:majorFont><a:minorFont><a:latin typeface="{}"/></a:minorFont></a:fontScheme><a:fmtScheme name="Contrix"><a:fillStyleLst/><a:lnStyleLst/><a:effectStyleLst/><a:bgFillStyleLst/></a:fmtScheme></a:themeElements></a:theme>"#,
        escape_xml(&pres.theme.name),
        escape_xml(&pres.theme.name),
        pres.theme.secondary_color.trim_start_matches('#'),
        pres.theme.primary_color.trim_start_matches('#'),
        accents[0],
        accents[1],
        accents[2],
        accents[3],
        accents[4],
        accents[5],
        escape_xml(&pres.theme.heading_font),
        escape_xml(&pres.theme.body_font),
    )
}

fn slide_master_xml(pres: &Presentation) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" bg1="lt1" bg2="lt2" folHlink="folHlink" hlink="hlink" tx1="dk1" tx2="dk2"/><p:sldLayoutIdLst><p:sldLayoutId id="1" r:id="rIdLayout"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle/><p:bodyStyle/><p:otherStyle/></p:txStyles></p:sldMaster>"#,
        pres.theme.primary_color.trim_start_matches('#')
    )
}

fn slide_layout_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1"><p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
}

fn notes_slide_xml(notes: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#,
        escape_xml(notes)
    )
}

fn slide_to_xml(slide: &Slide, resources: &SlideResources) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld>"#,
    );
    background_to_xml(&slide.background, &mut xml);
    xml.push_str(
        r#"<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>"#,
    );

    let mut next_id = 2u32;
    let mut shape_ids = HashMap::new();
    for element in &slide.elements {
        element_to_xml(element, resources, &mut next_id, &mut shape_ids, &mut xml);
    }

    xml.push_str("</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>");
    transition_to_xml(slide, &mut xml);
    animations_to_xml(slide, &shape_ids, &mut xml);
    xml.push_str("</p:sld>");
    xml
}

fn element_to_xml(
    element: &SlideElement,
    resources: &SlideResources,
    next_id: &mut u32,
    shape_ids: &mut HashMap<Uuid, u32>,
    xml: &mut String,
) {
    let sp_id = *next_id;
    *next_id = next_id.saturating_add(1);
    let element_id = match element {
        SlideElement::TextBox(value) => value.id,
        SlideElement::Image(value) => value.id,
        SlideElement::Shape(value) => value.id,
        SlideElement::Chart(value) => value.id,
        SlideElement::Video(value) => value.id,
        SlideElement::Group(value) => value.id,
    };
    shape_ids.insert(element_id, sp_id);
    match element {
        SlideElement::TextBox(value) => text_box_to_xml(value, sp_id, xml),
        SlideElement::Shape(value) => shape_to_xml(value, sp_id, xml),
        SlideElement::Image(value) => {
            image_to_xml(value, resources.images.get(&value.id), sp_id, xml)
        }
        SlideElement::Chart(value) => {
            chart_to_xml(value, resources.charts.get(&value.id), sp_id, xml)
        }
        SlideElement::Video(value) => {
            video_to_xml(value, resources.videos.get(&value.id), sp_id, xml)
        }
        SlideElement::Group(value) => {
            group_to_xml(value, resources, sp_id, next_id, shape_ids, xml)
        }
    }
}

/// Map an internal [`ShapeType`] to an OOXML preset geometry name.
fn shape_preset(shape_type: ShapeType) -> &'static str {
    match shape_type {
        ShapeType::Rectangle => "rect",
        ShapeType::RoundedRectangle => "roundRect",
        ShapeType::Circle | ShapeType::Ellipse => "ellipse",
        ShapeType::Triangle => "triangle",
        ShapeType::Arrow => "rightArrow",
        ShapeType::Star => "star5",
        ShapeType::Diamond => "diamond",
        ShapeType::Pentagon => "pentagon",
        ShapeType::Hexagon => "hexagon",
        ShapeType::Line => "line",
        ShapeType::CurvedLine => "curvedConnector3",
    }
}

fn shape_to_xml(shape: &Shape, sp_id: u32, xml: &mut String) {
    let x_emu = (shape.x * 9525.0) as i64;
    let y_emu = (shape.y * 9525.0) as i64;
    let w_emu = (shape.width * 9525.0) as i64;
    let h_emu = (shape.height * 9525.0) as i64;
    let prst = shape_preset(shape.shape_type);

    xml.push_str(&format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{sp_id}\" name=\"Shape {sp_id}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>"
    ));
    xml.push_str(&format!(
        "<p:spPr><a:xfrm><a:off x=\"{x_emu}\" y=\"{y_emu}\"/><a:ext cx=\"{w_emu}\" cy=\"{h_emu}\"/></a:xfrm><a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>"
    ));
    if let Some(ref fill) = shape.fill_color {
        let c = fill.trim_start_matches('#');
        xml.push_str(&format!(
            "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
            escape_xml(c)
        ));
    }
    xml.push_str("</p:spPr>");
    xml.push_str("<p:txBody><a:bodyPr/><a:lstStyle/><a:p>");
    if let Some(ref runs) = shape.text {
        for run in runs {
            run_to_xml(run, xml);
        }
    }
    xml.push_str("</a:p></p:txBody></p:sp>");
}

fn image_to_xml(img: &SlideImage, media: Option<&MediaPart>, sp_id: u32, xml: &mut String) {
    let Some(media) = media.filter(|media| !media.bytes.is_empty() || media.external_url.is_some())
    else {
        placeholder_box_to_xml(
            img.x,
            img.y,
            img.width,
            img.height,
            &format!("[Image: {}]", img.alt_text),
            sp_id,
            xml,
        );
        return;
    };
    let x = (img.x * 9525.0) as i64;
    let y = (img.y * 9525.0) as i64;
    let width = (img.width.max(1.0) * 9525.0) as i64;
    let height = (img.height.max(1.0) * 9525.0) as i64;
    let relationship = if media.external_url.is_some() {
        format!("r:link=\"{}\"", media.relationship_id)
    } else {
        format!("r:embed=\"{}\"", media.relationship_id)
    };
    xml.push_str(&format!(
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{sp_id}\" name=\"Image {sp_id}\" descr=\"{}\"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip {relationship}/>",
        escape_xml(&img.alt_text)
    ));
    if let Some(crop) = &img.crop {
        xml.push_str(&format!(
            "<a:srcRect t=\"{}\" r=\"{}\" b=\"{}\" l=\"{}\"/>",
            (crop.top.clamp(0.0, 1.0) * 100_000.0) as u32,
            (crop.right.clamp(0.0, 1.0) * 100_000.0) as u32,
            (crop.bottom.clamp(0.0, 1.0) * 100_000.0) as u32,
            (crop.left.clamp(0.0, 1.0) * 100_000.0) as u32,
        ));
    }
    xml.push_str(&format!(
        "<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm rot=\"{}\"><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{width}\" cy=\"{height}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
        (img.rotation * 60_000.0) as i64
    ));
}

fn chart_to_xml(
    chart: &SlideChart,
    resource: Option<&(String, usize)>,
    sp_id: u32,
    xml: &mut String,
) {
    let Some((relationship_id, _)) = resource else {
        placeholder_box_to_xml(
            chart.x,
            chart.y,
            chart.width,
            chart.height,
            "[Chart]",
            sp_id,
            xml,
        );
        return;
    };
    let x = (chart.x * 9525.0) as i64;
    let y = (chart.y * 9525.0) as i64;
    let width = (chart.width.max(1.0) * 9525.0) as i64;
    let height = (chart.height.max(1.0) * 9525.0) as i64;
    xml.push_str(&format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{sp_id}\" name=\"Chart {sp_id}\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{width}\" cy=\"{height}\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" r:id=\"{relationship_id}\"/></a:graphicData></a:graphic></p:graphicFrame>"
    ));
}

fn video_to_xml(video: &SlideVideo, media: Option<&MediaPart>, sp_id: u32, xml: &mut String) {
    let Some(media) = media.filter(|media| !media.bytes.is_empty() || media.external_url.is_some())
    else {
        placeholder_box_to_xml(
            video.x,
            video.y,
            video.width,
            video.height,
            "[Video]",
            sp_id,
            xml,
        );
        return;
    };
    let x = (video.x * 9525.0) as i64;
    let y = (video.y * 9525.0) as i64;
    let width = (video.width.max(1.0) * 9525.0) as i64;
    let height = (video.height.max(1.0) * 9525.0) as i64;
    xml.push_str(&format!(
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{sp_id}\" name=\"Video {sp_id}\"/><p:cNvPicPr/><p:nvPr><a:videoFile r:link=\"{}\"/></p:nvPr></p:nvPicPr><p:blipFill><a:blip/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{width}\" cy=\"{height}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val=\"111827\"/></a:solidFill></p:spPr></p:pic>",
        media.relationship_id
    ));
}

fn group_to_xml(
    group: &ElementGroup,
    resources: &SlideResources,
    sp_id: u32,
    next_id: &mut u32,
    shape_ids: &mut HashMap<Uuid, u32>,
    xml: &mut String,
) {
    let (x, y, width, height) = group_bounds(&group.elements);
    let x = (x * 9525.0) as i64;
    let y = (y * 9525.0) as i64;
    let width = (width.max(1.0) * 9525.0) as i64;
    let height = (height.max(1.0) * 9525.0) as i64;
    xml.push_str(&format!(
        "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{sp_id}\" name=\"Group {sp_id}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{width}\" cy=\"{height}\"/><a:chOff x=\"{x}\" y=\"{y}\"/><a:chExt cx=\"{width}\" cy=\"{height}\"/></a:xfrm></p:grpSpPr>"
    ));
    for element in &group.elements {
        element_to_xml(element, resources, next_id, shape_ids, xml);
    }
    xml.push_str("</p:grpSp>");
}

fn element_bounds(element: &SlideElement) -> (f32, f32, f32, f32) {
    match element {
        SlideElement::TextBox(value) => (value.x, value.y, value.width, value.height),
        SlideElement::Image(value) => (value.x, value.y, value.width, value.height),
        SlideElement::Shape(value) => (value.x, value.y, value.width, value.height),
        SlideElement::Chart(value) => (value.x, value.y, value.width, value.height),
        SlideElement::Video(value) => (value.x, value.y, value.width, value.height),
        SlideElement::Group(value) => group_bounds(&value.elements),
    }
}

fn group_bounds(elements: &[SlideElement]) -> (f32, f32, f32, f32) {
    if elements.is_empty() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for element in elements {
        let (x, y, width, height) = element_bounds(element);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + width);
        max_y = max_y.max(y + height);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

fn background_to_xml(background: &contrix_doc_engine::slides::Background, xml: &mut String) {
    use contrix_doc_engine::slides::Background;
    match background {
        Background::Solid(color) => xml.push_str(&format!(
            "<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>",
            escape_xml(color.trim_start_matches('#'))
        )),
        Background::Gradient(start, end, angle) => xml.push_str(&format!(
            "<p:bg><p:bgPr><a:gradFill><a:gsLst><a:gs pos=\"0\"><a:srgbClr val=\"{}\"/></a:gs><a:gs pos=\"100000\"><a:srgbClr val=\"{}\"/></a:gs></a:gsLst><a:lin ang=\"{}\" scaled=\"1\"/></a:gradFill><a:effectLst/></p:bgPr></p:bg>",
            escape_xml(start.trim_start_matches('#')),
            escape_xml(end.trim_start_matches('#')),
            (*angle * 60_000.0) as i64
        )),
        Background::Image(_) => {}
    }
}

fn transition_to_xml(slide: &Slide, xml: &mut String) {
    use contrix_doc_engine::slides::TransitionType;
    let Some(transition) = &slide.transition else {
        return;
    };
    let effect = match transition.transition_type {
        TransitionType::None => return,
        TransitionType::Fade => "<p:fade/>",
        TransitionType::Slide | TransitionType::Push => "<p:push dir=\"l\"/>",
        TransitionType::Zoom => "<p:zoom dir=\"in\"/>",
        TransitionType::Flip => "<p:flip dir=\"l\"/>",
        TransitionType::Dissolve => "<p:dissolve/>",
    };
    xml.push_str(&format!(
        "<p:transition advTm=\"{}\">{effect}</p:transition>",
        transition.duration_ms
    ));
}

fn animations_to_xml(slide: &Slide, shape_ids: &HashMap<Uuid, u32>, xml: &mut String) {
    if slide.animations.is_empty() {
        return;
    }
    let mut animations = slide.animations.clone();
    animations.sort_by_key(|animation| animation.order);
    xml.push_str("<p:timing><p:tnLst><p:seq concurrent=\"1\" nextAc=\"seek\"><p:cTn id=\"1\" dur=\"indefinite\" nodeType=\"tmRoot\"><p:childTnLst>");
    for (index, animation) in animations.iter().enumerate() {
        let Some(shape_id) = shape_ids.get(&animation.target_id) else {
            continue;
        };
        let filter = match animation.effect {
            contrix_doc_engine::slides::AnimationEffect::Appear => "appear",
            contrix_doc_engine::slides::AnimationEffect::Fade => "fade",
            contrix_doc_engine::slides::AnimationEffect::FlyIn => "fly",
            contrix_doc_engine::slides::AnimationEffect::Zoom => "zoom",
            contrix_doc_engine::slides::AnimationEffect::Pulse => "pulse",
            contrix_doc_engine::slides::AnimationEffect::Disappear => "disappear",
        };
        xml.push_str(&format!(
            "<p:animEffect transition=\"in\" filter=\"{filter}\"><p:cBhvr><p:cTn id=\"{}\" dur=\"{}\" delay=\"{}\"/><p:tgtEl><p:spTgt spid=\"{shape_id}\"/></p:tgtEl></p:cBhvr></p:animEffect>",
            index + 2,
            animation.duration_ms.max(1),
            animation.delay_ms,
        ));
    }
    xml.push_str("</p:childTnLst></p:cTn></p:seq></p:tnLst></p:timing>");
}

/// Emit a simple rectangle text box used as a placeholder for elements that are
/// not yet fully serialised to OOXML, so their position and a label survive.
fn placeholder_box_to_xml(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    label: &str,
    sp_id: u32,
    xml: &mut String,
) {
    let x_emu = (x * 9525.0) as i64;
    let y_emu = (y * 9525.0) as i64;
    let w_emu = (width.max(1.0) * 9525.0) as i64;
    let h_emu = (height.max(1.0) * 9525.0) as i64;

    xml.push_str(&format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{sp_id}\" name=\"Placeholder {sp_id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>"
    ));
    xml.push_str(&format!(
        "<p:spPr><a:xfrm><a:off x=\"{x_emu}\" y=\"{y_emu}\"/><a:ext cx=\"{w_emu}\" cy=\"{h_emu}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>"
    ));
    xml.push_str("<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/>");
    xml.push_str(&format!("<a:t>{}</a:t>", escape_xml(label)));
    xml.push_str("</a:r></a:p></p:txBody></p:sp>");
}

fn write_chart_parts<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    elements: &[SlideElement],
    resources: &SlideResources,
) -> Result<(), String> {
    for element in elements {
        match element {
            SlideElement::Chart(chart) => {
                if let Some((_, chart_number)) = resources.charts.get(&chart.id) {
                    zip.start_file(format!("ppt/charts/chart{chart_number}.xml"), options)
                        .map_err(|error| error.to_string())?;
                    write!(zip, "{}", chart_part_xml(chart)).map_err(|error| error.to_string())?;
                }
            }
            SlideElement::Group(group) => {
                write_chart_parts(zip, options, &group.elements, resources)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn chart_part_xml(chart: &SlideChart) -> String {
    let (tag, prefix, suffix, axes) = match chart.chart_type {
        ChartType::Bar => (
            "barChart",
            "<c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>",
            "",
            true,
        ),
        ChartType::Line => (
            "lineChart",
            "<c:grouping val=\"standard\"/>",
            "<c:marker val=\"1\"/>",
            true,
        ),
        ChartType::Pie => ("pieChart", "<c:varyColors val=\"1\"/>", "", false),
        ChartType::Doughnut => (
            "doughnutChart",
            "<c:varyColors val=\"1\"/>",
            "<c:holeSize val=\"50\"/>",
            false,
        ),
        ChartType::Scatter => (
            "scatterChart",
            "<c:scatterStyle val=\"lineMarker\"/>",
            "",
            true,
        ),
        ChartType::Area => ("areaChart", "<c:grouping val=\"standard\"/>", "", true),
    };
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/><c:{tag}>{prefix}"#
    );
    for (series_index, dataset) in chart.data.datasets.iter().enumerate() {
        xml.push_str(&format!(
            "<c:ser><c:idx val=\"{series_index}\"/><c:order val=\"{series_index}\"/><c:tx><c:v>{}</c:v></c:tx>",
            escape_xml(&dataset.name)
        ));
        if let Some(color) = &dataset.color {
            xml.push_str(&format!(
                "<c:spPr><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></c:spPr>",
                escape_xml(color.trim_start_matches('#'))
            ));
        }
        if matches!(chart.chart_type, ChartType::Scatter) {
            xml.push_str("<c:xVal><c:numLit>");
            xml.push_str(&format!("<c:ptCount val=\"{}\"/>", dataset.values.len()));
            for index in 0..dataset.values.len() {
                xml.push_str(&format!(
                    "<c:pt idx=\"{index}\"><c:v>{}</c:v></c:pt>",
                    index + 1
                ));
            }
            xml.push_str("</c:numLit></c:xVal><c:yVal>");
        } else {
            xml.push_str("<c:cat><c:strLit>");
            xml.push_str(&format!("<c:ptCount val=\"{}\"/>", chart.data.labels.len()));
            for (index, label) in chart.data.labels.iter().enumerate() {
                xml.push_str(&format!(
                    "<c:pt idx=\"{index}\"><c:v>{}</c:v></c:pt>",
                    escape_xml(label)
                ));
            }
            xml.push_str("</c:strLit></c:cat><c:val>");
        }
        xml.push_str("<c:numLit><c:formatCode>General</c:formatCode>");
        xml.push_str(&format!("<c:ptCount val=\"{}\"/>", dataset.values.len()));
        for (index, value) in dataset.values.iter().enumerate() {
            xml.push_str(&format!("<c:pt idx=\"{index}\"><c:v>{value}</c:v></c:pt>"));
        }
        xml.push_str("</c:numLit>");
        xml.push_str(if matches!(chart.chart_type, ChartType::Scatter) {
            "</c:yVal>"
        } else {
            "</c:val>"
        });
        xml.push_str("</c:ser>");
    }
    if axes {
        xml.push_str("<c:axId val=\"123456\"/><c:axId val=\"654321\"/>");
    }
    xml.push_str(suffix);
    xml.push_str(&format!("</c:{tag}>"));
    if axes {
        xml.push_str("<c:catAx><c:axId val=\"123456\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"654321\"/><c:crosses val=\"autoZero\"/></c:catAx><c:valAx><c:axId val=\"654321\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"123456\"/><c:crosses val=\"autoZero\"/></c:valAx>");
    }
    xml.push_str("</c:plotArea><c:plotVisOnly val=\"1\"/></c:chart></c:chartSpace>");
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
            xml.push_str(&format!("<a:latin typeface=\"{}\"/>", escape_xml(font)));
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

    if let Some(model) = crate::native_package::read(bytes, "customXml/contrix-presentation.json")
        && let Ok(presentation) = serde_json::from_value::<Presentation>(model)
    {
        return Ok(presentation);
    }

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
        theme: Default::default(),
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
                    let decoded = t.decode().ok()?;
                    title.push_str(&quick_xml::escape::unescape(&decoded).ok()?);
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
                b"a:t" if in_txbody => {
                    in_t = true;
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
                    let decoded = t.decode().map_err(|e| e.to_string())?;
                    let s = quick_xml::escape::unescape(&decoded).map_err(|e| e.to_string())?;
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
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/theme/theme1.xml",
        ] {
            assert!(archive.by_name(name).is_ok(), "missing: {}", name);
        }
    }

    #[test]
    fn test_pptx_round_trip() {
        let mut pres = Presentation::new("Trip".into());
        // Replace default first slide content with our text.
        pres.slides[0].elements.clear();
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
    fn media_charts_groups_and_animation_are_real_ooxml_and_round_trip() {
        use contrix_doc_engine::slides::{
            Animation, AnimationEffect, AnimationTrigger, ChartData, Dataset, ElementGroup,
            SlideChart, SlideVideo,
        };
        let mut pres = Presentation::new("Rich".into());
        let slide = &mut pres.slides[0];
        slide.elements.clear();
        let image_id = slide.add_image(
            10.0,
            20.0,
            120.0,
            80.0,
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9ZpK0AAAAASUVORK5CYII=",
        );
        slide.elements.push(SlideElement::Chart(SlideChart {
            id: Uuid::new_v4(),
            x: 150.0,
            y: 20.0,
            width: 300.0,
            height: 180.0,
            chart_type: ChartType::Bar,
            data: ChartData {
                labels: vec!["Q1".into(), "Q2".into()],
                datasets: vec![Dataset {
                    name: "Revenue".into(),
                    values: vec![10.0, 12.0],
                    color: Some("#2563EB".into()),
                }],
            },
        }));
        slide.elements.push(SlideElement::Group(ElementGroup {
            id: Uuid::new_v4(),
            elements: vec![SlideElement::Shape(Shape {
                id: Uuid::new_v4(),
                x: 20.0,
                y: 220.0,
                width: 100.0,
                height: 40.0,
                rotation: 0.0,
                shape_type: ShapeType::RoundedRectangle,
                fill_color: Some("#22C55E".into()),
                border: None,
                text: Some(vec![SlideTextRun::plain("Grouped")]),
            })],
        }));
        slide.elements.push(SlideElement::Video(SlideVideo {
            id: Uuid::new_v4(),
            x: 150.0,
            y: 220.0,
            width: 200.0,
            height: 120.0,
            url: "https://cdn.example.com/demo.mp4".into(),
            autoplay: false,
        }));
        slide.animations.push(Animation {
            target_id: image_id,
            effect: AnimationEffect::Fade,
            trigger: AnimationTrigger::OnClick,
            duration_ms: 500,
            delay_ms: 0,
            order: 1,
        });
        slide.notes = "Presenter note".into();

        let bytes = presentation_to_pptx(&pres).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        assert!(archive.by_name("ppt/media/image1.png").is_ok());
        assert!(archive.by_name("ppt/charts/chart1.xml").is_ok());
        assert!(archive.by_name("ppt/notesSlides/notesSlide1.xml").is_ok());
        let mut slide_xml = String::new();
        archive
            .by_name("ppt/slides/slide1.xml")
            .unwrap()
            .read_to_string(&mut slide_xml)
            .unwrap();
        assert!(slide_xml.contains("<p:pic>"));
        assert!(slide_xml.contains("<c:chart"));
        assert!(slide_xml.contains("<p:grpSp>"));
        assert!(slide_xml.contains("<p:timing>"));
        drop(archive);

        let imported = pptx_to_presentation(&bytes).unwrap();
        assert_eq!(imported.slides[0].elements.len(), 4);
        assert_eq!(imported.slides[0].notes, "Presenter note");
        assert_eq!(imported.slides[0].animations.len(), 1);
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<a&b>"), "&lt;a&amp;b&gt;");
    }
}
