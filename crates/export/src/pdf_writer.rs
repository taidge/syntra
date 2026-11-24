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
    )?;
    css.push_str("body{line-height:1.15}.writer-section{page:writer_base}p{margin:0 0 8pt;min-height:1em;white-space:pre-wrap;overflow-wrap:anywhere;orphans:2;widows:2}h1,h2,h3,h4,h5,h6{margin:12pt 0 8pt;break-after:avoid;white-space:pre-wrap}h1{font-size:24pt}h2{font-size:20pt}h3{font-size:18pt}h4{font-size:16pt}h5{font-size:14pt}h6{font-size:12pt}table{border-collapse:collapse;width:100%;table-layout:fixed;margin:8pt 0}td{border:1px solid #9ca3af;padding:5pt;vertical-align:top;overflow-wrap:anywhere}td p:last-child{margin-bottom:0}tr{break-inside:avoid}pre{font-family:monospace,'Office CJK',sans-serif;white-space:pre-wrap;overflow-wrap:anywhere;background:#f3f4f6;padding:8pt;font-size:10pt}blockquote{border-left:3pt solid #9ca3af;margin:8pt 0;padding-left:12pt}figure{margin:8pt 0;break-inside:avoid}figure img{max-width:100%;object-fit:contain}li{white-space:pre-wrap}li p{margin:0}.writer-page-break{break-after:page;height:0}a{color:inherit}ul,ol{padding-left:24pt}");
    if document.professional.different_first_page {
        css.push_str("@page:first{@top-center{content:none}@bottom-center{content:none}}");
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
    Ok((
        width,
        height,
        format!(
            "@page {name}{{size:{width}mm {height}mm;margin:{}mm {}mm {}mm {}mm;@top-center{{content:'{}';font:9pt Arial,'Office CJK',sans-serif;color:#475569}}@bottom-center{{content:{footer};font:9pt Arial,'Office CJK',sans-serif;color:#475569}}}}",
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
}

fn alignment(value: Alignment) -> &'static str {
    match value {
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
        Alignment::Justify => "justify",
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
                let style = self.paragraph_style(
                    format,
                    paragraph.alignment,
                    paragraph.spacing,
                    paragraph.indent_level,
                );
                out.push_str(&format!("<p style=\"{style}\">"));
                self.numbering(format, out);
                runs(&paragraph.runs, out);
                if paragraph.runs.iter().all(|run| run.text.is_empty()) {
                    out.push_str("&#160;");
                }
                out.push_str("</p>");
            }
            Block::Heading(heading) => {
                let level = heading.level.clamp(1, 6);
                let format = self.formats.get(&heading.id).copied();
                let style = self.paragraph_style(
                    format,
                    Alignment::Left,
                    Spacing {
                        before: 12.,
                        after: 8.,
                        line_height: 1.2,
                    },
                    0,
                );
                out.push_str(&format!("<h{level} style=\"{style}\">"));
                self.numbering(format, out);
                runs(&heading.runs, out);
                out.push_str(&format!("</h{level}>"));
            }
            Block::List(list) => self.list(&list.items, list.ordered, out, depth + 1)?,
            Block::Table(table) => {
                out.push_str("<table>");
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
                            "<td colspan=\"{}\" rowspan=\"{}\" style=\"background:{}\">",
                            cell.col_span.clamp(1, 1000),
                            cell.row_span.clamp(1, 1000),
                            color(cell.background_color.as_deref(), "transparent")
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
            .map(|value| format!("{}pt", finite(f64::from(value), 14., 1., 1000.)))
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

    fn numbering(&self, format: Option<&ParagraphFormat>, out: &mut String) {
        if let Some(label) = format.and_then(|format| format.numbering_label.as_deref()) {
            out.push_str(&format!("<span>{} </span>", escape(label)));
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
        out.push_str(&format!("<{tag}>"));
        for item in items {
            budget(&mut self.remaining)?;
            out.push_str("<li>");
            runs(&item.runs, out);
            if !item.children.is_empty() {
                self.list(&item.children, ordered, out, depth + 1)?;
            }
            out.push_str("</li>");
        }
        out.push_str(&format!("</{tag}>"));
        Ok(())
    }
}

fn runs(runs: &[TextRun], out: &mut String) {
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
        out.push_str(&format!("<span style=\"{family}{size}color:{};background:{};font-weight:{};font-style:{};text-decoration:{decoration};vertical-align:{}\">",color(run.color.as_deref(),"inherit"),color(run.background_color.as_deref(),"transparent"),if run.bold {"bold"} else {"normal"},if run.italic {"italic"} else {"normal"},if run.superscript {"super"} else if run.subscript {"sub"} else {"baseline"}));
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
