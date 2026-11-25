use super::*;

const CANVAS_CSS: &str = "@page{margin:0}.canvas-page{position:relative;break-after:page;break-inside:avoid;overflow:hidden}.canvas-page:last-child{break-after:auto}.canvas-page svg{display:block;overflow:visible;font-family:Arial,'Office CJK',sans-serif}.canvas-item{position:absolute;transform-origin:center}.canvas-text{box-sizing:border-box;display:flex;flex-direction:column;overflow:visible}.canvas-text p{flex:0 0 auto;min-width:0;margin:0;white-space:inherit}";

fn canvas_dimensions(
    width: &Value,
    height: &Value,
    default_width: f64,
    default_height: f64,
) -> Result<(f64, f64), String> {
    let width = width.as_f64().unwrap_or(default_width);
    let height = height.as_f64().unwrap_or(default_height);
    if !width.is_finite()
        || !height.is_finite()
        || !(16.0..=10000.).contains(&width)
        || !(16.0..=10000.).contains(&height)
        || width * height > 64_000_000.
    {
        return Err("PDF canvas dimensions exceed the finite rendering budget".into());
    }
    Ok((width, height))
}

pub(super) fn slides(content: &Value) -> Result<PrintDocument, String> {
    let (width, height) = canvas_dimensions(
        &content["metadata"]["slide_width"],
        &content["metadata"]["slide_height"],
        1920.,
        1080.,
    )?;
    let slides = content["slides"]
        .as_array()
        .ok_or("Presentation PDF has no slide array")?;
    if slides.len() > 500 {
        return Err("PDF presentation exceeds the 500-slide limit".into());
    }
    let mut body = String::new();
    let mut remaining = MAX_OBJECTS;
    for (index, slide) in slides.iter().enumerate() {
        let background = &slide["background"];
        let background_css = if let Some(stops) = background["Gradient"].as_array() {
            format!(
                "linear-gradient({}deg,{}, {})",
                number(stops.get(2).unwrap_or(&Value::Null), 0.),
                color(stops.first().and_then(Value::as_str), "white"),
                color(stops.get(1).and_then(Value::as_str), "black")
            )
        } else {
            color(
                background["Solid"]
                    .as_str()
                    .or_else(|| slide["bg_color"].as_str()),
                "white",
            )
        };
        body.push_str(&format!("<section class=\"canvas-page\" aria-label=\"Slide {}\" style=\"width:{width}px;height:{height}px;background:{background_css}\">",index+1));
        if let Some(source) = background["Image"].as_str() {
            body.push_str(&format!("<img src=\"{}\" alt=\"Slide background\" style=\"position:absolute;width:100%;height:100%;object-fit:fill\">",image(source)?));
        }
        let elements = slide["elements"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for element in elements {
            slide_element(element, &mut body, &mut remaining, 0)?;
        }
        if elements.is_empty() {
            body.push_str(&format!("<h1 style=\"position:absolute;left:6%;top:8%;font-size:36pt\">{}</h1><p style=\"position:absolute;left:6%;top:35%;font-size:22pt;white-space:pre-wrap\">{}</p>",escape(slide["title"].as_str().unwrap_or_default()),escape(slide["body"].as_str().unwrap_or_default())));
        }
        body.push_str("</section>");
        if body.len() > MAX_HTML_BYTES {
            return Err("PDF print layout exceeds the rendering budget".into());
        }
    }
    if slides.is_empty() {
        body.push_str(&format!(
            "<section class=\"canvas-page\" style=\"width:{width}px;height:{height}px\"></section>"
        ));
    }
    let (width_mm, height_mm) = (width * 25.4 / 96., height * 25.4 / 96.);
    let css = format!("@page{{size:{width_mm}mm {height_mm}mm}}{CANVAS_CSS}");
    Ok(html(
        content["title"].as_str().unwrap_or("Presentation"),
        &css,
        &body,
        width_mm,
        height_mm,
    ))
}

fn slide_element(
    element: &Value,
    out: &mut String,
    remaining: &mut usize,
    depth: usize,
) -> Result<(), String> {
    budget(remaining)?;
    if depth > 32 {
        return Err("PDF slide group nesting exceeds the rendering limit".into());
    }
    let num = |key, default| number(&element[key], default);
    if element["type"] == "Group" {
        for element in element["elements"].as_array().into_iter().flatten() {
            slide_element(element, out, remaining, depth + 1)?;
        }
        return Ok(());
    }
    if element["type"] == "Table" {
        for cell in syntra_doc_engine::slides::table_render_cells(element) {
            slide_element(&cell, out, remaining, depth + 1)?;
        }
        return Ok(());
    }
    let (x, y, width, height, rotation) = (
        num("x", 0.),
        num("y", 0.),
        finite(num("width", 500.), 500., 0., 10000.),
        finite(num("height", 300.), 300., 0., 10000.),
        num("rotation", 0.).rem_euclid(360.),
    );
    let placement = format!(
        "left:{x}px;top:{y}px;width:{width}px;height:{height}px;transform:rotate({rotation}deg);"
    );
    match element["type"].as_str().unwrap_or_default() {
        "Image" => {
            let source=image(element["url"].as_str().unwrap_or_default())?;
            let crop=|key|finite(number(&element["crop"][key],0.),0.,0.,0.99);
            let (left,right,top,bottom)=(crop("left"),crop("right"),crop("top"),crop("bottom"));
            let image_width=width/(1.-left-right).max(0.01);
            let image_height=height/(1.-top-bottom).max(0.01);
            out.push_str(&format!("<div class=\"canvas-item\" style=\"{placement}overflow:hidden\"><img src=\"{source}\" alt=\"{}\" style=\"position:absolute;max-width:none;width:{image_width}px;height:{image_height}px;left:{}px;top:{}px\"></div>",escape(element["alt_text"].as_str().unwrap_or_default()),-image_width*left,-image_height*top));
        }
        "TextBox"|"Shape" => {
            let text_box=element["type"]=="TextBox";
            let fill=color(element[if text_box {"background_color"} else {"fill_color"}].as_str(),"none");
            let stroke=color(element["border"]["color"].as_str(),"none");
            let stroke_width=finite(number(&element["border"]["width"],1.),1.,0.,1000.);
            out.push_str(&format!("<div class=\"canvas-item\" style=\"{placement}\"><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100%\" height=\"100%\" viewBox=\"0 0 {width} {height}\">{}{}</svg>",shape_svg(element["shape_type"].as_str().unwrap_or("Rectangle"),width,height,&fill,&stroke,stroke_width),syntra_doc_engine::slides::table_borders_svg(element,0.,0.,width,height)));
            if let Some(runs)=element[if text_box {"content"} else {"text"}].as_array() {text_box_html(runs,&element["text_layout"],num("padding",0.),out);}
            out.push_str("</div>");
        }
        "Chart"=>out.push_str(&format!("<div class=\"canvas-item\" style=\"{placement}\"><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100%\" height=\"100%\" viewBox=\"0 0 {width} {height}\">{}</svg></div>",chart_svg(element,0.,0.,width,height)?)),
        "Video"=>out.push_str(&format!("<div class=\"canvas-item\" style=\"{placement}background:#111827;color:white;display:flex;align-items:center;justify-content:center\">Video</div>")),
        other=>return Err(format!("PDF slide contains unsupported object '{other}'")),
    }
    Ok(())
}

fn shape_svg(
    kind: &str,
    width: f64,
    height: f64,
    fill: &str,
    stroke: &str,
    stroke_width: f64,
) -> String {
    let attrs = format!("fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\"");
    match kind {
        "Ellipse" | "Circle" => format!(
            "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" {attrs}/>",
            width / 2.,
            height / 2.,
            width / 2.,
            height / 2.
        ),
        "Line" | "CurvedLine" => format!("<path d=\"M0,0 L{width},{height}\" {attrs}/>"),
        "Triangle" | "Diamond" | "Pentagon" | "Hexagon" | "Star" | "Arrow" => {
            let points = match kind {
                "Triangle" => vec![(0.5, 0.), (1., 1.), (0., 1.)],
                "Diamond" => vec![(0.5, 0.), (1., 0.5), (0.5, 1.), (0., 0.5)],
                "Arrow" => syntra_doc_engine::slides::default_right_arrow_points(width, height),
                _ => {
                    let count = if kind == "Pentagon" {
                        5
                    } else if kind == "Hexagon" {
                        6
                    } else {
                        10
                    };
                    (0..count)
                        .map(|index| {
                            let angle = std::f64::consts::TAU * f64::from(index) / f64::from(count)
                                - std::f64::consts::FRAC_PI_2;
                            let radius = if kind == "Star" && index % 2 == 1 {
                                0.22
                            } else {
                                0.5
                            };
                            (0.5 + radius * angle.cos(), 0.5 + radius * angle.sin())
                        })
                        .collect()
                }
            };
            format!(
                "<polygon points=\"{}\" {attrs}/>",
                points
                    .iter()
                    .map(|(x, y)| format!("{},{}", x * width, y * height))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
        _ => format!(
            "<rect width=\"{width}\" height=\"{height}\" rx=\"{}\" {attrs}/>",
            if kind == "RoundedRectangle" {
                width.min(height) * 0.12
            } else {
                0.
            }
        ),
    }
}

fn text_box_html(runs: &[Value], layout: &Value, padding: f64, out: &mut String) {
    let mut paragraph_sizes = std::collections::BTreeMap::<Option<u64>, (f64, String)>::new();
    for run in runs {
        let size = finite(number(&run["font_size"], 28.), 28., 1., 1000.);
        let value = paragraph_sizes
            .entry(run["paragraph"]["index"].as_u64())
            .or_default();
        if size > value.0 {
            *value = (size, font(run["font_family"].as_str().unwrap_or("Arial")));
        }
    }
    let inset = |key| finite(number(&layout[key], padding), 0., 0., 10000.);
    let vertical = match layout["vertical_alignment"].as_str() {
        Some("center") => "center",
        Some("bottom") => "flex-end",
        _ => "flex-start",
    };
    let wrap = if layout["wrap"] == false {
        "pre"
    } else {
        "pre-wrap"
    };
    out.push_str(&format!("<div class=\"canvas-text\" style=\"position:absolute;inset:0;padding:{}px {}px {}px {}px;justify-content:{vertical};white-space:{wrap};overflow-wrap:anywhere\">",inset("inset_top"),inset("inset_right"),inset("inset_bottom"),inset("inset_left")));
    let mut paragraph_index = None;
    let mut opened = false;
    for run in runs {
        if run["paragraph_break"] == true {
            continue;
        }
        let index = run["paragraph"]["index"].as_u64();
        if !opened || (index.is_some() && index != paragraph_index) {
            if opened {
                out.push_str("</p>");
            }
            let style = &run["paragraph"];
            let alignment = match style["alignment"].as_str() {
                Some("center") => "center",
                Some("right") => "right",
                Some("justify") => "justify",
                _ => "left",
            };
            let line = style["line_height_points"]
                .as_f64()
                .filter(|value| value.is_finite() && *value > 0.)
                .map(|value| format!("{}pt", finite(value, 12., 1., 1000.)))
                .unwrap_or_else(|| {
                    (finite(number(&style["line_height"], 1.0), 1.0, 0.1, 10.) * 1.2).to_string()
                });
            let default_font = font("Arial");
            let paragraph_size = paragraph_sizes.get(&index).map(|v| v.0).unwrap_or(28.);
            let paragraph_font = paragraph_sizes
                .get(&index)
                .map(|v| v.1.as_str())
                .unwrap_or(&default_font);
            let configured_line = style["line_height_points"]
                .as_f64()
                .filter(|n| n.is_finite() && *n > 0.)
                .unwrap_or(
                    paragraph_size * finite(number(&style["line_height"], 1.), 1., 0.1, 10.) * 1.2,
                );
            let baseline_shift = (configured_line - paragraph_size * 1.2) / 2.;
            out.push_str(&format!(
                "<p style=\"position:relative;top:{baseline_shift}pt;font-family:{paragraph_font};font-size:{}pt;text-align:{alignment};line-height:{line};margin:{}pt 0 {}pt\">",
                paragraph_size,
                finite(number(&style["space_before_points"], 0.), 0., 0., 1000.),
                finite(number(&style["space_after_points"], 0.), 0., 0., 1000.)
            ));
            opened = true;
        }
        paragraph_index = index;
        let decoration = match (run["underline"] == true, run["strikethrough"] == true) {
            (true, true) => "underline line-through",
            (true, false) => "underline",
            (false, true) => "line-through",
            _ => "none",
        };
        out.push_str(&format!("<span style=\"font-family:{};font-size:{}pt;font-weight:{};font-style:{};color:{};text-decoration:{decoration}\">{}</span>",font(run["font_family"].as_str().unwrap_or("Arial")),finite(number(&run["font_size"],28.),28.,1.,1000.),if run["bold"]==true {"bold"} else {"normal"},if run["italic"]==true {"italic"} else {"normal"},color(run["color"].as_str(),"#111827"),escape(run["text"].as_str().unwrap_or_default())));
    }
    if opened {
        out.push_str("</p>");
    }
    out.push_str("</div>");
}

pub(super) fn chart_svg(
    element: &Value,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<String, String> {
    let datasets = element["data"]["datasets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if datasets.len() > 100
        || datasets
            .iter()
            .map(|s| s["values"].as_array().map_or(0, Vec::len))
            .sum::<usize>()
            > 10000
    {
        return Err("PDF chart exceeds the series or data-point budget".into());
    }
    Ok(syntra_doc_engine::chart_render::render_chart_svg(
        element, x, y, width, height,
    ))
}

pub(super) fn whiteboard(content: &Value) -> Result<PrintDocument, String> {
    let (width, height) = canvas_dimensions(
        &content["canvas_width"],
        &content["canvas_height"],
        1920.,
        1080.,
    )?;
    let background = color(content["background_color"].as_str(), "white");
    let mut body = format!(
        "<section class=\"canvas-page\" style=\"width:{width}px;height:{height}px;background:{background}\">"
    );
    let mut remaining = MAX_OBJECTS;
    for layer in content["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|layer| layer["visible"] != false)
    {
        body.push_str(&format!(
            "<div style=\"position:absolute;inset:0;opacity:{}\">",
            finite(number(&layer["opacity"], 1.), 1., 0., 1.)
        ));
        for element in layer["elements"].as_array().into_iter().flatten() {
            board_element(element, &mut body, &mut remaining, width, height)?;
        }
        body.push_str("</div>");
    }
    for element in content["elements"].as_array().into_iter().flatten() {
        board_element(element, &mut body, &mut remaining, width, height)?;
    }
    body.push_str("</section>");
    let (width_mm, height_mm) = (width * 25.4 / 96., height * 25.4 / 96.);
    Ok(html(
        content["title"].as_str().unwrap_or("Whiteboard"),
        &format!("@page{{size:{width_mm}mm {height_mm}mm}}{CANVAS_CSS}"),
        &body,
        width_mm,
        height_mm,
    ))
}

fn board_element(
    element: &Value,
    out: &mut String,
    remaining: &mut usize,
    canvas_width: f64,
    canvas_height: f64,
) -> Result<(), String> {
    budget(remaining)?;
    let num = |key, default| number(&element[key], default);
    let stroke = color(element["stroke_color"].as_str(), "#111827");
    let fill = color(element["fill_color"].as_str(), "none");
    let stroke_width = finite(num("stroke_width", 1.), 1., 0., 1000.);
    let attrs = format!("fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\"");
    let (x, y) = (num("x", 0.), num("y", 0.));
    let svg = match element["type"].as_str().unwrap_or_default() {
        "Rectangle" => {
            let (width, height) = (num("width", 100.).max(0.), num("height", 100.).max(0.));
            format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" rx=\"{}\" {attrs} transform=\"rotate({} {} {})\"/>",
                num("corner_radius", 0.).max(0.),
                num("rotation", 0.),
                x + width / 2.,
                y + height / 2.
            )
        }
        "Ellipse" => format!(
            "<ellipse cx=\"{x}\" cy=\"{y}\" rx=\"{}\" ry=\"{}\" {attrs} transform=\"rotate({} {x} {y})\"/>",
            num("rx", 50.).max(0.),
            num("ry", 50.).max(0.),
            num("rotation", 0.)
        ),
        "Path" => {
            let mut points = Vec::new();
            for point in element["points"].as_array().into_iter().flatten() {
                budget(remaining)?;
                points.push(format!(
                    "{},{}",
                    number(&point["x"], 0.),
                    number(&point["y"], 0.)
                ));
            }
            let tag = if element["closed"] == true {
                "polygon"
            } else {
                "polyline"
            };
            format!(
                "<{tag} points=\"{}\" {attrs} stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                points.join(" ")
            )
        }
        "Line" => {
            let start = &element["start"];
            let end = &element["end"];
            let id = *remaining;
            let marker = format!(
                "<defs><marker id=\"arrow{id}\" markerWidth=\"8\" markerHeight=\"8\" refX=\"7\" refY=\"4\" orient=\"auto-start-reverse\"><path d=\"M0 0L8 4L0 8Z\" fill=\"{stroke}\"/></marker></defs>"
            );
            format!(
                "{marker}<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width}\"{}{} />",
                number(&start["x"], 0.),
                number(&start["y"], 0.),
                number(&end["x"], 0.),
                number(&end["y"], 0.),
                if element["arrow_start"] == true {
                    format!(" marker-start=\"url(#arrow{id})\"")
                } else {
                    String::new()
                },
                if element["arrow_end"] == true {
                    format!(" marker-end=\"url(#arrow{id})\"")
                } else {
                    String::new()
                }
            )
        }
        "Text" => {
            out.push_str(&format!("<div style=\"position:absolute;left:{x}px;top:{y}px;white-space:pre;font-family:{};font-size:{}px;color:{};font-weight:{};font-style:{}\">{}</div>",font(element["font_family"].as_str().unwrap_or("Arial")),finite(num("font_size",16.),16.,1.,1000.),color(element["color"].as_str(),"#111827"),if element["bold"]==true {"bold"} else {"normal"},if element["italic"]==true {"italic"} else {"normal"},escape(element["text"].as_str().unwrap_or_default())));
            return Ok(());
        }
        "Image" => {
            out.push_str(&format!("<img src=\"{}\" alt=\"Whiteboard image\" style=\"position:absolute;left:{x}px;top:{y}px;width:{}px;height:{}px;max-width:none;transform:rotate({}deg)\">",image(element["url"].as_str().unwrap_or_default())?,num("width",100.).max(0.),num("height",100.).max(0.),num("rotation",0.)));
            return Ok(());
        }
        other => {
            return Err(format!(
                "PDF whiteboard contains unsupported object '{other}'"
            ));
        }
    };
    out.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" style=\"position:absolute;inset:0;width:100%;height:100%\" viewBox=\"0 0 {canvas_width} {canvas_height}\">{svg}</svg>"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn slides_keep_order_text_shape_and_embedded_images_on_separate_pages() {
        let content = json!({"title":"多页演示","metadata":{"slide_width":1280,"slide_height":720},"slides":[{"background":{"Solid":"#112233"},"elements":[{"type":"TextBox","x":40,"y":50,"width":600,"height":100,"content":[{"text":"第一页 中文","font_size":24}],"text_layout":{"inset_left":2,"wrap":true}}]},{"background":{"Gradient":["#ffffff","#cc5500",45]},"elements":[{"type":"Shape","shape_type":"Ellipse","x":0,"y":0,"width":200,"height":100,"fill_color":"#ff0000"},{"type":"Image","x":200,"y":100,"width":20,"height":20,"url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4//8/AAX+Av4N70a4AAAAAElFTkSuQmCC"}]}]});
        let printed = slides(&content).unwrap();
        assert_eq!(printed.html.matches("class=\"canvas-page\"").count(), 2);
        assert!(printed.html.contains("第一页 中文") && printed.html.contains("font-size:24pt"));
        assert!(
            printed.html.contains("<ellipse") && printed.html.contains("data:image/png;base64,")
        );
        assert!(printed.html.contains("linear-gradient(45deg"));
    }
    #[test]
    fn hidden_board_layers_are_not_exported_and_vector_objects_are_kept() {
        let printed=whiteboard(&json!({"title":"白板","canvas_width":800,"canvas_height":600,"layers":[{"visible":false,"elements":[{"type":"Text","text":"SHOULD_NOT_EXPORT_LAYER"}]},{"visible":true,"opacity":0.5,"elements":[{"type":"Text","text":"可见","x":20,"y":30},{"type":"Line","start":{"x":0,"y":0},"end":{"x":100,"y":100},"arrow_end":true}]}]})).unwrap();
        assert!(!printed.html.contains("SHOULD_NOT_EXPORT_LAYER"));
        assert!(printed.html.contains("可见") && printed.html.contains("marker-end="));
    }

    #[test]
    fn oversized_charts_fail_instead_of_silently_dropping_data() {
        let chart = json!({"chart_type":"Line","data":{"labels":[],"datasets":[{"values":vec![1.;10_001]}]}});
        assert!(
            chart_svg(&chart, 0., 0., 500., 300.)
                .unwrap_err()
                .contains("data-point budget")
        );
        assert!(whiteboard(&json!({"canvas_width":1_000_000,"canvas_height":720})).is_err());
    }
}
