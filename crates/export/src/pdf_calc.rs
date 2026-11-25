use super::*;
use syntra_doc_engine::calc::*;
use syntra_doc_engine::slides::{ChartData, Dataset};

pub(super) fn render(content: &Value, reader: bool) -> Result<PrintDocument, String> {
    let mut workbook: Workbook = serde_json::from_value(content.clone())
        .map_err(|error| format!("Cannot render Calc PDF: {error}"))?;
    if workbook.sheets.len() > 100 {
        return Err("PDF workbook exceeds the 100-sheet limit".into());
    }
    let mut body = String::new();
    let mut budget_cells = MAX_OBJECTS;
    let mut budget_drawings = MAX_WORKBOOK_DRAWINGS;
    let mut budget_pages = 500usize;
    let (header_width, header_height) = if reader { (28., 20.) } else { (0., 0.) };
    for (sheet_index, sheet) in workbook.sheets.iter_mut().enumerate() {
        sheet.recalculate();
        let rows = sheet
            .cells
            .keys()
            .map(|c| c.row.saturating_add(1))
            .chain(
                sheet
                    .merged_cells
                    .iter()
                    .map(|r| r.end.row.saturating_add(1)),
            )
            .max()
            .unwrap_or(1);
        let columns = sheet
            .cells
            .keys()
            .map(|c| c.col.saturating_add(1))
            .chain(
                sheet
                    .merged_cells
                    .iter()
                    .map(|r| r.end.col.saturating_add(1)),
            )
            .max()
            .unwrap_or(1);
        let dense = u64::from(rows) * u64::from(columns);
        if rows > 10_000 || columns > 256 || dense > budget_cells as u64 {
            return Err("PDF worksheet exceeds the printable cell budget; export a smaller range or use XLSX for sparse sheets".into());
        }
        let rules = sheet
            .merged_cells
            .len()
            .saturating_add(sheet.conditional_formats.len())
            .saturating_add(1);
        if dense.saturating_mul(rules as u64) > 5_000_000 {
            return Err("PDF worksheet merged cells and conditional formatting exceed the layout-work budget".into());
        }
        if sheet.images.len() + sheet.charts.len() > MAX_SHEET_DRAWINGS {
            return Err("PDF worksheet exceeds the drawing budget".into());
        }
        budget_drawings = budget_drawings
            .checked_sub(sheet.images.len() + sheet.charts.len())
            .ok_or("PDF workbook exceeds the drawing budget")?;
        let layout = sheet.drawing_layout();
        budget_cells -= dense as usize;
        // Model dimensions are already CSS pixels, also used by the editor.
        let widths = (0..columns)
            .map(|c| sheet.column_width_px(c))
            .collect::<Vec<_>>();
        let heights = (0..rows)
            .map(|r| sheet.row_height_px(r))
            .collect::<Vec<_>>();
        let grid_width = widths.iter().sum::<f64>() + header_width;
        let mut natural_width = grid_width.max(1.);
        let mut natural_height = (heights.iter().sum::<f64>() + header_height).max(1.);
        for picture in &sheet.images {
            let (x, y, w, h) = layout.drawing_bounds(&picture.anchor_start, &picture.drawing);
            natural_width = natural_width.max(x + w + header_width);
            natural_height = natural_height.max(y + h + header_height);
        }
        for chart in &sheet.charts {
            let (x, y, w, h) = layout.chart_bounds(chart);
            natural_width = natural_width.max(x + w + header_width);
            natural_height = natural_height.max(y + h + header_height);
        }
        // Fit the sheet horizontally once. Vertical pages retain all columns,
        // including merges and drawing anchors, rather than cutting each into
        // unrelated horizontal tables. Chrome's zoom scales layout and text.
        let scale = if reader {
            1.
        } else {
            (1040. / natural_width).min(1.)
        };
        let page_height = if reader { natural_height } else { 708. / scale };
        if (natural_height / page_height).ceil() > budget_pages as f64 {
            return Err("PDF workbook exceeds the 500-page print budget".into());
        }
        let mut row_boundaries = vec![0.];
        for height in &heights {
            row_boundaries.push(row_boundaries.last().copied().unwrap_or(0.) + height);
        }
        if dense.saturating_mul((natural_height / page_height).ceil().max(1.) as u64) > 2_000_000 {
            return Err("PDF repeated-page cell budget exceeded".into());
        }
        let mut page_top = 0.;
        while page_top < natural_height {
            budget_pages = budget_pages
                .checked_sub(1)
                .ok_or("PDF workbook exceeds the 500-page print budget")?;
            let mut page_bottom = (page_top + page_height).min(natural_height);
            if page_bottom < natural_height {
                if let Some(boundary) = row_boundaries
                    .iter()
                    .rev()
                    .find(|boundary| **boundary <= page_bottom && **boundary > page_top)
                {
                    page_bottom = *boundary;
                }
                for merged in &sheet.merged_cells {
                    let top = row_boundaries
                        .get(merged.start.row as usize)
                        .copied()
                        .unwrap_or(0.);
                    let bottom = row_boundaries
                        .get(merged.end.row.saturating_add(1) as usize)
                        .copied()
                        .unwrap_or(natural_height);
                    if top < page_bottom && bottom > page_bottom && top > page_top {
                        page_bottom = top;
                    }
                }
            }
            if reader {
                body.push_str(&format!(
                    "<section class=\"sheet\"><h1>{}</h1>",
                    escape(&sheet.name)
                ));
            } else {
                body.push_str(&format!(
                    "<section class=\"sheet\" aria-label=\"{}\">",
                    escape(&sheet.name)
                ));
            }
            body.push_str(&format!(
                "<div class=\"sheet-page\" style=\"height:{}px;overflow:hidden\"><div class=\"sheet-surface\" style=\"zoom:{scale};width:{natural_width}px;height:{natural_height}px;top:-{page_top}px\"><table style=\"width:{grid_width}px\"><colgroup>",
                (page_bottom - page_top) * scale
            ));
            if reader {
                body.push_str(&format!("<col style=\"width:{header_width}px\">"));
            }
            for width in &widths {
                body.push_str(&format!("<col style=\"width:{width}px\">"));
            }
            body.push_str("</colgroup>");
            if reader {
                body.push_str(&format!(
                    "<thead><tr style=\"height:{header_height}px\"><th></th>"
                ));
                for column in 0..columns {
                    body.push_str(&format!(
                        "<th scope=\"col\">{}</th>",
                        CellRef::new(0, column).to_a1().trim_end_matches('1')
                    ));
                }
                body.push_str("</tr></thead>");
            }
            body.push_str("<tbody>");
            for row in 0..rows {
                body.push_str(&format!(
                    "<tr style=\"height:{}px\">",
                    heights[row as usize]
                ));
                if reader {
                    body.push_str(&format!("<th scope=\"row\">{}</th>", row + 1));
                }
                for column in 0..columns {
                    let merged = sheet.merged_cells.iter().find(|r| {
                        row >= r.start.row
                            && row <= r.end.row
                            && column >= r.start.col
                            && column <= r.end.col
                    });
                    if merged.is_some_and(|r| row != r.start.row || column != r.start.col) {
                        continue;
                    }
                    let spans = merged
                        .map(|r| {
                            format!(
                                " rowspan=\"{}\" colspan=\"{}\"",
                                r.end.row - r.start.row + 1,
                                r.end.col - r.start.col + 1
                            )
                        })
                        .unwrap_or_default();
                    let cell = sheet.get_cell(row, column);
                    let mut format = sheet.effective_format(row, column);
                    let display = cell
                        .map(|c| format_cell_value(&c.value, c.format.number_format.as_deref()));
                    if let Some(color) = display.as_ref().and_then(|d| d.color.clone()) {
                        format.text_color = Some(color);
                    }
                    let (short_date, text) = match cell {
                        Some(c) if reader && c.format.localized_short_date => match &c.value {
                            CellValue::Date(iso) => {
                                (" data-short-date", iso.split('T').next().unwrap_or(iso))
                            }
                            _ => ("", display.as_ref().map(|d| d.text.as_str()).unwrap_or("")),
                        },
                        _ => ("", display.as_ref().map(|d| d.text.as_str()).unwrap_or("")),
                    };
                    body.push_str(&format!(
                        "<td{spans}{short_date} style=\"{}\">{}</td>",
                        cell_style(&format),
                        escape(text)
                    ));
                }
                body.push_str("</tr>");
            }
            body.push_str("</tbody></table>");
            for (index, picture) in sheet.images.iter().enumerate() {
                let (x, y, w, h) = layout.drawing_bounds(&picture.anchor_start, &picture.drawing);
                let (x, y) = (x + header_width, y + header_height);
                let clip = format!("sheet-image-{sheet_index}-{budget_pages}-{index}");
                let url = image(&picture.data_url)?;
                let [left, top, right, bottom] = picture.crop.map(|v| f64::from(v).clamp(0., 0.99));
                let iw = w / (1. - left - right).max(0.01);
                let ih = h / (1. - top - bottom).max(0.01);
                let rotation = finite(f64::from(picture.rotation), 0., -360000., 360000.);
                body.push_str(&format!("<svg class=\"sheet-drawing sheet-image\" aria-label=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" style=\"left:{x}px;top:{y}px;width:{w}px;height:{h}px;overflow:visible\"><defs><clipPath id=\"{clip}\"><rect width=\"{w}\" height=\"{h}\"/></clipPath></defs><g transform=\"rotate({rotation} {} {})\"><g clip-path=\"url(#{clip})\"><image href=\"{url}\" x=\"{}\" y=\"{}\" width=\"{iw}\" height=\"{ih}\" preserveAspectRatio=\"none\"/></g></g></svg>",escape(&picture.name),w/2.,h/2.,-iw*left,-ih*top));
            }
            for chart in &sheet.charts {
                let (x, y, w, h) = layout.chart_bounds(chart);
                let (x, y) = (x + header_width, y + header_height);
                let data = chart_data(sheet, chart)?;
                let value = serde_json::json!({"chart_type":chart.chart_type,"data":data,"title":chart.title});
                body.push_str(&format!("<svg class=\"sheet-drawing sheet-chart\" aria-label=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" style=\"left:{x}px;top:{y}px;width:{w}px;height:{h}px\"><rect class=\"worksheet-chart-background\" width=\"{w}\" height=\"{h}\" fill=\"#FFFFFF\"/>{}</svg>",escape(&chart.title),canvas::chart_svg(&value,0.,0.,w,h)?));
            }
            body.push_str("</div></div></section>");
            page_top = page_bottom;
            if body.len() > MAX_HTML_BYTES {
                return Err("PDF print layout exceeds the rendering budget".into());
            }
        }
    }
    if workbook.sheets.is_empty() {
        body.push_str("<section class=\"sheet\"></section>");
    }
    // Give explicit cell borders precedence over the track gridlines.
    let css = "@page{size:297mm 210mm;margin:10mm}.sheet{break-before:page}.sheet:first-child{break-before:auto}.sheet-page{position:relative;break-inside:avoid}.sheet-surface{position:relative}table{border-collapse:collapse;table-layout:fixed;font-size:10pt}tr{break-inside:avoid}tr,col{border:0.5pt solid #cbd5e1}th{background:#f1f5f9;color:#475569;font-weight:normal;text-align:center;font-size:9pt}td{padding:1px;overflow:hidden;white-space:pre;vertical-align:bottom;line-height:1.2}.sheet-drawing{position:absolute;display:block;background:transparent}svg text{font-family:Arial,'Office CJK',sans-serif}";
    Ok(html(&workbook.title, css, &body, 297., 210.))
}

fn cell_style(format: &CellFormat) -> String {
    let align = match format.horizontal_align {
        HAlign::Left => "left",
        HAlign::Center => "center",
        HAlign::Right => "right",
    };
    let vertical = match format.vertical_align {
        VAlign::Top => "top",
        VAlign::Middle => "middle",
        VAlign::Bottom => "bottom",
    };
    let mut result = format!(
        "text-align:{align};vertical-align:{vertical};font-family:{};font-size:{}pt;color:{};background:{};font-weight:{};font-style:{};text-decoration:{};white-space:{};",
        font(format.font_family.as_deref().unwrap_or("Arial")),
        finite(
            format.font_size.map(f64::from).unwrap_or(10.),
            10.,
            1.,
            1000.
        ),
        color(format.text_color.as_deref(), "#111827"),
        color(format.bg_color.as_deref(), "transparent"),
        if format.bold { "bold" } else { "normal" },
        if format.italic { "italic" } else { "normal" },
        if format.underline {
            "underline"
        } else {
            "none"
        },
        if format.wrap_text { "pre-wrap" } else { "pre" }
    );
    for (edge, border) in [
        ("top", &format.borders.top),
        ("right", &format.borders.right),
        ("bottom", &format.borders.bottom),
        ("left", &format.borders.left),
    ] {
        if let Some(border) = border {
            let (width, style) = match border.style {
                BorderStyle::Thin => (0.5, "solid"),
                BorderStyle::Medium => (1., "solid"),
                BorderStyle::Thick => (2., "solid"),
                BorderStyle::Dashed => (1., "dashed"),
                BorderStyle::Dotted => (1., "dotted"),
                BorderStyle::Double => (2., "double"),
            };
            result.push_str(&format!(
                "border-{edge}:{width}pt {style} {};",
                color(Some(&border.color), "#111827")
            ));
        }
    }
    result
}

fn display(cell: &Cell) -> String {
    format_cell_value(&cell.value, cell.format.number_format.as_deref()).text
}

fn chart_data(sheet: &Sheet, chart: &SheetChart) -> Result<ChartData, String> {
    let mut data = chart.cached_data.clone().unwrap_or_else(|| ChartData {
        presentation: None,
        labels: Vec::new(),
        datasets: Vec::new(),
    });
    if !chart.series_sources.is_empty() {
        for (index, source) in chart.series_sources.iter().enumerate() {
            let rows = source.value_end.row.saturating_sub(source.value_start.row) + 1;
            let columns = source.value_end.col.saturating_sub(source.value_start.col) + 1;
            if u64::from(rows) * u64::from(columns) > 10_000 {
                return Err("PDF chart data exceeds the rendering budget".into());
            }
            let mut values = Vec::new();
            for row in source.value_start.row..=source.value_end.row {
                for column in source.value_start.col..=source.value_end.col {
                    values.push(
                        sheet
                            .get_cell(row, column)
                            .and_then(|cell| match cell.value {
                                CellValue::Number(value) => Some(value),
                                _ => None,
                            })
                            .unwrap_or(0.),
                    );
                }
            }
            if let Some(series) = data.datasets.get_mut(index) {
                series.values = values;
            } else {
                data.datasets.push(Dataset {
                    name: format!("Series {}", index + 1),
                    values,
                    color: None,
                });
            }
            if index == 0
                && let Some((start, end)) =
                    source.label_start.as_ref().zip(source.label_end.as_ref())
            {
                let rows = u64::from(end.row.saturating_sub(start.row)) + 1;
                let columns = u64::from(end.col.saturating_sub(start.col)) + 1;
                if rows * columns > 10_000 {
                    return Err("PDF chart labels exceed the rendering budget".into());
                }
                data.labels = (start.row..=end.row)
                    .flat_map(|row| (start.col..=end.col).map(move |column| (row, column)))
                    .map(|(row, column)| {
                        sheet.get_cell(row, column).map(display).unwrap_or_default()
                    })
                    .collect();
            }
        }
    } else if chart.cached_data.is_none() {
        let rows = chart.data_end.row.saturating_sub(chart.data_start.row) + 1;
        let columns = chart.data_end.col.saturating_sub(chart.data_start.col) + 1;
        if u64::from(rows) * u64::from(columns) > 10_000 {
            return Err("PDF chart data exceeds the rendering budget".into());
        }
        for row in chart.data_start.row.saturating_add(1)..=chart.data_end.row {
            data.labels.push(
                sheet
                    .get_cell(row, chart.data_start.col)
                    .map(display)
                    .unwrap_or_default(),
            );
        }
        for column in chart.data_start.col.saturating_add(1)..=chart.data_end.col {
            data.datasets.push(Dataset {
                name: sheet
                    .get_cell(chart.data_start.row, column)
                    .map(display)
                    .unwrap_or_default(),
                color: None,
                values: (chart.data_start.row.saturating_add(1)..=chart.data_end.row)
                    .map(|row| {
                        sheet
                            .get_cell(row, column)
                            .and_then(|cell| match cell.value {
                                CellValue::Number(value) => Some(value),
                                _ => None,
                            })
                            .unwrap_or(0.)
                    })
                    .collect(),
            });
        }
    }
    data.presentation = Some(chart.resolved_presentation());
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formulas_are_recalculated_and_merged_formatted_cells_are_printed() {
        let mut workbook = Workbook::new("账单".into());
        let sheet = &mut workbook.sheets[0];
        sheet.set_cell(0, 0, CellValue::Number(12.5));
        sheet.set_formula(1, 0, "=A1*2".into());
        sheet
            .cells
            .get_mut(&CellRef::new(1, 0))
            .unwrap()
            .format
            .number_format = Some("0.00".into());
        sheet.merged_cells.push(MergedRange {
            start: CellRef::new(0, 1),
            end: CellRef::new(0, 2),
        });
        let printed = render(&serde_json::to_value(workbook).unwrap(), false).unwrap();
        assert!(printed.html.contains("25.00"));
        assert!(printed.html.contains("colspan=\"2\""));
        assert!(printed.html.contains("@page{size:297mm 210mm"));
    }

    #[test]
    fn bound_chart_labels_and_values_use_the_current_sheet() {
        let mut workbook = Workbook::new("Current chart".into());
        let sheet = &mut workbook.sheets[0];
        sheet.set_cell(0, 0, CellValue::Text("Fresh label".into()));
        sheet.set_cell(0, 1, CellValue::Number(42.));
        let chart = SheetChart {
            drawing: None,
            id: uuid::Uuid::new_v4(),
            title: "Chart".into(),
            chart_type: SheetChartType::Bar,
            data_start: CellRef::new(0, 0),
            data_end: CellRef::new(0, 1),
            anchor_start: CellRef::new(2, 0),
            anchor_end: CellRef::new(10, 5),
            cached_data: Some(ChartData {
                presentation: None,
                labels: vec!["Stale label".into()],
                datasets: vec![Dataset {
                    name: "Values".into(),
                    values: vec![1.],
                    color: None,
                }],
            }),
            series_sources: vec![SheetChartSource {
                value_start: CellRef::new(0, 1),
                value_end: CellRef::new(0, 1),
                label_start: Some(CellRef::new(0, 0)),
                label_end: Some(CellRef::new(0, 0)),
            }],
        };
        let data = chart_data(sheet, &chart).unwrap();
        assert_eq!(data.labels, vec!["Fresh label"]);
        assert_eq!(data.datasets[0].values, vec![42.]);
    }

    #[test]
    fn dense_rows_and_many_merged_ranges_cannot_amplify_layout_work() {
        let mut workbook = Workbook::new("Bounded layout".into());
        let sheet = &mut workbook.sheets[0];
        sheet.set_cell(999, 0, CellValue::Text("Last row".into()));
        sheet.merged_cells = vec![
            MergedRange {
                start: CellRef::new(0, 0),
                end: CellRef::new(0, 0)
            };
            5_000
        ];
        assert!(
            render(&serde_json::to_value(workbook).unwrap(), false)
                .unwrap_err()
                .contains("layout-work budget")
        );
    }
}
