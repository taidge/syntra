//! Read standard SpreadsheetML features independently of syntra native caches.
use crate::ooxml_import::{self as xml, Archive, Node, warn};
use std::collections::{BTreeMap, BTreeSet};
use syntra_doc_engine::calc::*;
#[derive(Default)]
struct Styles {
    formats: Vec<CellFormat>,
    digit_width: Option<f64>,
    differential: Vec<CellFormat>,
    colors: Vec<String>,
    date_formats: BTreeSet<usize>,
}
fn rgb(node: &Node, colors: &[String]) -> Option<String> {
    let color = if let Some(color) = node.attr("rgb") {
        if !color.is_ascii()
            || !color.bytes().all(|c| c.is_ascii_hexdigit())
            || !matches!(color.len(), 6 | 8)
        {
            return None;
        }
        format!("#{}", if color.len() == 8 { &color[2..] } else { color })
    } else if let Some(index) = node.attr("theme").and_then(|s| s.parse::<usize>().ok()) {
        colors.get(index)?.clone()
    } else {
        let index = node.attr("indexed").and_then(|s| s.parse::<usize>().ok())?;
        let basic = [
            "#000000", "#FFFFFF", "#FF0000", "#00FF00", "#0000FF", "#FFFF00", "#FF00FF", "#00FFFF",
            "#000000", "#FFFFFF", "#FF0000", "#00FF00", "#0000FF", "#FFFF00", "#FF00FF", "#00FFFF",
            "#800000", "#008000", "#000080", "#808000", "#800080", "#008080", "#C0C0C0", "#808080",
            "#9999FF", "#993366", "#FFFFCC", "#CCFFFF", "#660066", "#FF8080", "#0066CC", "#CCCCFF",
            "#000080", "#FF00FF", "#FFFF00", "#00FFFF", "#800080", "#800000", "#008080", "#0000FF",
            "#00CCFF", "#CCFFFF", "#CCFFCC", "#FFFF99", "#99CCFF", "#FF99CC", "#CC99FF", "#FFCC99",
            "#3366FF", "#33CCCC", "#99CC00", "#FFCC00", "#FF9900", "#FF6600", "#666699", "#969696",
            "#003366", "#339966", "#003300", "#333300", "#993300", "#993366", "#333399", "#333333",
        ];
        basic.get(index).copied().unwrap_or("#000000").into()
    };
    if let Some(tint) = node.num("tint") {
        let text = color.trim_start_matches('#');
        if text.len() == 6 && text.is_ascii() {
            let mut values = Vec::new();
            for i in [0, 2, 4] {
                let value = u8::from_str_radix(&text[i..i + 2], 16).ok()? as f64;
                values.push(
                    if tint >= 0.0 {
                        value + (255.0 - value) * tint
                    } else {
                        value * (1.0 + tint)
                    }
                    .clamp(0.0, 255.0) as u8,
                );
            }
            return Some(format!(
                "#{:02X}{:02X}{:02X}",
                values[0], values[1], values[2]
            ));
        }
    }
    Some(color)
}
fn checked_metric(node: &Node, key: &str, min: f64, max: f64) -> Result<Option<f64>, String> {
    let Some(raw) = node.attr(key) else {
        return Ok(None);
    };
    let value = raw
        .parse::<f64>()
        .map_err(|_| format!("Invalid worksheet metric {}.{key}", node.name))?;
    if !value.is_finite() || value < min || value > max {
        return Err(format!(
            "Worksheet metric {}.{key} exceeds the finite layout budget",
            node.name
        ));
    }
    Ok(Some(value))
}
fn font(format: &mut CellFormat, node: &Node, colors: &[String]) {
    format.bold = node
        .child("b")
        .is_some_and(|n| n.flag("val") != Some(false));
    format.italic = node
        .child("i")
        .is_some_and(|n| n.flag("val") != Some(false));
    format.underline = node
        .child("u")
        .is_some_and(|n| n.attr("val") != Some("none"));
    format.font_family = node
        .child("name")
        .and_then(|n| n.attr("val"))
        .map(str::to_owned);
    format.font_size = node
        .child("sz")
        .and_then(|n| n.num("val"))
        .map(|n| n as f32);
    format.text_color = node.child("color").and_then(|n| rgb(n, colors));
}
fn fill(format: &mut CellFormat, node: &Node, colors: &[String], warnings: &mut Vec<String>) {
    if let Some(pattern) = node.child("patternFill") {
        if pattern
            .attr("patternType")
            .is_some_and(|p| p != "none" && p != "solid")
        {
            warn(
                warnings,
                "Pattern fills are approximated as a solid foreground color; the original retains the pattern.",
            );
        }
        if pattern.attr("patternType") != Some("none") {
            format.bg_color = pattern.child("fgColor").and_then(|n| rgb(n, colors));
        }
    }
    if let Some(gradient) = node.child("gradientFill") {
        format.bg_color = gradient.desc("color").and_then(|n| rgb(n, colors));
        warn(
            warnings,
            "Gradient cell fills are approximated using the first color; original fill stops are retained in the upload.",
        );
    }
}
fn borders(format: &mut CellFormat, node: &Node, colors: &[String], warnings: &mut Vec<String>) {
    let edge = |name: &str| {
        node.child(name).and_then(|n| {
            let style = match n.attr("style")? {
                "thin" | "hair" => BorderStyle::Thin,
                "medium" => BorderStyle::Medium,
                "thick" => BorderStyle::Thick,
                "dashed" | "mediumDashed" | "dashDot" | "dashDotDot" => BorderStyle::Dashed,
                "dotted" => BorderStyle::Dotted,
                "double" => BorderStyle::Double,
                _ => return None,
            };
            Some(Border {
                style,
                color: n
                    .child("color")
                    .and_then(|n| rgb(n, colors))
                    .unwrap_or_else(|| "#000000".into()),
            })
        })
    };
    format.borders = CellBorders {
        left: edge("left"),
        right: edge("right"),
        top: edge("top"),
        bottom: edge("bottom"),
    };
    if node.flag("diagonalUp") == Some(true) || node.flag("diagonalDown") == Some(true) {
        warn(
            warnings,
            "Diagonal cell borders are not represented; the original retains them.",
        );
    }
}
fn alignment(format: &mut CellFormat, node: &Node, warnings: &mut Vec<String>) {
    format.horizontal_align = match node.attr("horizontal").unwrap_or("left") {
        "center" | "centerContinuous" => HAlign::Center,
        "right" => HAlign::Right,
        "left" | "general" => HAlign::Left,
        _ => {
            warn(
                warnings,
                "Distributed/justified cell alignment is approximated; the original retains exact alignment.",
            );
            HAlign::Left
        }
    };
    format.vertical_align = match node.attr("vertical").unwrap_or("bottom") {
        "top" => VAlign::Top,
        "center" => VAlign::Middle,
        _ => VAlign::Bottom,
    };
    format.wrap_text = node.flag("wrapText").unwrap_or(false);
    if node.num("textRotation").unwrap_or(0.0) != 0.0 || node.num("indent").unwrap_or(0.0) != 0.0 {
        warn(
            warnings,
            "Cell text rotation/indent is not represented by the current format model; original retained.",
        );
    }
}
fn builtin(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"$\"#,##0_);(\"$\"#,##0)",
        6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
        7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
        8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0_);(#,##0)",
        38 => "#,##0_);[Red](#,##0)",
        39 => "#,##0.00_);(#,##0.00)",
        40 => "#,##0.00_);[Red](#,##0.00)",
        41 => "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)",
        42 => "_(\"$\"* #,##0_);_(\"$\"* \\(#,##0\\);_(\"$\"* \"-\"_);_(@_)",
        43 => "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)",
        44 => "_(\"$\"* #,##0.00_)_(\"$\"* \\(#,##0.00\\)_(\"$\"* \"-\"??_)_(@_)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}
fn date_format(code: &str) -> bool {
    let mut visible = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut bracket = false;
    for c in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' || c == '_' || c == '*' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if c == '[' {
            bracket = true;
            continue;
        }
        if c == ']' {
            bracket = false;
            continue;
        }
        if !bracket {
            visible.push(c.to_ascii_lowercase());
        }
    }
    visible.chars().any(|c| matches!(c, 'y' | 'd' | 'h' | 's'))
}
fn styles(
    archive: &mut Archive<'_>,
    path: Option<&str>,
    theme_path: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Styles, String> {
    let mut result = Styles::default();
    if let Some(path) = theme_path
        && let Some(root) = xml::optional(archive, path)?
        && let Some(scheme) = root.desc("clrScheme")
    {
        for name in [
            "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
            "accent6", "hlink", "folHlink",
        ] {
            result.colors.push(
                scheme
                    .child(name)
                    .and_then(|n| n.children.first())
                    .and_then(|n| n.attr("lastClr").or_else(|| n.attr("val")))
                    .map(|c| format!("#{c}"))
                    .unwrap_or_else(|| "#000000".into()),
            );
        }
    }
    let Some(path) = path else {
        return Ok(result);
    };
    let Some(root) = xml::optional(archive, path)? else {
        return Ok(result);
    };
    for size in root.descendants("sz") {
        checked_metric(size, "val", 1.0, 10000.0)?;
    }
    let numbers: BTreeMap<u32, String> = root
        .descendants("numFmt")
        .iter()
        .filter_map(|n| {
            Some((
                n.attr("numFmtId")?.parse().ok()?,
                n.attr("formatCode")?.into(),
            ))
        })
        .collect();
    let fonts = root
        .child("fonts")
        .map(|n| n.children.as_slice())
        .unwrap_or(&[]);
    // SpreadsheetML column widths use the largest digit of the Normal style.
    // SimSun's Latin digits occupy half an em, rounded to whole device pixels.
    let normal_xf = root
        .child("cellStyles")
        .and_then(|styles| {
            styles
                .children
                .iter()
                .find(|n| n.attr("builtinId") == Some("0"))
        })
        .and_then(|n| n.attr("xfId")?.parse::<usize>().ok())
        .unwrap_or(0);
    let normal_font = root
        .child("cellStyleXfs")
        .and_then(|xfs| xfs.children.get(normal_xf))
        .and_then(|xf| xf.attr("fontId")?.parse::<usize>().ok())
        .unwrap_or(0);
    if let Some(font) = fonts.get(normal_font)
        && font
            .child("name")
            .and_then(|n| n.attr("val"))
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("SimSun")
                    || name.eq_ignore_ascii_case("NSimSun")
                    || matches!(name, "宋体" | "新宋体")
            })
        && let Some(points) = font.child("sz").and_then(|n| n.num("val"))
    {
        result.digit_width = Some(((points * 96. / 72.).round() / 2.).round().max(1.));
    }
    let fills = root
        .child("fills")
        .map(|n| n.children.as_slice())
        .unwrap_or(&[]);
    let edges = root
        .child("borders")
        .map(|n| n.children.as_slice())
        .unwrap_or(&[]);
    let bases = root
        .child("cellStyleXfs")
        .map(|n| n.children.as_slice())
        .unwrap_or(&[]);
    if let Some(formats) = root.child("cellXfs") {
        for (index, xf) in formats.children.iter().enumerate() {
            let base = xf
                .attr("xfId")
                .and_then(|s| s.parse::<usize>().ok())
                .and_then(|i| bases.get(i));
            let integer = |key: &str| {
                xf.attr(key)
                    .or_else(|| base.and_then(|n| n.attr(key)))
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0)
            };
            let mut format = CellFormat::default();
            if let Some(node) = fonts.get(integer("fontId")) {
                font(&mut format, node, &result.colors);
            }
            if let Some(node) = fills.get(integer("fillId")) {
                fill(&mut format, node, &result.colors, warnings);
            }
            if let Some(node) = edges.get(integer("borderId")) {
                borders(&mut format, node, &result.colors, warnings);
            }
            if let Some(node) = xf
                .child("alignment")
                .or_else(|| base.and_then(|n| n.child("alignment")))
            {
                alignment(&mut format, node, warnings);
            }
            let id = integer("numFmtId") as u32;
            format.localized_short_date = id == 14 && !numbers.contains_key(&id);
            format.number_format = numbers
                .get(&id)
                .cloned()
                .or_else(|| builtin(id).map(str::to_owned));
            if id > 0 && format.number_format.is_none() {
                warn(
                    warnings,
                    format!(
                        "Number format {id} is locale-specific or missing its definition; original numeric format remains in the upload."
                    ),
                );
            }
            let elapsed = format.number_format.as_ref().is_some_and(|code| {
                let code = code.to_ascii_lowercase();
                code.contains("[h") || code.contains("[m") || code.contains("[s")
            });
            if elapsed {
                warn(
                    warnings,
                    "Elapsed-time formats are retained as numeric serials; the current Calc display may differ from Office duration formatting.",
                );
            } else if matches!(id, 14..=22 | 45 | 47)
                || format
                    .number_format
                    .as_ref()
                    .is_some_and(|code| date_format(code))
            {
                result.date_formats.insert(index);
            }
            if xf.child("protection").is_some() {
                warn(
                    warnings,
                    "Cell protection flags are not represented; the uploaded original retains workbook protection semantics.",
                );
            }
            result.formats.push(format);
        }
    }
    if let Some(dxfs) = root.child("dxfs") {
        for dxf in &dxfs.children {
            let mut format = CellFormat::default();
            if let Some(node) = dxf.child("font") {
                font(&mut format, node, &result.colors);
            }
            if let Some(node) = dxf.child("fill") {
                fill(&mut format, node, &result.colors, warnings);
            }
            if let Some(node) = dxf.child("border") {
                borders(&mut format, node, &result.colors, warnings);
            }
            if let Some(node) = dxf.child("alignment") {
                alignment(&mut format, node, warnings);
            }
            result.differential.push(format);
        }
    }
    Ok(result)
}
fn range(reference: &str) -> Option<(CellRef, CellRef)> {
    let mut pieces = reference.trim().split(':');
    let start = CellRef::from_a1(&pieces.next()?.replace('$', ""))?;
    let end = pieces
        .next()
        .map(|s| CellRef::from_a1(&s.replace('$', "")))
        .unwrap_or_else(|| Some(start.clone()))?;
    if pieces.next().is_some()
        || start.row > 1_048_575
        || end.row > 1_048_575
        || start.col > 16_383
        || end.col > 16_383
    {
        return None;
    }
    Some((start, end))
}
fn qualified(reference: &str) -> Option<(String, CellRef, CellRef)> {
    let (sheet, reference) = reference.trim().rsplit_once('!')?;
    let sheet = sheet.trim_matches('\'').replace("''", "'");
    if sheet.contains(['[', ']']) {
        return None;
    }
    let (start, end) = range(reference)?;
    Some((sheet, start, end))
}
fn date(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() || serial.abs() > 3_000_000.0 {
        return None;
    }
    let base = chrono::NaiveDate::from_ymd_opt(
        if date1904 { 1904 } else { 1899 },
        if date1904 { 1 } else { 12 },
        if date1904 {
            1
        } else if serial < 60.0 {
            31
        } else {
            30
        },
    )?;
    let value = base
        .and_hms_opt(0, 0, 0)?
        .checked_add_signed(chrono::Duration::seconds((serial * 86400.0).round() as i64))?;
    Some(if serial.fract().abs() < 1e-9 {
        value.date().to_string()
    } else {
        value.format("%Y-%m-%dT%H:%M:%S").to_string()
    })
}
fn validation(
    node: &Node,
    first: &str,
    second: Option<&str>,
    date1904: bool,
) -> Option<ValidationRule> {
    let between = node.attr("operator").unwrap_or("between") == "between";
    Some(match node.attr("type").unwrap_or("none") {
        "whole" if between => {
            ValidationRule::WholeNumberBetween(first.parse().ok()?, second?.parse().ok()?)
        }
        "decimal" if between => {
            ValidationRule::DecimalBetween(first.parse().ok()?, second?.parse().ok()?)
        }
        "textLength" if between => {
            ValidationRule::TextLengthBetween(first.parse().ok()?, second?.parse().ok()?)
        }
        "date" if between => ValidationRule::DateBetween(
            date(first.parse().ok()?, date1904)?,
            date(second?.parse().ok()?, date1904)?,
        ),
        "list" if first.starts_with('"') && first.ends_with('"') => ValidationRule::List(
            first[1..first.len() - 1]
                .split(',')
                .map(str::to_owned)
                .collect(),
        ),
        "custom" => ValidationRule::CustomFormula(first.trim_start_matches('=').into()),
        _ => return None,
    })
}
fn annotations(
    sheet: &mut Sheet,
    root: &Node,
    styles: &Styles,
    date1904: bool,
    warnings: &mut Vec<String>,
) {
    for node in root.descendants("dataValidation") {
        let first = node
            .child("formula1")
            .map(Node::all_text)
            .unwrap_or_default();
        let second = node.child("formula2").map(Node::all_text);
        let Some(rule) = validation(
            node,
            first.trim(),
            second.as_deref().map(str::trim),
            date1904,
        ) else {
            warn(
                warnings,
                "A validation rule uses a range/list binding or operator outside the current model; original validation is retained in the upload.",
            );
            continue;
        };
        if matches!(rule, ValidationRule::CustomFormula(_)) {
            warn(
                warnings,
                "Custom validation formulas are preserved for Office export; this editor does not enforce those formulas.",
            );
        }
        for reference in node.attr("sqref").unwrap_or_default().split_whitespace() {
            if let Some((range_start, range_end)) = range(reference) {
                sheet.data_validations.push(DataValidation {
                    range_start,
                    range_end,
                    rule: rule.clone(),
                    input_message: node.attr("prompt").map(str::to_owned),
                    error_message: node.attr("error").map(str::to_owned),
                });
            } else {
                warn(
                    warnings,
                    "A validation range could not be represented; original retained.",
                );
            }
        }
    }
    for conditional in root.descendants("conditionalFormatting") {
        for node in conditional.children.iter().filter(|n| n.name == "cfRule") {
            let formulas: Vec<_> = node
                .children
                .iter()
                .filter(|n| n.name == "formula")
                .map(|n| n.all_text().trim().to_owned())
                .collect();
            let rule = match (node.attr("type"), node.attr("operator"), formulas.first()) {
                (Some("cellIs"), Some("greaterThan"), Some(value)) => {
                    value.parse().ok().map(ConditionalRule::GreaterThan)
                }
                (Some("cellIs"), Some("lessThan"), Some(value)) => {
                    value.parse().ok().map(ConditionalRule::LessThan)
                }
                (Some("cellIs"), Some("between"), Some(value)) => value
                    .parse()
                    .ok()
                    .zip(formulas.get(1).and_then(|s| s.parse().ok()))
                    .map(|(a, b)| ConditionalRule::Between(a, b)),
                (Some("cellIs"), Some("equal"), Some(value)) => {
                    Some(ConditionalRule::EqualTo(value.trim_matches('"').into()))
                }
                (Some("containsText"), _, _) => node
                    .attr("text")
                    .map(|s| ConditionalRule::ContainsText(s.into())),
                (Some("containsBlanks"), _, _) => Some(ConditionalRule::IsEmpty),
                (Some("notContainsBlanks"), _, _) => Some(ConditionalRule::IsNotEmpty),
                _ => None,
            };
            let Some(rule) = rule else {
                warn(
                    warnings,
                    "Formula/color-scale/data-bar/icon conditional formatting cannot be represented by the current model; original retained.",
                );
                continue;
            };
            let format = node
                .attr("dxfId")
                .and_then(|s| s.parse::<usize>().ok())
                .and_then(|i| styles.differential.get(i))
                .cloned()
                .unwrap_or_default();
            for reference in conditional
                .attr("sqref")
                .unwrap_or_default()
                .split_whitespace()
            {
                if let Some((range_start, range_end)) = range(reference) {
                    sheet.conditional_formats.push(ConditionalFormat {
                        range_start,
                        range_end,
                        rule: rule.clone(),
                        format: format.clone(),
                    });
                }
            }
        }
    }
}
fn chart(
    archive: &mut Archive<'_>,
    part: &str,
    sheet: &Sheet,
    anchor: &Node,
    warnings: &mut Vec<String>,
    points_remaining: &mut usize,
) -> Option<SheetChart> {
    let root = xml::part(archive, part).ok()?;
    let (kind, data) = crate::pptx::external_import::chart_data(&root, warnings, points_remaining)?;
    let kind = match kind {
        syntra_doc_engine::slides::ChartType::Bar => SheetChartType::Bar,
        syntra_doc_engine::slides::ChartType::Line => SheetChartType::Line,
        syntra_doc_engine::slides::ChartType::Pie => SheetChartType::Pie,
        syntra_doc_engine::slides::ChartType::Area => SheetChartType::Area,
        syntra_doc_engine::slides::ChartType::Doughnut => SheetChartType::Doughnut,
        syntra_doc_engine::slides::ChartType::Scatter => SheetChartType::Scatter,
    };
    let point = |name: &str, default: (u32, u32)| {
        let node = anchor.child(name);
        CellRef::new(
            node.and_then(|n| n.child("row"))
                .and_then(|n| n.all_text().parse().ok())
                .unwrap_or(default.0),
            node.and_then(|n| n.child("col"))
                .and_then(|n| n.all_text().parse().ok())
                .unwrap_or(default.1),
        )
    };
    let anchor_start = point("from", (0, 0));
    let anchor_end = point(
        "to",
        (
            anchor_start.row.saturating_add(15),
            anchor_start.col.saturating_add(8),
        ),
    );
    if anchor.name != "twoCellAnchor" {
        warn(
            warnings,
            "One-cell/absolute chart anchors are approximated as cell-grid rectangles; original pixel offsets retained in the upload.",
        );
    }
    let mut sources = Vec::new();
    let mut first = None;
    let mut last = None;
    for series in root.descendants("ser") {
        let value = series
            .child("val")
            .or_else(|| series.child("yVal"))
            .and_then(|n| n.desc("f"))
            .and_then(|n| qualified(n.all_text().trim()));
        let labels = series
            .child("cat")
            .or_else(|| series.child("xVal"))
            .and_then(|n| n.desc("f"))
            .and_then(|n| qualified(n.all_text().trim()));
        if let Some((_, start, end)) = value.filter(|(name, _, _)| name == &sheet.name) {
            first.get_or_insert(start.clone());
            last = Some(end.clone());
            let (label_start, label_end) = labels
                .filter(|(name, _, _)| name == &sheet.name)
                .map(|(_, a, b)| (Some(a), Some(b)))
                .unwrap_or((None, None));
            sources.push(SheetChartSource {
                value_start: start,
                value_end: end,
                label_start,
                label_end,
            });
        } else {
            warn(
                warnings,
                "Chart literal/external/cross-sheet data is preserved as cached series; automatic binding refresh is limited to same-sheet ranges.",
            );
        }
    }
    if sources.len() != data.datasets.len() {
        sources.clear();
    }
    let title = root
        .desc("title")
        .map(|n| xml::text(n, "t"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Imported chart".into());
    Some(SheetChart {
        id: uuid::Uuid::new_v4(),
        title,
        chart_type: kind,
        data_start: first.unwrap_or_else(|| CellRef::new(0, 0)),
        data_end: last.unwrap_or_else(|| CellRef::new(0, 0)),
        anchor_start,
        anchor_end,
        cached_data: Some(data),
        series_sources: sources,
    })
}
fn drawings(
    archive: &mut Archive<'_>,
    sheet: &mut Sheet,
    root: &Node,
    rels: &BTreeMap<String, xml::Relationship>,
    warnings: &mut Vec<String>,
    points_remaining: &mut usize,
) -> Result<(), String> {
    for drawing in root.children.iter().filter(|n| n.name == "drawing") {
        let Some(relation) = drawing
            .attr("r:id")
            .and_then(|id| rels.get(id))
            .filter(|r| !r.external)
        else {
            continue;
        };
        let data = xml::part(archive, &relation.target)?;
        let links = xml::relationships(archive, &relation.target)?;
        for anchor in &data.children {
            if let Some(part) = anchor
                .desc("chart")
                .and_then(|n| n.attr("r:id"))
                .and_then(|id| links.get(id))
                .filter(|r| !r.external)
                && let Some(chart) = chart(
                    archive,
                    &part.target,
                    sheet,
                    anchor,
                    warnings,
                    points_remaining,
                )
            {
                sheet.charts.push(chart);
                warn(
                    warnings,
                    "Worksheet charts and representable bindings are preserved for Office export; the current Calc grid does not render a chart overlay.",
                );
            }
            if !anchor.descendants("pic").is_empty() || !anchor.descendants("sp").is_empty() {
                warn(
                    warnings,
                    "Worksheet drawing images/shapes are outside the current Calc model; original drawing objects remain downloadable.",
                );
            }
        }
    }
    Ok(())
}
fn pivots(
    archive: &mut Archive<'_>,
    workbook: &Node,
    rels: &BTreeMap<String, xml::Relationship>,
    sheets: &[(String, Node, BTreeMap<String, xml::Relationship>)],
    result: &mut Workbook,
) -> Result<(), String> {
    let mut caches = BTreeMap::new();
    for cache in workbook.descendants("pivotCache") {
        if let (Some(id), Some(link)) = (
            cache.attr("cacheId"),
            cache
                .attr("r:id")
                .and_then(|id| rels.get(id))
                .filter(|r| !r.external),
        ) {
            caches.insert(id.to_owned(), xml::part(archive, &link.target)?);
        }
    }
    for (si, (_, root, links)) in sheets.iter().enumerate() {
        for pivot in root.descendants("pivotTablePart") {
            let Some(link) = pivot
                .attr("r:id")
                .and_then(|id| links.get(id))
                .filter(|r| !r.external)
            else {
                continue;
            };
            let definition = xml::part(archive, &link.target)?;
            let Some(source) = definition
                .attr("cacheId")
                .and_then(|id| caches.get(id))
                .and_then(|n| n.desc("worksheetSource"))
            else {
                warn(
                    &mut result.metadata.import_warnings,
                    "External/OLAP pivot sources are not represented; cached worksheet values and the original file are retained.",
                );
                continue;
            };
            let rows = definition
                .child("rowFields")
                .map(|n| n.children.as_slice())
                .unwrap_or(&[]);
            let fields = definition
                .child("dataFields")
                .map(|n| n.children.as_slice())
                .unwrap_or(&[]);
            if definition.desc("pivotFilters").is_some()
                || definition
                    .descendants("item")
                    .iter()
                    .any(|n| n.flag("h") == Some(true))
            {
                warn(
                    &mut result.metadata.import_warnings,
                    "Filtered pivot items cannot be refreshed faithfully by the current pivot model; cached cells and the original definition are retained.",
                );
                continue;
            }
            if rows.len() != 1
                || fields.len() != 1
                || definition.child("colFields").is_some()
                || definition.child("pageFields").is_some()
            {
                warn(
                    &mut result.metadata.import_warnings,
                    "Multi-axis/multi-value/filtered pivots exceed the single-row-field pivot model; original pivot and cached cells retained.",
                );
                continue;
            }
            let source_sheet = source
                .attr("sheet")
                .and_then(|name| result.sheets.iter().position(|s| s.name == name));
            let rect = source.attr("ref").and_then(range);
            let Some((source_sheet, (start, end))) = source_sheet.zip(rect) else {
                warn(
                    &mut result.metadata.import_warnings,
                    "Named-table pivot sources are not yet resolved to a worksheet rectangle; original pivot retained.",
                );
                continue;
            };
            if start.row >= end.row {
                warn(
                    &mut result.metadata.import_warnings,
                    "An empty/header-only pivot source cannot be refreshed by the current model; cached cells and original retained.",
                );
                continue;
            }
            let Some((target, _)) = definition
                .child("location")
                .and_then(|n| n.attr("ref"))
                .and_then(range)
            else {
                continue;
            };
            let row_field = rows[0].attr("x").and_then(|s| s.parse::<u32>().ok());
            let value_field = fields[0].attr("fld").and_then(|s| s.parse::<u32>().ok());
            let aggregation = match fields[0].attr("subtotal").unwrap_or("sum") {
                "sum" => PivotAggregation::Sum,
                "count" => PivotAggregation::Count,
                "average" => PivotAggregation::Average,
                _ => {
                    warn(
                        &mut result.metadata.import_warnings,
                        "Unsupported pivot aggregation; original pivot retained.",
                    );
                    continue;
                }
            };
            if fields[0].attr("showDataAs").is_some_and(|s| s != "normal") {
                warn(
                    &mut result.metadata.import_warnings,
                    "Pivot calculated display operations are not represented; original retained.",
                );
                continue;
            }
            if let Some((row_field, value_field)) = row_field.zip(value_field).filter(|(a, b)| {
                *a <= end.col.saturating_sub(start.col) && *b <= end.col.saturating_sub(start.col)
            }) {
                let first_data = definition
                    .child("location")
                    .and_then(|n| n.num("firstDataRow"))
                    .unwrap_or(1.0) as u32;
                result.pivot_tables.push(PivotTable {
                    id: uuid::Uuid::new_v4(),
                    name: definition.attr("name").unwrap_or("Imported pivot").into(),
                    source_sheet_id: result.sheets[source_sheet].id,
                    source_start: CellRef::new(start.row.saturating_add(1), start.col),
                    source_end: end,
                    target_sheet_id: result.sheets[si].id,
                    target: CellRef::new(target.row.saturating_add(first_data), target.col),
                    row_field_col: start.col + row_field,
                    value_field_col: start.col + value_field,
                    aggregation,
                });
            }
        }
    }
    Ok(())
}
/// Calamine reserves from the untrusted uniqueCount header before parsing any
/// strings. Check its canonical input part first, not after Xlsx::new has run.
pub(super) fn preflight_shared_strings(bytes: &[u8]) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let Some(root) = xml::optional(&mut archive, "xl/sharedStrings.xml")? else {
        return Ok(());
    };
    if root.name != "sst" {
        return Err("Invalid shared string table root".into());
    }
    let max_unique = syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES / 200;
    let max_references = 1_000_000u64;
    for (attribute, limit) in [
        ("uniqueCount", max_unique as u64),
        ("count", max_references),
    ] {
        if let Some(raw) = root.attr(attribute) {
            let value = raw
                .parse::<u64>()
                .map_err(|_| format!("Invalid shared string {attribute} budget"))?;
            if value > limit {
                return Err(format!(
                    "Shared string {attribute} exceeds allocation budget"
                ));
            }
        }
    }
    let mut actual = 0usize;
    let mut text_bytes = 0usize;
    let mut stack = vec![&root];
    while let Some(node) = stack.pop() {
        actual += usize::from(node.name == "si");
        text_bytes = text_bytes.saturating_add(node.text.len());
        if actual > max_unique || text_bytes > syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES {
            return Err("Shared string actual content exceeds allocation budget".into());
        }
        stack.extend(&node.children);
    }
    Ok(())
}
pub(super) fn enrich(bytes: &[u8], workbook: &mut Workbook) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let root = xml::part(&mut archive, "xl/workbook.xml")?;
    let links = xml::relationships(&mut archive, "xl/workbook.xml")?;
    for relation in links
        .values()
        .filter(|r| r.kind.ends_with("/sharedStrings") && !r.external)
    {
        let strings = xml::part(&mut archive, &relation.target)?;
        if strings
            .descendants("r")
            .iter()
            .any(|run| run.child("rPr").is_some())
        {
            warn(
                &mut workbook.metadata.import_warnings,
                "Rich cell text uses one cell-wide style in Calc; original per-run formatting remains in the uploaded file.",
            );
        }
    }

    let style_path = links
        .values()
        .find(|r| r.kind.ends_with("/styles") && !r.external)
        .map(|r| r.target.as_str());
    let theme_path = links
        .values()
        .find(|r| r.kind.ends_with("/theme") && !r.external)
        .map(|r| r.target.as_str());
    let styles = styles(
        &mut archive,
        style_path,
        theme_path,
        &mut workbook.metadata.import_warnings,
    )?;
    let date1904 = root
        .child("workbookPr")
        .and_then(|n| n.flag("date1904"))
        .unwrap_or(false);
    let mut sheets = Vec::new();
    for sheet_node in root.descendants("sheet") {
        let Some(relation) = sheet_node
            .attr("r:id")
            .and_then(|id| links.get(id))
            .filter(|r| !r.external && r.kind.ends_with("/worksheet"))
        else {
            continue;
        };
        let root = xml::part(&mut archive, &relation.target)?;
        let rels = xml::relationships(&mut archive, &relation.target)?;
        sheets.push((
            sheet_node.attr("name").unwrap_or_default().to_owned(),
            root,
            rels,
        ));
    }
    let mut points_remaining = syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES / 200;
    for (name, root, relations) in &sheets {
        let Some(sheet) = workbook.sheets.iter_mut().find(|s| &s.name == name) else {
            continue;
        };
        let digit_width = styles.digit_width.unwrap_or(7.);
        let mut expanded = 0usize;
        let mut column_styles = BTreeMap::new();
        let mut row_styles = BTreeMap::new();
        for column in root.descendants("col") {
            let min = column.num("min").unwrap_or(1.0) as u32;
            let max = column.num("max").unwrap_or(min as f64) as u32;
            if min == 0 || max > 16_384 || min > max {
                return Err("Invalid worksheet column dimensions".into());
            }
            expanded = expanded.saturating_add((max - min + 1) as usize);
            if expanded > 100_000 {
                return Err("Worksheet column-style expansion budget exceeded".into());
            }
            if let Some(style) = column.attr("style").and_then(|s| s.parse::<usize>().ok()) {
                for col in min - 1..max {
                    column_styles.insert(col, style);
                }
            }
            if let Some(width) = checked_metric(column, "width", 0.0, 100_000.0)? {
                for col in min - 1..max {
                    sheet
                        .column_widths
                        .insert(col, (width * digit_width) as f32);
                }
            }
            if column.flag("hidden") == Some(true) {
                warn(
                    &mut workbook.metadata.import_warnings,
                    "Hidden worksheet columns are retained in the original; hidden-column state is not represented in Calc.",
                );
            }
        }
        if let Some(props) = root.child("sheetFormatPr")
            && let Some(default) = checked_metric(props, "defaultColWidth", 0., 100_000.)?
        {
            sheet.default_column_width = Some((default * digit_width) as f32);
        }
        for row in root.descendants("row") {
            if let (Some(index), Some(style)) = (
                row.attr("r")
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| *n > 0 && *n <= 1_048_576),
                row.attr("s").and_then(|s| s.parse::<usize>().ok()),
            ) {
                row_styles.insert(index - 1, style);
            }

            if let (Some(index), Some(height)) = (
                row.attr("r")
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|i| *i > 0 && *i <= 1_048_576),
                checked_metric(row, "ht", 0.0, 100_000.0)?,
            ) {
                sheet.row_heights.insert(index - 1, (height / 0.75) as f32);
            }
            if row.flag("hidden") == Some(true) {
                warn(
                    &mut workbook.metadata.import_warnings,
                    "Hidden worksheet rows are retained in the original; hidden-row state is not represented in Calc.",
                );
            }
        }
        if let Some(pane) = root.desc("pane") {
            if pane
                .attr("state")
                .is_some_and(|s| s == "frozen" || s == "frozenSplit")
            {
                sheet.frozen_cols = pane.num("xSplit").unwrap_or(0.0).clamp(0.0, 16384.0) as u32;
                sheet.frozen_rows = pane.num("ySplit").unwrap_or(0.0).clamp(0.0, 1048576.0) as u32;
            } else {
                warn(
                    &mut workbook.metadata.import_warnings,
                    "Split panes are not represented as frozen rows/columns; original retained.",
                );
            }
        }
        for cell in root.descendants("c") {
            let Some(reference) = cell.attr("r").and_then(CellRef::from_a1) else {
                continue;
            };
            if reference.row >= 1_048_576 || reference.col >= 16_384 {
                return Err("Worksheet cell exceeds coordinate bounds".into());
            }
            let index = cell
                .attr("s")
                .and_then(|s| s.parse::<usize>().ok())
                .or_else(|| row_styles.get(&reference.row).copied())
                .or_else(|| column_styles.get(&reference.col).copied())
                .unwrap_or(0);
            let stored = sheet.cells.entry(reference).or_insert_with(|| Cell {
                value: CellValue::Empty,
                formula: None,
                format: CellFormat::default(),
                comment: None,
            });
            if let Some(format) = styles.formats.get(index) {
                stored.format = format.clone();
            } else if index > 0 {
                warn(
                    &mut workbook.metadata.import_warnings,
                    "A cell references a missing style; default formatting was used.",
                );
            }
            if styles.date_formats.contains(&index)
                && let Some(serial) = cell
                    .child("v")
                    .and_then(|n| n.all_text().parse::<f64>().ok())
                && let Some(value) = date(serial, date1904)
            {
                stored.value = CellValue::Date(value);
            }
        }
        annotations(
            sheet,
            root,
            &styles,
            date1904,
            &mut workbook.metadata.import_warnings,
        );
        drawings(
            &mut archive,
            sheet,
            root,
            relations,
            &mut workbook.metadata.import_warnings,
            &mut points_remaining,
        )?;
        if root.child("hyperlinks").is_some() {
            warn(
                &mut workbook.metadata.import_warnings,
                "Worksheet hyperlinks are outside the current cell model; original link targets remain in the uploaded file.",
            );
        }
        if root.child("tableParts").is_some() || root.child("autoFilter").is_some() {
            warn(
                &mut workbook.metadata.import_warnings,
                "Structured tables and filter state are not represented as editable Calc tables; cell data and the original definitions are retained.",
            );
        }
        if root.child("pageSetup").is_some() || root.child("headerFooter").is_some() {
            warn(
                &mut workbook.metadata.import_warnings,
                "Worksheet print/page/header settings are not represented; download the original for its print layout.",
            );
        }
        if root
            .descendants("r")
            .iter()
            .any(|n| n.child("rPr").is_some())
        {
            warn(
                &mut workbook.metadata.import_warnings,
                "Rich cell text uses one cell-wide style in Calc; original per-run formatting remains in the uploaded file.",
            );
        }
        for relation in relations
            .values()
            .filter(|r| r.kind.ends_with("/comments") && !r.external)
        {
            let comments = xml::part(&mut archive, &relation.target)?;
            for comment in comments.descendants("comment") {
                if let Some(reference) = comment
                    .attr("ref")
                    .and_then(CellRef::from_a1)
                    .filter(|r| r.row < 1_048_576 && r.col < 16_384)
                {
                    let cell = sheet.cells.entry(reference).or_insert_with(|| Cell {
                        value: CellValue::Empty,
                        formula: None,
                        format: CellFormat::default(),
                        comment: None,
                    });
                    cell.comment = Some(xml::text(comment, "t"));
                }
            }
            warn(
                &mut workbook.metadata.import_warnings,
                "Comment text is preserved in Calc; author, threaded replies and legacy comment shape layout remain in the original file.",
            );
        }
        if root.desc("sheetProtection").is_some()
            || root.desc("legacyDrawing").is_some()
            || root.desc("extLst").is_some()
        {
            warn(
                &mut workbook.metadata.import_warnings,
                "Worksheet protection, legacy drawings or extension records may not be fully represented; the original file remains available.",
            );
        }
    }
    for name in root.descendants("definedName") {
        let Some(key) = name.attr("name") else {
            continue;
        };
        if key.starts_with("_xlnm.") {
            warn(
                &mut workbook.metadata.import_warnings,
                "Print areas/titles and other reserved defined names are not represented as editable named ranges; original retained.",
            );
            continue;
        }
        if let Some((sheet, start, end)) = qualified(name.all_text().trim())
            && let Some(sheet) = workbook.sheets.iter().find(|s| s.name == sheet)
        {
            if !workbook.named_ranges.iter().any(|r| r.name == key) {
                workbook.named_ranges.push(NamedRange {
                    name: key.into(),
                    sheet_id: sheet.id,
                    start,
                    end,
                });
            } else {
                warn(
                    &mut workbook.metadata.import_warnings,
                    "Sheet-scoped duplicate named ranges cannot be represented without scope; the first binding was imported.",
                );
            }
        } else {
            warn(
                &mut workbook.metadata.import_warnings,
                "A formula, external or multi-area defined name cannot be represented as one named rectangle; original retained.",
            );
        }
    }
    if root.desc("externalReferences").is_some() {
        warn(
            &mut workbook.metadata.import_warnings,
            "External workbook bindings are not fetched; cached values/formulas are preserved where available and the original retains its external references.",
        );
    }
    if root
        .descendants("sheet")
        .iter()
        .any(|n| n.attr("state").is_some_and(|s| s != "visible"))
    {
        warn(
            &mut workbook.metadata.import_warnings,
            "Hidden worksheet state is not represented; imported sheets are visible and the original retains visibility settings.",
        );
    }
    pivots(&mut archive, &root, &links, &sheets, workbook)?;
    Ok(())
}
