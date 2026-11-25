use super::*;

/// Bound the dense grid expansion before allocating ZIP/XML output. Typed
/// callers can construct sparse grids much larger than the JSON byte budget.
pub(super) fn validate_tables(pres: &Presentation) -> Result<(), String> {
    const MAX_GRID: usize = 100_000;
    const MAX_WORK: usize = 5_000_000;
    const MAX_XML: usize = 48 * 1024 * 1024;
    struct Budget {
        grid: usize,
        work: usize,
        xml: usize,
        nodes: usize,
    }
    fn add(total: &mut usize, value: usize, limit: usize, message: &str) -> Result<(), String> {
        *total = total
            .checked_add(value)
            .filter(|n| *n <= limit)
            .ok_or_else(|| message.to_string())?;
        Ok(())
    }
    fn rgb(value: &str) -> bool {
        value.len() == 7
            && value.starts_with('#')
            && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
    }
    fn visit(elements: &[SlideElement], depth: usize, budget: &mut Budget) -> Result<(), String> {
        if depth > 64 {
            return Err("Native table traversal exceeds the nesting budget".into());
        }
        add(
            &mut budget.nodes,
            elements.len(),
            100_000,
            "Native presentation exceeds the element budget",
        )?;
        for element in elements {
            match element {
                SlideElement::Group(group) => visit(&group.elements, depth + 1, budget)?,
                SlideElement::Table(table) => {
                    let columns = table.column_widths.len();
                    let rows = table.row_heights.len();
                    if columns == 0 || rows == 0 {
                        return Err("Native table requires a nonempty grid".into());
                    }
                    let grid = columns
                        .checked_mul(rows)
                        .ok_or("Native table grid budget exceeded")?;
                    add(
                        &mut budget.grid,
                        grid,
                        MAX_GRID,
                        "Native table grid budget exceeded",
                    )?;
                    let work = grid
                        .checked_mul(table.cells.len().max(1))
                        .ok_or("Native table dense-work budget exceeded")?;
                    add(
                        &mut budget.work,
                        work,
                        MAX_WORK,
                        "Native table dense-work budget exceeded",
                    )?;
                    // Each expanded cell emits bounded geometry, margins and
                    // border markup. Text/font strings occur at owner cells.
                    add(
                        &mut budget.xml,
                        grid.checked_mul(1024)
                            .ok_or("Native table XML expansion budget exceeded")?,
                        MAX_XML,
                        "Native table XML expansion budget exceeded",
                    )?;
                    if !table.width.is_finite()
                        || !table.height.is_finite()
                        || table.width <= 0.
                        || table.height <= 0.
                        || table.width > 100_000.
                        || table.height > 100_000.
                        || !table.x.is_finite()
                        || !table.y.is_finite()
                        || table.x.abs() > 10_000_000.
                        || table.y.abs() > 10_000_000.
                        || !table.rotation.is_finite()
                    {
                        return Err("Native table has invalid finite geometry".into());
                    }
                    if table
                        .column_widths
                        .iter()
                        .chain(&table.row_heights)
                        .any(|n| !n.is_finite() || *n <= 0. || *n > 10_000_000.)
                    {
                        return Err(
                            "Native table grid dimensions must be finite and positive".into()
                        );
                    }
                    let mut owners = vec![false; grid];
                    for cell in &table.cells {
                        let row_end = cell
                            .row
                            .checked_add(cell.row_span)
                            .filter(|n| cell.row_span > 0 && *n <= rows)
                            .ok_or("Native table owner row/span is outside the grid")?;
                        let column_end = cell
                            .column
                            .checked_add(cell.column_span)
                            .filter(|n| cell.column_span > 0 && *n <= columns)
                            .ok_or("Native table owner column/span is outside the grid")?;
                        if cell.row >= rows || cell.column >= columns {
                            return Err("Native table owner is outside the grid".into());
                        }
                        for row in cell.row..row_end {
                            for column in cell.column..column_end {
                                let owner = &mut owners[row * columns + column];
                                if *owner {
                                    return Err("Native table merge owners overlap".into());
                                }
                                *owner = true;
                            }
                        }
                        let l = &cell.text_layout;
                        if [l.inset_left, l.inset_right, l.inset_top, l.inset_bottom]
                            .iter()
                            .any(|n| !n.is_finite() || *n < 0. || *n > 100_000.)
                        {
                            return Err(
                                "Native table cell margins must be finite and nonnegative".into()
                            );
                        }
                        if cell.background_color.as_deref().is_some_and(|s| !rgb(s)) {
                            return Err("Native table cell fill requires an RGB color".into());
                        }
                        for border in [
                            &cell.borders.top,
                            &cell.borders.right,
                            &cell.borders.bottom,
                            &cell.borders.left,
                        ]
                        .into_iter()
                        .flatten()
                        {
                            if !rgb(&border.color)
                                || !border.width.is_finite()
                                || border.width < 0.
                                || border.width > 1000.
                            {
                                return Err("Native table cell border is outside the supported color/width budget".into());
                            }
                        }
                        add(
                            &mut budget.xml,
                            512,
                            MAX_XML,
                            "Native table XML expansion budget exceeded",
                        )?;
                        for run in &cell.content {
                            let estimate = run
                                .text
                                .len()
                                .checked_mul(6)
                                .and_then(|n| {
                                    n.checked_add(
                                        run.font_family
                                            .as_ref()
                                            .map_or(0, String::len)
                                            .saturating_mul(24),
                                    )
                                })
                                .and_then(|n| {
                                    n.checked_add(
                                        run.color.as_ref().map_or(0, String::len).saturating_mul(6),
                                    )
                                })
                                .and_then(|n| n.checked_add(512))
                                .ok_or("Native table text XML expansion budget exceeded")?;
                            add(
                                &mut budget.xml,
                                estimate,
                                MAX_XML,
                                "Native table text XML expansion budget exceeded",
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut budget = Budget {
        grid: 0,
        work: 0,
        xml: 0,
        nodes: 0,
    };
    for slide in &pres.slides {
        visit(&slide.elements, 0, &mut budget)?;
    }
    Ok(())
}

pub(super) fn text_body_to_xml(
    runs: &[SlideTextRun],
    layout: Option<&SlideTextLayout>,
    padding: f32,
    tag: &str,
    resources: &SlideResources,
    xml: &mut String,
) {
    let inset = |name: fn(&SlideTextLayout) -> f32| {
        (layout.map(name).unwrap_or(padding).max(0.) * 9525.) as i64
    };
    let anchor = match layout.map(|l| l.vertical_alignment.as_str()) {
        Some("center") => "ctr",
        Some("bottom") => "b",
        _ => "t",
    };
    xml.push_str(&format!("<{tag}><a:bodyPr wrap=\"{}\" anchor=\"{anchor}\" lIns=\"{}\" rIns=\"{}\" tIns=\"{}\" bIns=\"{}\"/><a:lstStyle/>",if layout.is_some_and(|l|!l.wrap){"none"}else{"square"},inset(|l|l.inset_left),inset(|l|l.inset_right),inset(|l|l.inset_top),inset(|l|l.inset_bottom)));
    let mut opened = false;
    let mut index = None;
    for run in runs {
        if run.paragraph_break {
            continue;
        }
        let current = run.paragraph.as_ref().map(|p| p.index);
        if !opened || (current.is_some() && current != index) {
            if opened {
                xml.push_str("</a:p>");
            }
            xml.push_str("<a:p>");
            opened = true;
            if let Some(p) = &run.paragraph {
                let alignment = match p.alignment.as_str() {
                    "center" => "ctr",
                    "right" => "r",
                    "justify" => "just",
                    _ => "l",
                };
                xml.push_str(&format!("<a:pPr algn=\"{alignment}\">"));
                if let Some(points) = p.line_height_points {
                    xml.push_str(&format!(
                        "<a:lnSpc><a:spcPts val=\"{}\"/></a:lnSpc>",
                        (points * 100.) as i64
                    ));
                } else if let Some(factor) = p.line_height {
                    xml.push_str(&format!(
                        "<a:lnSpc><a:spcPct val=\"{}\"/></a:lnSpc>",
                        (factor * 100000.).round() as i64
                    ));
                }
                for (tag, points) in [
                    ("spcBef", p.space_before_points),
                    ("spcAft", p.space_after_points),
                ] {
                    if let Some(v) = points {
                        xml.push_str(&format!(
                            "<a:{tag}><a:spcPts val=\"{}\"/></a:{tag}>",
                            (v * 100.) as i64
                        ));
                    }
                }
                xml.push_str("</a:pPr>");
            }
        }
        index = current;
        for (i, part) in run.text.split('\n').enumerate() {
            if i > 0 {
                xml.push_str("<a:br/>");
            }
            if !part.is_empty() {
                let mut value = run.clone();
                value.text = part.into();
                run_to_xml(&value, resources, xml);
            }
        }
    }
    if !opened {
        xml.push_str("<a:p>");
    }
    xml.push_str(&format!("</a:p></{tag}>"));
}

fn table_borders_to_xml(borders: &TableCellBorders, xml: &mut String) {
    for (tag, border) in [
        ("lnT", &borders.top),
        ("lnR", &borders.right),
        ("lnB", &borders.bottom),
        ("lnL", &borders.left),
    ] {
        if let Some(b) = border {
            xml.push_str(&format!("<a:{tag} w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:prstDash val=\"solid\"/></a:{tag}>",(b.width.max(0.)*9525.) as i64,escape_xml(b.color.trim_start_matches('#'))));
        } else {
            xml.push_str(&format!("<a:{tag}><a:noFill/></a:{tag}>"));
        }
    }
}
pub(super) fn table_to_xml(
    table: &SlideTable,
    sp_id: u32,
    resources: &SlideResources,
    xml: &mut String,
) {
    xml.push_str(&format!("<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{sp_id}\" name=\"Table {sp_id}\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm rot=\"{}\"><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr/><a:tblGrid>",(table.rotation*60000.) as i64,(table.x*9525.) as i64,(table.y*9525.) as i64,(table.width*9525.) as i64,(table.height*9525.) as i64));
    for width in &table.column_widths {
        xml.push_str(&format!("<a:gridCol w=\"{}\"/>", (*width * 9525.) as i64));
    }
    xml.push_str("</a:tblGrid>");
    for (row, height) in table.row_heights.iter().enumerate() {
        xml.push_str(&format!("<a:tr h=\"{}\">", (*height * 9525.) as i64));
        for column in 0..table.column_widths.len() {
            let owner = table.cells.iter().find(|c| {
                row >= c.row
                    && row < c.row.saturating_add(c.row_span.max(1))
                    && column >= c.column
                    && column < c.column.saturating_add(c.column_span.max(1))
            });
            if let Some(cell) = owner {
                let anchor = row == cell.row && column == cell.column;
                let attrs = if anchor {
                    format!(
                        " gridSpan=\"{}\" rowSpan=\"{}\"",
                        cell.column_span.max(1),
                        cell.row_span.max(1)
                    )
                } else {
                    format!(
                        " hMerge=\"{}\" vMerge=\"{}\"",
                        u8::from(column > cell.column),
                        u8::from(row > cell.row)
                    )
                };
                xml.push_str(&format!("<a:tc{attrs}>"));
                text_body_to_xml(
                    if anchor { &cell.content } else { &[] },
                    Some(&cell.text_layout),
                    0.,
                    "a:txBody",
                    resources,
                    xml,
                );
                let l = &cell.text_layout;
                let anchor = match l.vertical_alignment.as_str() {
                    "center" => "ctr",
                    "bottom" => "b",
                    _ => "t",
                };
                xml.push_str(&format!(
                    "<a:tcPr anchor=\"{anchor}\" marL=\"{}\" marR=\"{}\" marT=\"{}\" marB=\"{}\">",
                    (l.inset_left * 9525.) as i64,
                    (l.inset_right * 9525.) as i64,
                    (l.inset_top * 9525.) as i64,
                    (l.inset_bottom * 9525.) as i64
                ));
                table_borders_to_xml(&cell.borders, xml);
                if let Some(fill) = &cell.background_color {
                    xml.push_str(&format!(
                        "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
                        escape_xml(fill.trim_start_matches('#'))
                    ));
                }
                xml.push_str("</a:tcPr></a:tc>");
            } else {
                xml.push_str(
                    "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc>",
                );
            }
        }
        xml.push_str("</a:tr>");
    }
    xml.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
}
