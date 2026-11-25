use super::*;
use syntra_doc_engine::slides::{ChartAxis, ChartLabels, ChartPresentation};

pub(super) fn title_xml(title: Option<&str>, style: Option<&ChartPresentation>) -> String {
    let Some(title) = title.filter(|v| !v.is_empty()) else {
        return "<c:autoTitleDeleted val=\"1\"/>".into();
    };
    let font = escape_xml(
        style
            .and_then(|s| s.font_family.as_deref())
            .unwrap_or("Arial"),
    );
    let size = style.and_then(|s| s.title_font_size).unwrap_or(24.) * 75.;
    format!(
        "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"{}\" b=\"1\"><a:latin typeface=\"{font}\"/><a:ea typeface=\"{font}\"/></a:defRPr></a:pPr><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/>",
        size as i64,
        escape_xml(title)
    )
}
pub(super) fn labels_xml(labels: &ChartLabels) -> String {
    let position = if labels.position.is_empty() {
        String::new()
    } else {
        format!("<c:dLblPos val=\"{}\"/>", escape_xml(&labels.position))
    };
    let mut xml = format!(
        "<c:dLbls>{position}<c:showLegendKey val=\"0\"/><c:showVal val=\"{}\"/><c:showCatName val=\"{}\"/><c:showSerName val=\"{}\"/><c:showPercent val=\"{}\"/>",
        u8::from(labels.show_value),
        u8::from(labels.show_category),
        u8::from(labels.show_series),
        u8::from(labels.show_percent)
    );
    if let Some(format) = &labels.number_format {
        xml.push_str(&format!(
            "<c:numFmt formatCode=\"{}\" sourceLinked=\"0\"/>",
            escape_xml(format)
        ));
    }
    xml.push_str("</c:dLbls>");
    xml
}

pub(super) fn series_color(color: Option<&str>, index: usize) -> String {
    const PALETTE: [&str; 6] = [
        "#336699", "#D97706", "#16A34A", "#9333EA", "#0891B2", "#DC2626",
    ];
    color
        .filter(|s| {
            s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
        })
        .unwrap_or(PALETTE[index % PALETTE.len()])
        .to_string()
}
pub(super) fn axis_xml(
    tag: &str,
    id: u32,
    cross: u32,
    axis: Option<&ChartAxis>,
    style: Option<&ChartPresentation>,
) -> String {
    let default = ChartAxis::default();
    let axis = axis.unwrap_or(&default);
    let mut xml =
        format!("<c:{tag}><c:axId val=\"{id}\"/><c:scaling><c:orientation val=\"minMax\"/>");
    for (tag, value) in [("min", axis.minimum), ("max", axis.maximum)] {
        if let Some(v) = value.filter(|n| n.is_finite()) {
            xml.push_str(&format!("<c:{tag} val=\"{v}\"/>"));
        }
    }
    let horizontal = style.is_some_and(|s| s.horizontal);
    let position = if (tag == "catAx") != horizontal {
        "b"
    } else {
        "l"
    };
    xml.push_str(&format!(
        "</c:scaling><c:delete val=\"{}\"/><c:axPos val=\"{position}\"/>",
        u8::from(axis.hidden)
    ));
    if axis.major_gridlines {
        xml.push_str(&format!("<c:majorGridlines><c:spPr><a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>",(axis.grid_width.unwrap_or(1.)*9525.) as i64,escape_xml(axis.grid_color.as_deref().unwrap_or("#D3DAE2").trim_start_matches('#'))));
    }
    if axis.title.is_some() {
        xml.push_str(
            &title_xml(axis.title.as_deref(), style).replace("<c:autoTitleDeleted val=\"0\"/>", ""),
        );
    }
    if let Some(format) = &axis.number_format {
        xml.push_str(&format!(
            "<c:numFmt formatCode=\"{}\" sourceLinked=\"0\"/>",
            escape_xml(format)
        ));
    }
    if let Some(unit) = axis.major_unit.filter(|n| n.is_finite() && *n > 0.) {
        xml.push_str(&format!("<c:majorUnit val=\"{unit}\"/>"));
    }
    xml.push_str(&format!(
        "<c:crossAx val=\"{cross}\"/><c:crosses val=\"autoZero\"/></c:{tag}>"
    ));
    xml
}
pub(super) fn text_properties_xml(style: &ChartPresentation) -> String {
    let font = escape_xml(style.font_family.as_deref().unwrap_or("Arial"));
    let size = style.font_size.unwrap_or(12.) * 75.;
    format!(
        "<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"{}\"><a:latin typeface=\"{font}\"/><a:ea typeface=\"{font}\"/></a:defRPr></a:pPr><a:endParaRPr/></a:p></c:txPr>",
        size as i64
    )
}
