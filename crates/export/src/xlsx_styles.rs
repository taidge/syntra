//! OOXML style table. Whole formats are deduplicated independently of native caches.
use super::{XlsxExportStats, differential_format_xml, escape_xml};
use std::collections::BTreeMap;
use syntra_doc_engine::calc::{Border, BorderStyle, CellFormat, HAlign, VAlign, Workbook};

pub(super) struct Styles {
    formats: Vec<CellFormat>,
    indices: BTreeMap<String, usize>,
}
impl Styles {
    pub(super) fn new(workbook: &Workbook, stats: &mut XlsxExportStats) -> Self {
        let unique: BTreeMap<_, _> = workbook
            .sheets
            .iter()
            .flat_map(|s| s.cells.values())
            .map(|c| {
                stats.style_cell_visits += 1;
                (
                    serde_json::to_string(&c.format).expect("format serializes"),
                    c.format.clone(),
                )
            })
            .collect();
        let mut formats = Vec::new();
        let mut indices = BTreeMap::new();
        for (key, format) in unique {
            indices.insert(key, formats.len() + 1);
            formats.push(format);
        }
        Self { formats, indices }
    }
    pub(super) fn index(&self, format: &CellFormat) -> usize {
        self.indices
            .get(&serde_json::to_string(format).expect("format serializes"))
            .copied()
            .unwrap_or(0)
    }
    pub(super) fn xml(&self, workbook: &Workbook) -> String {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
        );
        let formats = &self.formats;
        let numbered: Vec<_> = formats
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.number_format.as_ref().map(|code| (i, code)))
            .collect();
        xml.push_str(&format!("<numFmts count=\"{}\">", numbered.len()));
        for (i, code) in &numbered {
            xml.push_str(&format!(
                "<numFmt numFmtId=\"{}\" formatCode=\"{}\"/>",
                164 + i,
                escape_xml(number_code(code))
            ));
        }
        xml.push_str("</numFmts>");
        xml.push_str(&format!(
            "<fonts count=\"{}\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>",
            formats.len() + 1
        ));
        for f in formats {
            xml.push_str("<font>");
            xml.push_str(&format!(
                "<sz val=\"{}\"/><name val=\"{}\"/>",
                f.font_size.filter(|n| n.is_finite()).unwrap_or(11.0),
                escape_xml(f.font_family.as_deref().unwrap_or("Calibri"))
            ));
            if f.bold {
                xml.push_str("<b/>");
            }
            if f.italic {
                xml.push_str("<i/>");
            }
            if f.underline {
                xml.push_str("<u/>");
            }
            if let Some(color) = &f.text_color {
                xml.push_str(&format!("<color rgb=\"{}\"/>", color_xml(color)));
            }
            xml.push_str("</font>");
        }
        xml.push_str(&format!("</fonts><fills count=\"{}\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill>",formats.len()+2));
        for f in formats {
            if let Some(color) = &f.bg_color {
                xml.push_str(&format!("<fill><patternFill patternType=\"solid\"><fgColor rgb=\"{}\"/><bgColor indexed=\"64\"/></patternFill></fill>",color_xml(color)));
            } else {
                xml.push_str("<fill><patternFill patternType=\"none\"/></fill>");
            }
        }
        xml.push_str(&format!("</fills><borders count=\"{}\"><border><left/><right/><top/><bottom/><diagonal/></border>",formats.len()+1));
        for f in formats {
            xml.push_str("<border>");
            for (name, border) in [
                ("left", &f.borders.left),
                ("right", &f.borders.right),
                ("top", &f.borders.top),
                ("bottom", &f.borders.bottom),
            ] {
                xml.push_str(&border_xml(name, border.as_ref()));
            }
            xml.push_str("<diagonal/></border>");
        }
        xml.push_str(&format!("</borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"{}\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>",formats.len()+1));
        for (i, f) in formats.iter().enumerate() {
            let horizontal = match f.horizontal_align {
                HAlign::Left => "left",
                HAlign::Center => "center",
                HAlign::Right => "right",
            };
            let vertical = match f.vertical_align {
                VAlign::Top => "top",
                VAlign::Middle => "center",
                VAlign::Bottom => "bottom",
            };
            let num_fmt = if f.number_format.is_some() {
                164 + i
            } else {
                0
            };
            xml.push_str(&format!("<xf numFmtId=\"{num_fmt}\" fontId=\"{}\" fillId=\"{}\" borderId=\"{}\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyNumberFormat=\"1\" applyAlignment=\"1\"><alignment horizontal=\"{horizontal}\" vertical=\"{vertical}\" wrapText=\"{}\"/></xf>",i+1,i+2,i+1,if f.wrap_text {1}else{0}));
        }
        let differentials: Vec<_> = workbook
            .sheets
            .iter()
            .flat_map(|s| &s.conditional_formats)
            .map(|f| differential_format_xml(&f.format))
            .collect();
        xml.push_str(&format!("</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles><dxfs count=\"{}\">{}</dxfs></styleSheet>",differentials.len(),differentials.join("")));
        xml
    }
}
fn number_code(code: &str) -> &str {
    match code {
        "general" => "General",
        "number" | "decimal2" => "0.00",
        "decimal0" => "0",
        "currency" => "$#,##0.00",
        "percentage" | "percent" => "0.00%",
        "date" => "yyyy-mm-dd",
        other => other,
    }
}
fn color_xml(color: &str) -> String {
    let color = color.trim_start_matches('#');
    escape_xml(&if color.len() == 6 {
        format!("FF{color}")
    } else {
        color.into()
    })
}
fn border_xml(name: &str, border: Option<&Border>) -> String {
    let Some(border) = border else {
        return format!("<{name}/>");
    };
    let style = match border.style {
        BorderStyle::Thin => "thin",
        BorderStyle::Medium => "medium",
        BorderStyle::Thick => "thick",
        BorderStyle::Dashed => "dashed",
        BorderStyle::Dotted => "dotted",
        BorderStyle::Double => "double",
    };
    format!(
        "<{name} style=\"{style}\"><color rgb=\"{}\"/></{name}>",
        color_xml(&border.color)
    )
}
