//! Generate independently inspectable HTML print fixtures for all four editors.
//! Usage: cargo run -p syntra-export --example pdf_print_fixtures -- OUT_DIR [CJK_TTF]
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{io::Cursor, path::PathBuf};
use syntra_doc_engine::{calc as c, slides as s, whiteboard as b, writer as w};
use uuid::Uuid;

fn paragraph(text: &str) -> w::Block {
    w::Block::Paragraph(w::Paragraph {
        id: Uuid::new_v4(),
        runs: vec![w::TextRun::plain(text)],
        alignment: w::Alignment::Left,
        indent_level: 0,
        spacing: w::Spacing::default(),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = PathBuf::from(args.next().ok_or("Expected output directory")?);
    std::fs::create_dir_all(&output)?;
    let font=args.next().map(std::fs::read).transpose()?.map(|data|format!("<style>@font-face{{font-family:'Office CJK';src:url(data:font/ttf;base64,{}) format('truetype');font-weight:100 900;font-style:normal}}</style>",STANDARD.encode(data)));
    let pixels = image::RgbImage::from_fn(300, 150, |x, y| {
        if (x / 30 + y / 30) % 2 == 0 {
            image::Rgb([37, 99, 235])
        } else {
            image::Rgb([234, 88, 12])
        }
    });
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels).write_to(&mut encoded, image::ImageFormat::Png)?;
    let source = format!(
        "data:image/png;base64,{}",
        STANDARD.encode(encoded.into_inner())
    );

    let mut writer = w::WriterDocument::new("PDF 中文排版验证".into());
    writer.professional.header = "syntra · 文档页眉".into();
    writer.professional.footer = "保留文字、图片与表格".into();
    writer.professional.page_numbers = true;
    writer.body.push(w::Block::Heading(w::Heading {
        id: Uuid::new_v4(),
        level: 1,
        runs: vec![w::TextRun {
            text: "中文 PDF 导出验证".into(),
            bold: true,
            font_size: Some(20.),
            color: Some("#2563eb".into()),
            ..Default::default()
        }],
    }));
    writer.body.push(paragraph("这是可搜索、可选择的中文文本。This text remains selectable, rather than a screenshot. The image below is embedded locally."));
    writer.body.push(w::Block::Image(w::ImageBlock {
        id: Uuid::new_v4(),
        url: source.clone(),
        alt_text: "Blue orange test image".into(),
        width: Some(300.),
        height: Some(150.),
        alignment: w::Alignment::Center,
    }));
    writer.body.push(w::Block::Table(w::Table {
        id: Uuid::new_v4(),
        column_widths: vec![1., 2.],
        rows: vec![w::TableRow {
            cells: vec![
                w::TableCell {
                    blocks: vec![paragraph("项目")],
                    col_span: 1,
                    row_span: 1,
                    background_color: Some("#dbeafe".into()),
                },
                w::TableCell {
                    blocks: vec![paragraph("说明：中文、English、123")],
                    col_span: 1,
                    row_span: 1,
                    background_color: None,
                },
            ],
        }],
    }));
    writer.body.push(w::Block::PageBreak);
    writer.body.push(w::Block::Heading(w::Heading {
        id: Uuid::new_v4(),
        level: 2,
        runs: vec![w::TextRun::plain("第二页 / Page two")],
    }));
    for index in 1..=65 {
        writer.body.push(paragraph(&format!("多页段落 {index}：正文按页边距自动换行，并保留段落间距。Exported pages retain searchable Chinese and English text.")));
    }

    let mut workbook = c::Workbook::new("PDF 工作簿验证".into());
    let sheet = &mut workbook.sheets[0];
    sheet.name = "销售报表".into();
    for (column, text) in ["项目", "单价", "数量", "合计"].into_iter().enumerate() {
        sheet.set_cell(0, column as u32, c::CellValue::Text(text.into()));
    }
    for row in 1..=50 {
        sheet.set_cell(row, 0, c::CellValue::Text(format!("产品 {row}")));
        sheet.set_cell(row, 1, c::CellValue::Number(12.5));
        sheet.set_cell(row, 2, c::CellValue::Number(2.));
        sheet.set_formula(row, 3, format!("=B{}*C{}", row + 1, row + 1));
        sheet
            .cells
            .get_mut(&c::CellRef::new(row, 3))
            .unwrap()
            .format
            .number_format = Some("¥0.00".into());
    }
    sheet.charts.push(c::SheetChart {
        id: Uuid::new_v4(),
        title: "Sales / 销售图".into(),
        chart_type: c::SheetChartType::Bar,
        data_start: c::CellRef::new(0, 0),
        data_end: c::CellRef::new(4, 1),
        anchor_start: c::CellRef::new(1, 6),
        anchor_end: c::CellRef::new(20, 12),
        cached_data: None,
        series_sources: Vec::new(),
    });

    let mut presentation = s::Presentation::new("PDF 演示文稿验证".into());
    presentation.metadata.slide_width = 1280.;
    presentation.metadata.slide_height = 720.;
    presentation.slides[0].background = s::Background::Solid("#0f172a".into());
    presentation.slides[0].add_text_box(60., 50., 1000., 100., "第一页 · First slide");
    if let s::SlideElement::TextBox(text) = &mut presentation.slides[0].elements[0] {
        text.content[0].color = Some("#ffffff".into());
        text.content[0].font_size = Some(36.);
    }
    presentation.slides[0]
        .elements
        .push(s::SlideElement::Image(s::SlideImage {
            id: Uuid::new_v4(),
            x: 80.,
            y: 220.,
            width: 600.,
            height: 300.,
            rotation: 0.,
            url: source.clone(),
            alt_text: "Embedded checker image".into(),
            crop: None,
        }));
    let second = presentation.add_slide(s::SlideLayout::Blank);
    second.background = s::Background::Gradient("#dbeafe".into(), "#ffedd5".into(), 45.);
    second.add_text_box(60., 50., 1000., 100., "第二页 · 图形与图表");
    second.elements.push(s::SlideElement::Shape(s::Shape {
        id: Uuid::new_v4(),
        x: 80.,
        y: 220.,
        width: 250.,
        height: 250.,
        rotation: 15.,
        shape_type: s::ShapeType::Star,
        fill_color: Some("#ea580c".into()),
        border: None,
        text: None,
    }));
    second.elements.push(s::SlideElement::Chart(s::SlideChart {
        id: Uuid::new_v4(),
        x: 430.,
        y: 200.,
        width: 650.,
        height: 350.,
        chart_type: s::ChartType::Line,
        data: s::ChartData {
            labels: vec!["一".into(), "二".into(), "三".into()],
            datasets: vec![s::Dataset {
                name: "销售".into(),
                values: vec![4., 2., 7.],
                color: Some("#2563eb".into()),
            }],
        },
    }));

    let mut board = b::Whiteboard::new("PDF 白板验证".into());
    board.canvas_width = 1280.;
    board.canvas_height = 720.;
    board.background_color = "#f8fafc".into();
    board.layers[0]
        .elements
        .push(b::DrawElement::Rectangle(b::DrawRect {
            id: Uuid::new_v4(),
            x: 80.,
            y: 100.,
            width: 400.,
            height: 260.,
            rotation: 10.,
            fill_color: Some("#dbeafe".into()),
            stroke_color: "#2563eb".into(),
            stroke_width: 3.,
            corner_radius: 16.,
        }));
    board.layers[0]
        .elements
        .push(b::DrawElement::Text(b::DrawText {
            id: Uuid::new_v4(),
            x: 80.,
            y: 30.,
            text: "白板中文与矢量图形".into(),
            font_size: 32.,
            font_family: "Arial".into(),
            color: "#0f172a".into(),
            bold: true,
            italic: false,
        }));
    board.layers[0]
        .elements
        .push(b::DrawElement::Image(b::DrawImage {
            id: Uuid::new_v4(),
            x: 650.,
            y: 100.,
            width: 450.,
            height: 225.,
            rotation: 0.,
            url: source,
        }));
    board.layers[0]
        .elements
        .push(b::DrawElement::Line(b::DrawLine {
            id: Uuid::new_v4(),
            start: b::Point::new(450., 450.),
            end: b::Point::new(800., 450.),
            stroke_color: "#ea580c".into(),
            stroke_width: 6.,
            arrow_start: false,
            arrow_end: true,
        }));

    for (kind, model) in [
        ("writer", serde_json::to_value(writer)?),
        ("calc", serde_json::to_value(workbook)?),
        ("slides", serde_json::to_value(presentation)?),
        ("whiteboard", serde_json::to_value(board)?),
    ] {
        let mut printed = syntra_export::pdf::document_to_print_html(&model, kind)?;
        if let Some(font) = &font {
            printed.html = printed
                .html
                .replacen("</head>", &format!("{font}</head>"), 1);
        }
        std::fs::write(output.join(format!("{kind}.html")), printed.html)?;
        std::fs::write(
            output.join(format!("{kind}.json")),
            serde_json::to_vec_pretty(&model)?,
        )?;
        println!("{kind}: {} × {} mm", printed.width_mm, printed.height_mm);
    }
    Ok(())
}
