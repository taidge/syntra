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
    for sheet in &mut workbook.sheets {
        sheet.recalculate();
        let rows = sheet
            .cells
            .keys()
            .map(|cell| cell.row.saturating_add(1))
            .chain(
                sheet
                    .merged_cells
                    .iter()
                    .map(|range| range.end.row.saturating_add(1)),
            )
            .max()
            .unwrap_or(1);
        let columns = sheet
            .cells
            .keys()
            .map(|cell| cell.col.saturating_add(1))
            .chain(
                sheet
                    .merged_cells
                    .iter()
                    .map(|range| range.end.col.saturating_add(1)),
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
        budget_cells -= dense as usize;
        let widths = (0..columns)
            .map(|column| {
                finite(
                    f64::from(
                        sheet
                            .column_widths
                            .get(&column)
                            .copied()
                            .or(sheet.default_column_width)
                            .unwrap_or(75.),
                    ),
                    75.,
                    1.,
                    2048.,
                )
            })
            .collect::<Vec<_>>();
        let mut start = 0;
        while start < columns {
            let mut end = start;
            let mut width = 32.;
            while end < columns && (reader || end == start || width + widths[end as usize] <= 1000.)
            {
                width += widths[end as usize];
                end += 1;
            }
            // A merged cell remains on one horizontal print segment.
            loop {
                let crossing = sheet.merged_cells.iter().find(|range| {
                    range.start.col < end && range.end.col >= end && range.end.col < columns
                });
                let Some(range) = crossing else { break };
                if range.start.col > start {
                    end = range.start.col;
                    break;
                }
                end = range.end.col + 1;
            }
            let grid_width = 28. + widths[start as usize..end as usize].iter().sum::<f64>();
            body.push_str(&format!(
                "<section class=\"sheet\"><h1>{}</h1><table style=\"width:{grid_width}px\"><colgroup><col style=\"width:28px\">",
                escape(&sheet.name)
            ));
            for column in start..end {
                body.push_str(&format!(
                    "<col style=\"width:{}px\">",
                    widths[column as usize]
                ));
            }
            body.push_str("</colgroup><thead><tr><th></th>");
            for column in start..end {
                body.push_str(&format!(
                    "<th>{}</th>",
                    CellRef::new(0, column).to_a1().trim_end_matches('1')
                ));
            }
            body.push_str("</tr></thead><tbody>");
            for row in 0..rows {
                let row_height = finite(
                    f64::from(*sheet.row_heights.get(&row).unwrap_or(&20.)),
                    20.,
                    1.,
                    1000.,
                );
                body.push_str(&format!(
                    "<tr style=\"height:{row_height}px\"><th>{}</th>",
                    row + 1
                ));
                for column in start..end {
                    let merged = sheet.merged_cells.iter().find(|range| {
                        row >= range.start.row
                            && row <= range.end.row
                            && column >= range.start.col
                            && column <= range.end.col
                    });
                    if merged
                        .is_some_and(|range| row != range.start.row || column != range.start.col)
                    {
                        continue;
                    }
                    let spans = merged
                        .map(|range| {
                            format!(
                                " rowspan=\"{}\" colspan=\"{}\"",
                                range.end.row - range.start.row + 1,
                                (range.end.col - range.start.col + 1).min(end - column)
                            )
                        })
                        .unwrap_or_default();
                    let cell = sheet.get_cell(row, column);
                    let style = sheet
                        .conditional_format_for(row, column)
                        .or_else(|| cell.map(|cell| &cell.format));
                    let css = if let Some(range) = merged {
                        let mut style = style.cloned().unwrap_or_default();
                        if let Some(edge) = sheet
                            .get_cell(range.end.row, column)
                            .and_then(|c| c.format.borders.bottom.as_ref())
                        {
                            style.borders.bottom = Some(edge.clone());
                        }
                        if let Some(edge) = sheet
                            .get_cell(row, range.end.col)
                            .and_then(|c| c.format.borders.right.as_ref())
                        {
                            style.borders.right = Some(edge.clone());
                        }
                        cell_style(&style)
                    } else {
                        style.map(cell_style).unwrap_or_default()
                    };
                    let short_date = if reader
                        && cell.is_some_and(|c| {
                            c.format.localized_short_date && matches!(c.value, CellValue::Date(_))
                        }) {
                        " data-short-date"
                    } else {
                        ""
                    };
                    body.push_str(&format!(
                        "<td{spans}{short_date} style=\"{}\">{}</td>",
                        css,
                        cell.map(display)
                            .map(|text| escape(&text))
                            .unwrap_or_default()
                    ));
                }
                body.push_str("</tr>");
            }
            body.push_str("</tbody></table></section>");
            start = end;
        }
        for chart in &sheet.charts {
            let data = chart_data(sheet, chart)?;
            let value = serde_json::json!({"chart_type":chart.chart_type,"data":data});
            body.push_str(&format!("<section class=\"sheet-chart\"><h2>{}</h2><svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1000 500\" style=\"width:100%;height:auto\">{}</svg></section>",escape(&chart.title),canvas::chart_svg(&value,0.,0.,1000.,500.)?));
        }
        if body.len() > MAX_HTML_BYTES {
            return Err("PDF print layout exceeds the rendering budget".into());
        }
    }
    if workbook.sheets.is_empty() {
        body.push_str("<section class=\"sheet\"><p>&#160;</p></section>");
    }
    let css = "@page{size:297mm 210mm;margin:10mm}h1{font-size:15pt;margin:0 0 8pt}h2{font-size:13pt}table{border-collapse:collapse;table-layout:fixed;width:100%;font-size:10pt}thead{display:table-header-group}tr{break-inside:avoid}td,th{border:0.5pt solid #cbd5e1;padding:0 2px;overflow-wrap:anywhere;white-space:pre-wrap}th{background:#f1f5f9;color:#475569;font-weight:normal;text-align:center;font-size:9pt}td{vertical-align:bottom;overflow:hidden}.sheet{break-before:page}.sheet:first-child{break-before:auto}.sheet-chart{break-inside:avoid;margin-top:12pt}svg text{font-family:Arial,'Office CJK',sans-serif}";
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
    match &cell.value {
        CellValue::Empty => String::new(),
        CellValue::Text(value) | CellValue::Date(value) | CellValue::Error(value) => value.clone(),
        CellValue::Boolean(value) => if *value { "TRUE" } else { "FALSE" }.into(),
        CellValue::Number(value) => {
            let Some(pattern) = cell
                .format
                .number_format
                .as_deref()
                .filter(|pattern| !pattern.eq_ignore_ascii_case("general"))
            else {
                return value.to_string();
            };
            if pattern == ";;;" {
                return String::new();
            }
            let pattern = pattern.split(';').next().unwrap_or(pattern);
            let percentage = pattern.contains('%');
            let number = if percentage { *value * 100. } else { *value };
            let decimals = pattern
                .split_once('.')
                .map(|(_, decimal)| {
                    decimal
                        .chars()
                        .take_while(|c| matches!(c, '0' | '#'))
                        .count()
                })
                .unwrap_or(0)
                .min(12);
            if pattern.contains('y')
                && (pattern.contains('d') || pattern.contains('m'))
                && *value >= 0.
                && *value < 3_000_000.
            {
                if let Some(date) = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)
                    .and_then(|date| date.checked_add_signed(chrono::Duration::days(*value as i64)))
                {
                    return date.format("%Y-%m-%d").to_string();
                }
            }
            let mut formatted = format!("{number:.decimals$}");
            if pattern.contains(',') {
                let (integer, fraction) = formatted
                    .split_once('.')
                    .map(|(left, right)| (left.to_owned(), format!(".{right}")))
                    .unwrap_or((formatted.clone(), String::new()));
                let negative = integer.starts_with('-');
                let digits = integer.trim_start_matches('-').chars().collect::<Vec<_>>();
                let mut grouped = String::new();
                for (index, digit) in digits.iter().enumerate() {
                    if index > 0 && (digits.len() - index) % 3 == 0 {
                        grouped.push(',');
                    }
                    grouped.push(*digit);
                }
                formatted = format!("{}{grouped}{fraction}", if negative { "-" } else { "" });
            }
            let currency = pattern
                .chars()
                .find(|character| matches!(character, '$' | '€' | '£' | '¥'));
            format!(
                "{}{formatted}{}",
                currency.map(|c| c.to_string()).unwrap_or_default(),
                if percentage { "%" } else { "" }
            )
        }
    }
}

fn chart_data(sheet: &Sheet, chart: &SheetChart) -> Result<ChartData, String> {
    let mut data = chart.cached_data.clone().unwrap_or_else(|| ChartData {
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
            id: uuid::Uuid::new_v4(),
            title: "Chart".into(),
            chart_type: SheetChartType::Bar,
            data_start: CellRef::new(0, 0),
            data_end: CellRef::new(0, 1),
            anchor_start: CellRef::new(2, 0),
            anchor_end: CellRef::new(10, 5),
            cached_data: Some(ChartData {
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
