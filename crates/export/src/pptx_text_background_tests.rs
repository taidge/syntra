use super::*;

#[test]
fn theme_background_resolves_style_index_placeholder_color_and_color_map() {
    let root = xml::parse(r#"<theme><clrScheme><dk1><srgbClr val="111111"/></dk1><lt1><srgbClr val="EEEEEE"/></lt1><accent1><srgbClr val="123456"/></accent1></clrScheme><fmtScheme><fillStyleLst><solidFill><srgbClr val="ABCDEF"/></solidFill></fillStyleLst><bgFillStyleLst><solidFill><schemeClr val="phClr"/></solidFill><gradFill><gsLst><gs pos="0"><schemeClr val="phClr"/></gs><gs pos="100000"><srgbClr val="FFFFFF"/></gs></gsLst><lin ang="5400000"/></gradFill></bgFillStyleLst></fmtScheme></theme>"#).unwrap();
    let mut theme = theme(Some(&root));
    let mapping = xml::parse(r#"<clrMap bg1="dk1" tx1="lt1"/>"#).unwrap();
    apply_color_map(&mut theme, Some(&mapping));
    assert_eq!(theme.colors["bg1"], "#111111");
    let background =
        xml::parse(r#"<bg><bgRef idx="1002"><schemeClr val="accent1"/></bgRef></bg>"#).unwrap();
    let (background, resolved) = theme_background(&background, &theme).unwrap();
    let gradient = background.child("gradFill").unwrap();
    let stops: Vec<_> = gradient
        .descendants("gs")
        .into_iter()
        .filter_map(|n| n.children.iter().find_map(|n| color(n, &resolved)))
        .collect();
    assert_eq!(stops, ["#123456", "#FFFFFF"]);
    assert_eq!(gradient.child("lin").unwrap().num("ang"), Some(5400000.));
    assert!(
        !theme.colors.contains_key("phClr"),
        "placeholder substitution must stay local to the reference"
    );
}

#[test]
fn text_body_preserves_wrapping_anchor_insets_and_paragraph_spacing() {
    let node = xml::parse(r#"<sp><txBody><bodyPr wrap="square" anchor="ctr" lIns="0" tIns="9525" rIns="19050" bIns="28575"/><p><pPr algn="r"><lnSpc><spcPct val="125000"/></lnSpc><spcAft><spcPts val="1200"/></spcAft><defRPr sz="2000"/></pPr><r><t>First paragraph</t></r></p><p><pPr algn="ctr"/><r><t>Second paragraph</t></r></p></txBody></sp>"#).unwrap();
    let layout = text_layout(&node, None).unwrap();
    assert!(layout.wrap);
    assert_eq!(layout.vertical_alignment, "center");
    assert_eq!(
        (
            layout.inset_left,
            layout.inset_top,
            layout.inset_right,
            layout.inset_bottom
        ),
        (0., 1., 2., 3.)
    );
    let runs = runs(
        &node,
        None,
        &Theme::default(),
        &BTreeMap::new(),
        &mut Vec::new(),
    );
    assert_eq!(
        runs.iter().map(|r| r.text.as_str()).collect::<String>(),
        "First paragraph\nSecond paragraph"
    );
    let paragraph = runs[0].paragraph.as_ref().unwrap();
    assert_eq!(paragraph.alignment, "right");
    assert_eq!(paragraph.line_height, Some(1.25));
    assert_eq!(paragraph.space_after_points, Some(12.));
    assert_eq!(runs[0].font_size, Some(20.));
    assert!(runs[1].paragraph_break);
    assert_eq!(runs[2].paragraph.as_ref().unwrap().alignment, "center");
}

#[test]
fn imported_reused_background_fixture_keeps_text_layout() {
    let presentation = crate::pptx::pptx_to_presentation(include_bytes!(
        "../tests/fixtures/reused-large-background.pptx"
    ))
    .unwrap();
    assert_eq!(presentation.slides.len(), 20);
    assert!(presentation.metadata.import_warnings.is_empty());
    let images = presentation
        .slides
        .iter()
        .map(|slide| {
            let images = slide
                .elements
                .iter()
                .filter_map(|element| match element {
                    SlideElement::Image(image) => Some(image.url.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(images.len(), 1, "every slide retains its reused background");
            images[0]
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(images.len(), 1);
    assert!(
        images
            .first()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    let texts: Vec<_> = presentation.slides[0]
        .elements
        .iter()
        .filter_map(|e| {
            if let SlideElement::TextBox(text) = e {
                Some(text)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert!(
        texts
            .iter()
            .all(|text| text.text_layout.as_ref().is_some_and(|layout| layout.wrap))
    );
    assert_eq!(
        texts[0].content[0].paragraph.as_ref().unwrap().alignment,
        "center"
    );
    let paragraph = texts[1].content[0].paragraph.as_ref().unwrap();
    assert_eq!(paragraph.alignment, "right");
    assert_eq!(paragraph.line_height, Some(1.25));
    assert_eq!(paragraph.space_after_points, Some(12.));
}
