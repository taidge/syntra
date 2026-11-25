use super::*;
use std::collections::HashMap;
use syntra_doc_engine::writer::*;

pub(super) fn render(content: &Value) -> Result<PrintDocument, String> {
    let document: WriterDocument = serde_json::from_value(content.clone())
        .map_err(|error| format!("Cannot render Writer PDF: {error}"))?;
    let (width, height, mut css) = page_css(
        "writer_base",
        &document.page_settings,
        &document.professional.header,
        &document.professional.footer,
        document.professional.page_numbers,
        document.professional.header_alignment,
        document.professional.footer_alignment,
    )?;
    let page = &document.page_settings;
    css.push_str(&format!(
        ":root{{--writer-page-margins:{}mm {}mm {}mm {}mm}}",
        page.margin_top, page.margin_right, page.margin_bottom, page.margin_left
    ));
    css.push_str("body{line-height:1.15}.writer-minimum-line>span{line-height:normal}.writer-section{page:writer_base}p{margin:0 0 8pt;min-height:1em;white-space:pre-wrap;overflow-wrap:anywhere;orphans:2;widows:2}h1,h2,h3,h4,h5,h6{margin:12pt 0 8pt;break-after:avoid;white-space:pre-wrap}h1{font-size:24pt}h2{font-size:20pt}h3{font-size:18pt}h4{font-size:16pt}h5{font-size:14pt}h6{font-size:12pt}table{border-collapse:collapse;width:100%;table-layout:fixed;margin:8pt 0}td{border:1px solid #9ca3af;padding:5pt;vertical-align:top;overflow-wrap:anywhere}td p:last-child{margin-bottom:0}tr{break-inside:avoid}pre{font-family:monospace,'Office CJK',sans-serif;white-space:pre-wrap;overflow-wrap:anywhere;background:#f3f4f6;padding:8pt;font-size:10pt}blockquote{border-left:3pt solid #9ca3af;margin:8pt 0;padding-left:12pt}figure{margin:8pt 0;break-inside:avoid}figure img{max-width:100%;object-fit:contain}li{white-space:pre-wrap}li p{margin:0}.writer-page-break{break-after:page;height:0}a{color:inherit}ul,ol{padding-left:24pt}");
    // Word suppresses paragraph space-before on the first block after an
    // explicit page break; applying the heading's normal before-spacing here
    // would move every new-page heading down by an extra paragraph gap.
    css.push_str(".writer-page-break+*{margin-top:0!important}");
    if document.professional.different_first_page {
        css.push_str("@page:first{@top-left{content:none}@top-center{content:none}@top-right{content:none}@bottom-left{content:none}@bottom-center{content:none}@bottom-right{content:none}}");
    }
    let formats = document
        .professional
        .paragraph_formats
        .iter()
        .map(|format| (format.block_id, format))
        .collect::<HashMap<_, _>>();
    let mut state = WriterState {
        formats,
        remaining: MAX_OBJECTS,
        font_metrics: &document.professional.font_metrics,
    };
    let mut body = String::from("<main>");
    let mut sections = document
        .professional
        .sections
        .iter()
        .filter(|section| section.start_block < document.body.len())
        .collect::<Vec<_>>();
    sections.sort_by_key(|section| section.start_block);
    sections.dedup_by_key(|section| section.start_block);
    let mut section_index = 0;
    body.push_str("<section class=\"writer-section\">");
    for (index, block) in document.body.iter().enumerate() {
        if let Some(section) = sections
            .get(section_index)
            .filter(|section| section.start_block == index)
        {
            let name = format!("writer_section_{section_index}");
            let (_, _, section_css) = page_css(
                &name,
                &section.page_settings,
                section
                    .header
                    .as_deref()
                    .unwrap_or(&document.professional.header),
                section
                    .footer
                    .as_deref()
                    .unwrap_or(&document.professional.footer),
                document.professional.page_numbers,
                document.professional.header_alignment,
                document.professional.footer_alignment,
            )?;
            css.push_str(&section_css);
            body.push_str(&format!(
                "</section><section class=\"writer-section\" style=\"page:{name};{}\">",
                if index == 0 { "" } else { "break-before:page" }
            ));
            section_index += 1;
        }
        state.block(block, &mut body, 0)?;
    }
    if document.body.is_empty() {
        body.push_str("<p>&#160;</p>");
    }
    if !document.professional.footnotes.is_empty() || !document.professional.endnotes.is_empty() {
        body.push_str("<section style=\"margin-top:18pt;border-top:1px solid #9ca3af\">");
        for note in document
            .professional
            .footnotes
            .iter()
            .chain(&document.professional.endnotes)
        {
            body.push_str(&format!(
                "<p><sup>{}</sup> {}</p>",
                note.id,
                escape(&note.text)
            ));
        }
        body.push_str("</section>");
    }
    if !document.professional.bibliography.is_empty() {
        body.push_str("<section><h2>References</h2><ol>");
        for reference in &document.professional.bibliography {
            body.push_str(&format!(
                "<li>{}. {}{}</li>",
                escape(&reference.author),
                escape(&reference.title),
                reference
                    .year
                    .map(|year| format!(" ({year})"))
                    .unwrap_or_default()
            ));
        }
        body.push_str("</ol></section>");
    }
    body.push_str("</section></main>");
    Ok(html(&document.title, &css, &body, width, height))
}

fn page_css(
    name: &str,
    settings: &PageSettings,
    header: &str,
    footer: &str,
    numbers: bool,
    header_alignment: Option<Alignment>,
    footer_alignment: Option<Alignment>,
) -> Result<(f64, f64, String), String> {
    let mut width = f64::from(settings.width_mm);
    let mut height = f64::from(settings.height_mm);
    if !width.is_finite()
        || !height.is_finite()
        || !(20.0..=2000.).contains(&width)
        || !(20.0..=2000.).contains(&height)
    {
        return Err("PDF page dimensions must be finite and between 20 and 2000 mm".into());
    }
    if settings.orientation == Orientation::Landscape && width < height {
        std::mem::swap(&mut width, &mut height);
    }
    let margins = [
        settings.margin_top,
        settings.margin_right,
        settings.margin_bottom,
        settings.margin_left,
    ]
    .map(f64::from);
    if margins
        .iter()
        .any(|margin| !margin.is_finite() || *margin < 0.)
        || margins[0] + margins[2] > height - 5.
        || margins[1] + margins[3] > width - 5.
    {
        return Err("PDF page margins leave no printable content area".into());
    }
    let footer = format!(
        "'{}'{}",
        css_string(footer),
        if numbers { " '  ' counter(page)" } else { "" }
    );
    let box_suffix = |alignment| match alignment {
        Some(Alignment::Left | Alignment::Justify) => "left",
        Some(Alignment::Right) => "right",
        _ => "center",
    };
    let header_box = box_suffix(header_alignment);
    let footer_box = box_suffix(footer_alignment);
    Ok((
        width,
        height,
        format!(
            "@page {name}{{size:{width}mm {height}mm;margin:{}mm {}mm {}mm {}mm;@top-{header_box}{{content:'{}';font:9pt Arial,'Office CJK',sans-serif;color:#475569}}@bottom-{footer_box}{{content:{footer};font:9pt Arial,'Office CJK',sans-serif;color:#475569}}}}",
            margins[0],
            margins[1],
            margins[2],
            margins[3],
            css_string(header)
        ),
    ))
}

struct WriterState<'a> {
    formats: HashMap<uuid::Uuid, &'a ParagraphFormat>,
    remaining: usize,
    font_metrics: &'a std::collections::BTreeMap<String, FontMetrics>,
}

fn alignment(value: Alignment) -> &'static str {
    match value {
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::Justify => "justify",
    }
}

fn minimum_line_class(format: Option<&ParagraphFormat>) -> &'static str {
    if format.is_some_and(|f| f.line_height_at_least && f.line_height_pt.is_some()) {
        " class=\"writer-minimum-line\""
    } else {
        ""
    }
}

impl WriterState<'_> {
    fn block(&mut self, block: &Block, out: &mut String, depth: usize) -> Result<(), String> {
        budget(&mut self.remaining)?;
        if depth > 32 {
            return Err("PDF document nesting exceeds the layout limit".into());
        }
        match block {
            Block::Paragraph(paragraph) => {
                let format = self.formats.get(&paragraph.id).copied();
                let line_box = office_line_box(
                    &paragraph.runs,
                    paragraph.spacing,
                    format,
                    self.font_metrics,
                );
                let mut style = self.paragraph_style(
                    format,
                    paragraph.alignment,
                    paragraph.spacing,
                    paragraph.indent_level,
                );
                style.push_str(&self.font_style(line_box.as_ref()));
                let class = minimum_line_class(format);
                out.push_str(&format!("<p{class} style=\"{style}\">"));
                self.numbering(format, out, line_box.as_ref(), None, None);
                runs(&paragraph.runs, out, line_box.as_ref(), self.font_metrics);
                if paragraph.runs.iter().all(|run| run.text.is_empty()) {
                    out.push_str("&#160;");
                }
                out.push_str("</p>");
            }
            Block::Heading(heading) => {
                let level = heading.level.clamp(1, 6);
                let format = self.formats.get(&heading.id).copied();
                let line_box = office_line_box(
                    &heading.runs,
                    Spacing {
                        before: 12.,
                        after: 8.,
                        line_height: 1.2,
                    },
                    format,
                    self.font_metrics,
                );
                let mut style = self.paragraph_style(
                    format,
                    Alignment::Left,
                    Spacing {
                        before: 12.,
                        after: 8.,
                        line_height: 1.2,
                    },
                    0,
                );
                style.push_str(&self.font_style(line_box.as_ref()));
                let class = minimum_line_class(format);
                out.push_str(&format!("<h{level}{class} style=\"{style}\">"));
                self.numbering(format, out, line_box.as_ref(), None, None);
                runs(&heading.runs, out, line_box.as_ref(), self.font_metrics);
                out.push_str(&format!("</h{level}>"));
            }
            Block::List(list) => self.list(&list.items, list.ordered, out, depth + 1)?,
            Block::Table(table) => {
                if table
                    .rows
                    .iter()
                    .any(|row| row.cells.iter().any(|cell| cell.layout.is_some()))
                {
                    out.push_str("<table style=\"margin:0\">");
                } else {
                    out.push_str("<table>");
                }
                if !table.column_widths.is_empty() {
                    let total = table
                        .column_widths
                        .iter()
                        .filter(|n| n.is_finite() && **n > 0.)
                        .sum::<f32>();
                    if total > 0. {
                        out.push_str("<colgroup>");
                        for width in &table.column_widths {
                            out.push_str(&format!(
                                "<col style=\"width:{}%\">",
                                finite(f64::from(*width / total) * 100., 1., 0.1, 100.)
                            ));
                        }
                        out.push_str("</colgroup>");
                    }
                }
                out.push_str("<tbody>");
                for row in &table.rows {
                    out.push_str("<tr>");
                    for cell in &row.cells {
                        budget(&mut self.remaining)?;
                        out.push_str(&format!(
                            "<td colspan=\"{}\" rowspan=\"{}\" style=\"background:{};{}\">",
                            cell.col_span.clamp(1, 1000),
                            cell.row_span.clamp(1, 1000),
                            color(cell.background_color.as_deref(), "transparent"),
                            cell.layout.as_ref().map(|layout| {
                                let align=match layout.vertical_alignment {syntra_doc_engine::writer::CellVerticalAlignment::Top=>"top",syntra_doc_engine::writer::CellVerticalAlignment::Center=>"middle",syntra_doc_engine::writer::CellVerticalAlignment::Bottom=>"bottom"};
                                let pad=layout.padding.map(|v|finite(v.unwrap_or(0.) as f64,0.,0.,1000.));
                                format!("vertical-align:{align};padding:{}pt {}pt {}pt {}pt",pad[0],pad[1],pad[2],pad[3])
                            }).unwrap_or_default()
                        ));
                        for block in &cell.blocks {
                            self.block(block, out, depth + 1)?;
                        }
                        out.push_str("</td>");
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table>");
            }
            Block::Image(picture) => {
                let source = image(&picture.url)?;
                let width = picture
                    .width
                    .map(|width| {
                        format!("width:{}px;", finite(f64::from(width), 100., 1., 20_000.))
                    })
                    .unwrap_or_default();
                let height = picture
                    .height
                    .map(|height| {
                        format!("height:{}px;", finite(f64::from(height), 100., 1., 20_000.))
                    })
                    .unwrap_or_default();
                out.push_str(&format!("<figure style=\"text-align:{}\"><img src=\"{source}\" alt=\"{}\" style=\"{width}{height}\"></figure>",alignment(picture.alignment),escape(&picture.alt_text)));
            }
            Block::CodeBlock(code) => out.push_str(&format!("<pre>{}</pre>", escape(&code.code))),
            Block::Blockquote(quote) => {
                out.push_str("<blockquote>");
                for block in &quote.blocks {
                    self.block(block, out, depth + 1)?;
                }
                out.push_str("</blockquote>");
            }
            Block::HorizontalRule => out.push_str("<hr>"),
            Block::PageBreak => out.push_str("<div class=\"writer-page-break\"></div>"),
        }
        if out.len() > MAX_HTML_BYTES {
            return Err("PDF print layout exceeds the rendering budget".into());
        }
        Ok(())
    }

    fn paragraph_style(
        &self,
        format: Option<&ParagraphFormat>,
        align: Alignment,
        spacing: Spacing,
        indent: u8,
    ) -> String {
        let spacing = format.and_then(|format| format.spacing).unwrap_or(spacing);
        let left = format
            .and_then(|f| f.left_indent_pt)
            .map(f64::from)
            .unwrap_or(f64::from(indent) * 18.);
        let right = format
            .and_then(|f| f.right_indent_pt)
            .map(f64::from)
            .unwrap_or(0.);
        let line = format
            .and_then(|f| f.line_height_pt)
            .map(|value| {
                let points = finite(f64::from(value), 14., 1., 1000.);
                if format.is_some_and(|f| f.line_height_at_least) {
                    format!("max(1em,{points}pt)")
                } else {
                    format!("{points}pt")
                }
            })
            .unwrap_or_else(|| finite(f64::from(spacing.line_height), 1.15, 0.5, 10.).to_string());
        let first = if let Some(value) = format.and_then(|f| f.first_line_indent_chars) {
            format!("{}em", finite(f64::from(value), 0., -100., 100.))
        } else {
            format!(
                "{}pt",
                finite(
                    format
                        .and_then(|f| f.first_line_indent_pt)
                        .map(f64::from)
                        .unwrap_or(0.),
                    0.,
                    -1000.,
                    1000.
                )
            )
        };
        format!(
            "margin:{}pt {}pt {}pt {}pt;line-height:{line};text-align:{};text-indent:{first}",
            finite(f64::from(spacing.before), 0., 0., 1000.),
            finite(right, 0., -1000., 1000.),
            finite(f64::from(spacing.after), 8., 0., 1000.),
            finite(left, 0., -1000., 1000.),
            alignment(format.and_then(|f| f.alignment).unwrap_or(align))
        )
    }

    fn font_style(&self, line: Option<&FontLineBox>) -> String {
        line.map(|line| {
            format!(
                ";font-family:{};font-size:{}pt;line-height:{}pt",
                font(&line.font_family),
                line.font_size,
                line.line_height_pt
            )
        })
        .unwrap_or_default()
    }

    fn numbering(
        &self,
        format: Option<&ParagraphFormat>,
        out: &mut String,
        line: Option<&FontLineBox>,
        family: Option<&str>,
        size: Option<f32>,
    ) {
        if let Some(label) = format.and_then(|format| format.numbering_label.as_deref()) {
            let hanging = format
                .and_then(|format| format.first_line_indent_pt)
                .filter(|indent| *indent < 0.);
            let mut style = hanging
                .map(|indent| {
                    format!(
                        "display:inline-block;text-indent:0;width:{}pt;",
                        finite(f64::from(-indent), 18., 0., 1000.)
                    )
                })
                .unwrap_or_default();
            let family = family.or_else(|| line.map(|line| line.font_family.as_str()));
            let size = size.or_else(|| line.map(|line| line.font_size));
            if let Some(family) = family {
                style.push_str(&format!("font-family:{};", font(family)));
            }
            if let Some(size) = size {
                style.push_str(&format!(
                    "font-size:{}pt;",
                    finite(f64::from(size), 12., 1., 1000.)
                ));
            }
            if let Some(line) = line {
                let run = TextRun {
                    text: label.into(),
                    font_family: family.map(str::to_owned),
                    font_size: size,
                    ..Default::default()
                };
                if let Some(offset) = office_run_offset(&run, line, self.font_metrics) {
                    style.push_str(&format!(
                        "vertical-align:top;position:relative;top:{offset}pt;line-height:inherit;"
                    ));
                }
            }
            out.push_str(&format!(
                "<span style=\"{style}\">{} </span>",
                escape(label)
            ));
        }
    }

    fn list(
        &mut self,
        items: &[ListItem],
        ordered: bool,
        out: &mut String,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("PDF list nesting exceeds the layout limit".into());
        }
        let tag = if ordered { "ol" } else { "ul" };
        let imported = items.iter().any(|item| item.layout.is_some());
        out.push_str(&format!(
            "<{tag}{}>",
            if imported {
                " style=\"padding-left:0;margin:0\""
            } else {
                ""
            }
        ));
        for (index, item) in items.iter().enumerate() {
            budget(&mut self.remaining)?;
            if let Some(layout) = &item.layout {
                let mut spacing = layout.spacing;
                if layout.format.contextual_spacing == Some(true)
                    && items
                        .get(index + 1)
                        .and_then(|item| item.layout.as_ref())
                        .is_some_and(|layout| layout.format.contextual_spacing == Some(true))
                {
                    spacing.after = 0.;
                }
                let mut style =
                    self.paragraph_style(Some(&layout.format), Alignment::Left, spacing, 0);
                let mut line_runs = item.runs.clone();
                if let Some(family) = &layout.label_font_family {
                    line_runs.push(TextRun {
                        text: layout.format.numbering_label.clone().unwrap_or_default(),
                        font_family: Some(family.clone()),
                        font_size: layout.label_font_size,
                        ..Default::default()
                    });
                }
                let line_box =
                    office_line_box(&line_runs, spacing, Some(&layout.format), self.font_metrics);
                style.push_str(&self.font_style(line_box.as_ref()));
                let size = item
                    .runs
                    .first()
                    .and_then(|run| run.font_size)
                    .map(|size| format!("font-size:{}pt;", finite(f64::from(size), 12., 1., 1000.)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "<li style=\"list-style:none\"><p style=\"{size}{style}\">"
                ));
                self.numbering(
                    Some(&layout.format),
                    out,
                    line_box.as_ref(),
                    layout.label_font_family.as_deref(),
                    layout.label_font_size,
                );
                runs(&item.runs, out, line_box.as_ref(), self.font_metrics);
                out.push_str("</p>");
            } else {
                out.push_str("<li>");
                runs(&item.runs, out, None, self.font_metrics);
            }
            if !item.children.is_empty() {
                self.list(&item.children, ordered, out, depth + 1)?;
            }
            out.push_str("</li>");
        }
        out.push_str(&format!("</{tag}>"));
        Ok(())
    }
}

fn runs(
    runs: &[TextRun],
    out: &mut String,
    line: Option<&FontLineBox>,
    metrics: &std::collections::BTreeMap<String, FontMetrics>,
) {
    for run in runs {
        let family = run
            .font_family
            .as_deref()
            .map(font)
            .map(|family| format!("font-family:{family};"))
            .unwrap_or_default();
        let size = run
            .font_size
            .map(|size| format!("font-size:{}pt;", finite(f64::from(size), 11., 1., 1000.)))
            .unwrap_or_default();
        let decoration = match (run.underline, run.strikethrough) {
            (true, true) => "underline line-through",
            (true, false) => "underline",
            (false, true) => "line-through",
            _ => "none",
        };
        let baseline = if !run.superscript && !run.subscript {
            line.and_then(|line| office_run_offset(run, line, metrics))
                .map(|offset| {
                    format!(
                        ";vertical-align:top;position:relative;top:{offset}pt;line-height:inherit"
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&format!("<span style=\"{family}{size}color:{};background:{};font-weight:{};font-style:{};text-decoration:{decoration};vertical-align:{}{baseline}\">",color(run.color.as_deref(),"inherit"),color(run.background_color.as_deref(),"transparent"),if run.bold {"bold"} else {"normal"},if run.italic {"italic"} else {"normal"},if run.superscript {"super"} else if run.subscript {"sub"} else {"baseline"}));
        let link = run.link.as_deref().filter(|link| {
            link.starts_with("https://")
                || link.starts_with("http://")
                || link.starts_with("mailto:")
        });
        if let Some(link) = link {
            out.push_str(&format!("<a href=\"{}\">", escape(link)));
        }
        out.push_str(&escape(&run.text));
        if link.is_some() {
            out.push_str("</a>");
        }
        out.push_str("</span>");
    }
}
