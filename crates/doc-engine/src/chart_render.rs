//! Shared, self-contained chart drawing for the editor and print/export paths.
//! It consumes the serialized chart model so Calc and Slides use identical axes,
//! legends and labels, while keeping the numeric data independently editable.
use serde_json::Value;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn number(value: &Value, fallback: f64) -> f64 {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .unwrap_or(fallback)
        .clamp(-1e100, 1e100)
}
fn color(value: &Value, fallback: &str) -> String {
    let s = value.as_str().unwrap_or(fallback);
    if s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        s.to_string()
    } else {
        fallback.to_string()
    }
}
fn label_number(value: f64, format: Option<&str>) -> String {
    let format = format.unwrap_or("General");
    if format.contains('%') {
        let decimals = format
            .split('.')
            .nth(1)
            .map(|v| v.chars().take_while(|c| *c == '0').count())
            .unwrap_or(0)
            .min(8);
        return format!("{:.*}%", decimals, value * 100.);
    }
    if let Some(fraction) = format.split('.').nth(1) {
        let decimals = fraction
            .chars()
            .take_while(|c| *c == '0' || *c == '#')
            .count()
            .min(8);
        if decimals > 0 {
            return format!("{value:.decimals$}");
        }
    }
    if (value - value.round()).abs() < 1e-8 {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}
fn nice_step(value: f64) -> f64 {
    let exponent = value.max(1e-12).log10().floor();
    let base = 10f64.powf(exponent);
    let fraction = value / base;
    [1., 2., 2.5, 5., 10.]
        .into_iter()
        .find(|n| *n >= fraction)
        .unwrap_or(10.)
        * base
}

/// Render chart geometry and semantic labels. All user strings are XML escaped.
pub fn render_chart_svg(element: &Value, x: f64, y: f64, width: f64, height: f64) -> String {
    let data = &element["data"];
    let style = &data["presentation"];
    let datasets = data["datasets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let labels = data["labels"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let values: Vec<Vec<f64>> = datasets
        .iter()
        .take(100)
        .map(|s| {
            s["values"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .take(10000)
                .map(|n| number(n, 0.))
                .collect()
        })
        .collect();
    if values.iter().map(Vec::len).sum::<usize>() > 10000 || width <= 0. || height <= 0. {
        return String::new();
    }
    let palette = [
        "#336699", "#D97706", "#16A34A", "#9333EA", "#0891B2", "#DC2626",
    ];
    let series_color = |i: usize| {
        color(
            &datasets.get(i).unwrap_or(&Value::Null)["color"],
            palette[i % palette.len()],
        )
    };
    let vary_colors = style.is_null() || style["vary_colors"] == true;
    let point_color = |s: usize, i: usize| {
        let fallback = if vary_colors {
            palette[i % palette.len()].to_string()
        } else {
            series_color(s)
        };
        color(&style["point_colors"][s][i], &fallback)
    };
    let kind = element["chart_type"].as_str().unwrap_or("Bar");
    let pie = matches!(kind, "Pie" | "Doughnut");
    let font = escape(&format!(
        "{},Arial,Microsoft YaHei,Office CJK,Noto Sans CJK SC,sans-serif",
        style["font_family"].as_str().unwrap_or("Arial")
    ));
    let font_size = number(&style["font_size"], 12.).clamp(6., 96.);
    let title = style["title"]
        .as_str()
        .or_else(|| element["title"].as_str())
        .unwrap_or_default();
    let title_size = number(&style["title_font_size"], 24.).clamp(8., 144.);
    let show_legend = style["show_legend"] == true;
    let legend_position = style["legend_position"].as_str().unwrap_or("bottom");
    let title_space = if title.is_empty() {
        height * 0.04
    } else {
        (height * 0.16).max(title_size * 1.6)
    };
    let side_legend = show_legend && matches!(legend_position, "right" | "left" | "r" | "l");
    let legend_space = if show_legend {
        if side_legend {
            width.min(180.) * 0.28 + 70.
        } else {
            (height * 0.07).max(font_size + 8.)
        }
    } else {
        0.
    };
    let left_margin = if pie {
        8.
    } else {
        (width * 0.04).max(font_size * 1.9 + 8.)
    };
    let right_margin = if pie { 8. } else { (width * 0.02).max(8.) };
    let mut plot_x = x + left_margin;
    let mut plot_y = y + title_space;
    let mut plot_w = (width - left_margin - right_margin).max(1.);
    let mut plot_h = (height
        - title_space
        - if pie {
            8.
        } else {
            (height * 0.065).max(font_size * 1.8)
        })
    .max(1.);
    if side_legend {
        plot_w = (plot_w - legend_space).max(1.);
        if matches!(legend_position, "left" | "l") {
            plot_x += legend_space;
        }
    } else if show_legend {
        plot_h = (plot_h - legend_space).max(1.);
        if matches!(legend_position, "top" | "t") {
            plot_y += legend_space;
        }
    }
    let mut out = format!("<g font-family=\"{font}\" font-size=\"{font_size}\" fill=\"#333333\">");
    if !title.is_empty() {
        out.push_str(&format!("<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{title_size}\" font-weight=\"bold\">{}</text>",x+width/2.,y+(title_space+title_size)/2.,escape(title)));
    }
    let label_style = &style["labels"];
    let label_text = |s: usize, i: usize, value: f64, total: f64| {
        let mut parts = Vec::new();
        if label_style["show_series"] == true {
            parts.push(
                datasets
                    .get(s)
                    .and_then(|v| v["name"].as_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        if label_style["show_category"] == true {
            parts.push(
                labels
                    .get(i)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        if label_style["show_value"] == true {
            parts.push(label_number(value, label_style["number_format"].as_str()));
        }
        if label_style["show_percent"] == true && total > 0. {
            parts.push(format!("{:.0}%", value / total * 100.));
        }
        parts.join(" ")
    };
    if pie {
        let points = values.first().map(Vec::as_slice).unwrap_or_default();
        let total = points.iter().map(|n| n.max(0.)).sum::<f64>();
        let radius = (plot_w.min(plot_h) * 0.49).max(1.);
        let (cx, cy) = (plot_x + plot_w / 2., plot_y + plot_h / 2.);
        let mut angle = (-90. + number(&style["first_slice_angle"], 0.)).to_radians();
        for (i, value) in points.iter().enumerate().filter(|(_, n)| **n > 0.) {
            let delta = value / total * std::f64::consts::TAU;
            let next = angle + delta;
            let fill = point_color(0, i);
            if delta >= std::f64::consts::TAU - 1e-9 {
                out.push_str(&format!(
                    "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{radius}\" fill=\"{fill}\"/>"
                ));
            } else {
                out.push_str(&format!("<path d=\"M{cx},{cy} L{},{} A{radius},{radius} 0 {} 1 {},{} Z\" fill=\"{fill}\"/>",cx+radius*angle.cos(),cy+radius*angle.sin(),u8::from(delta>std::f64::consts::PI),cx+radius*next.cos(),cy+radius*next.sin()));
            }
            let label = label_text(0, i, *value, total);
            if !label.is_empty() {
                let r = radius * 0.75;
                let a = angle + delta / 2.;
                out.push_str(&format!("<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" dominant-baseline=\"middle\" fill=\"#111111\">{}</text>",cx+r*a.cos(),cy+r*a.sin(),escape(&label)));
            }
            angle = next;
        }
        if kind == "Doughnut" {
            out.push_str(&format!(
                "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{}\" fill=\"white\"/>",
                radius * number(&style["hole_size"], 50.).clamp(10., 90.) / 100.
            ));
        }
    } else {
        let axis = &style["value_axis"];
        let cat_axis = &style["category_axis"];
        let raw_min = values.iter().flatten().copied().fold(0., f64::min);
        let raw_max = values.iter().flatten().copied().fold(1., f64::max);
        let step = number(&axis["major_unit"], nice_step((raw_max - raw_min) / 8.)).max(1e-12);
        let minimum = number(&axis["minimum"], (raw_min / step).floor() * step);
        let maximum =
            number(&axis["maximum"], ((raw_max * 1.05) / step).ceil() * step).max(minimum + step);
        let horizontal = style["horizontal"] == true;
        let point_count = values.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let xs = if kind == "Scatter" {
            labels
                .iter()
                .take(point_count)
                .map(|v| {
                    v.as_str()?
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite())
                        .map(|n| n.clamp(-1e100, 1e100))
                })
                .collect::<Option<Vec<_>>>()
        } else {
            None
        };
        let xmin = xs
            .as_ref()
            .map(|v| v.iter().copied().fold(f64::INFINITY, f64::min))
            .filter(|n| n.is_finite())
            .unwrap_or(0.);
        let xmax = xs
            .as_ref()
            .map(|v| v.iter().copied().fold(f64::NEG_INFINITY, f64::max))
            .filter(|n| n.is_finite())
            .unwrap_or(1.);
        let category_position = |i: usize| {
            xs.as_ref()
                .and_then(|v| v.get(i))
                .map(|v| (*v - xmin) / (xmax - xmin).max(1e-9))
                .unwrap_or((i as f64 + 0.5) / point_count as f64)
        };
        let value_position = |v: f64| (v - minimum) / (maximum - minimum);
        if axis["hidden"] != true {
            let ticks = ((maximum - minimum) / step).ceil().clamp(0., 100.) as usize;
            for i in 0..=ticks {
                let v = minimum + i as f64 * step;
                if v > maximum + step * 0.001 {
                    break;
                }
                let fraction = value_position(v);
                let (tx, ty) = if horizontal {
                    (plot_x + fraction * plot_w, plot_y + plot_h + font_size + 5.)
                } else {
                    (
                        plot_x - 6.,
                        plot_y + plot_h - fraction * plot_h + font_size * 0.35,
                    )
                };
                out.push_str(&format!(
                    "<text x=\"{tx}\" y=\"{ty}\" text-anchor=\"{}\">{}</text>",
                    if horizontal { "middle" } else { "end" },
                    escape(&label_number(v, axis["number_format"].as_str()))
                ));
                if axis["major_gridlines"] == true {
                    let gc = color(&axis["grid_color"], "#D3DAE2");
                    let gw = number(&axis["grid_width"], 1.).clamp(0., 10.);
                    let (x1, y1, x2, y2) = if horizontal {
                        (tx, plot_y, tx, plot_y + plot_h)
                    } else {
                        (
                            plot_x,
                            ty - font_size * 0.35,
                            plot_x + plot_w,
                            ty - font_size * 0.35,
                        )
                    };
                    out.push_str(&format!("<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"{gc}\" stroke-width=\"{gw}\"/>"));
                }
            }
        }
        out.push_str(&format!(
            "<path d=\"M{plot_x},{plot_y} V{} H{}\" fill=\"none\" stroke=\"#AAB4C0\"/>",
            plot_y + plot_h,
            plot_x + plot_w
        ));
        let baseline = value_position(0.).clamp(0., 1.);
        for (si, series) in values.iter().enumerate() {
            let fill = series_color(si);
            let mut coordinates = Vec::new();
            for (i, value) in series.iter().enumerate() {
                let category = category_position(i);
                let fraction = value_position(*value);
                let (px, py) = if horizontal {
                    (plot_x + fraction * plot_w, plot_y + category * plot_h)
                } else {
                    (
                        plot_x + category * plot_w,
                        plot_y + (1. - fraction) * plot_h,
                    )
                };
                if kind == "Bar" {
                    let slot = if horizontal { plot_h } else { plot_w } / point_count as f64;
                    // DrawingML gapWidth is measured in units of one bar's
                    // width, not a fixed fraction of the category slot.
                    let bw = slot
                        / (values.len().max(1) as f64
                            + number(&style["gap_width"], 150.).clamp(0., 500.) / 100.);
                    let offset = (si as f64 - values.len().saturating_sub(1) as f64 / 2.) * bw;
                    let (rx, ry, rw, rh) = if horizontal {
                        let z = plot_x + baseline * plot_w;
                        (z.min(px), py + offset - bw / 2., (px - z).abs(), bw)
                    } else {
                        let z = plot_y + (1. - baseline) * plot_h;
                        (px + offset - bw / 2., z.min(py), bw, (py - z).abs())
                    };
                    out.push_str(&format!(
                        "<rect x=\"{rx}\" y=\"{ry}\" width=\"{rw}\" height=\"{rh}\" fill=\"{}\"/>",
                        point_color(si, i)
                    ));
                    let label = label_text(si, i, *value, 0.);
                    if !label.is_empty() {
                        out.push_str(&format!(
                            "<text x=\"{}\" y=\"{}\" text-anchor=\"{}\" fill=\"#111111\">{}</text>",
                            if horizontal { px + 5. } else { px + offset },
                            if horizontal {
                                py + offset + font_size * 0.35
                            } else {
                                py - 7.
                            },
                            if horizontal { "start" } else { "middle" },
                            escape(&label)
                        ));
                    }
                } else {
                    coordinates.push((px, py));
                    out.push_str(&format!(
                        "<circle cx=\"{px}\" cy=\"{py}\" r=\"3\" fill=\"{fill}\"/>"
                    ));
                    let label = label_text(si, i, *value, 0.);
                    if !label.is_empty() {
                        out.push_str(&format!(
                            "<text x=\"{px}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
                            py - 7.,
                            escape(&label)
                        ));
                    }
                }
            }
            if matches!(kind, "Line" | "Area") && !coordinates.is_empty() {
                let points = coordinates
                    .iter()
                    .map(|(x, y)| format!("{x},{y}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                if kind == "Area" {
                    out.push_str(&format!("<polygon points=\"{},{} {points} {},{}\" fill=\"{fill}\" fill-opacity=\"0.3\"/>",coordinates[0].0,plot_y+plot_h,coordinates.last().unwrap().0,plot_y+plot_h));
                }
                out.push_str(&format!("<polyline points=\"{points}\" fill=\"none\" stroke=\"{fill}\" stroke-width=\"2\"/>"));
            }
        }
        if cat_axis["hidden"] != true {
            for (i, label) in labels.iter().take(point_count).enumerate() {
                let (tx, ty) = if horizontal {
                    (
                        plot_x - 7.,
                        plot_y + category_position(i) * plot_h + font_size * 0.35,
                    )
                } else {
                    (
                        plot_x + category_position(i) * plot_w,
                        plot_y + plot_h + font_size + 7.,
                    )
                };
                out.push_str(&format!(
                    "<text x=\"{tx}\" y=\"{ty}\" text-anchor=\"{}\">{}</text>",
                    if horizontal { "end" } else { "middle" },
                    escape(label.as_str().unwrap_or_default())
                ));
            }
        }
        if let Some(t) = axis["title"].as_str().filter(|v| !v.is_empty()) {
            out.push_str(&format!("<text transform=\"translate({}, {}) rotate(-90)\" text-anchor=\"middle\">{}</text>",x+font_size,plot_y+plot_h/2.,escape(t)));
        }
        if let Some(t) = cat_axis["title"].as_str().filter(|v| !v.is_empty()) {
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
                plot_x + plot_w / 2.,
                plot_y + plot_h + font_size * 2.5,
                escape(t)
            ));
        }
    }
    if show_legend {
        let items: Vec<(String, String)> = if pie {
            labels
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    (
                        l.as_str().unwrap_or_default().to_string(),
                        point_color(0, i),
                    )
                })
                .collect()
        } else {
            datasets
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        s["name"].as_str().unwrap_or_default().to_string(),
                        series_color(i),
                    )
                })
                .collect()
        };
        let item_widths: Vec<f64> = items
            .iter()
            .map(|(s, _)| {
                s.chars()
                    .map(|c| {
                        if c.is_ascii() {
                            font_size * 0.55
                        } else {
                            font_size
                        }
                    })
                    .sum::<f64>()
                    + 28.
            })
            .collect();
        let total = item_widths.iter().sum::<f64>();
        let mut lx = if side_legend {
            if matches!(legend_position, "left" | "l") {
                x + 4.
            } else {
                plot_x + plot_w + 12.
            }
        } else {
            x + (width - total) / 2.
        };
        let mut ly = if side_legend {
            plot_y + plot_h / 2. - items.len() as f64 * (font_size + 10.) / 2.
        } else if matches!(legend_position, "top" | "t") {
            y + title_space + font_size + 5.
        } else {
            y + height - 10.
        };
        for ((name, fill), item_width) in items.into_iter().zip(item_widths) {
            out.push_str(&format!("<rect x=\"{lx}\" y=\"{}\" width=\"9\" height=\"9\" fill=\"{fill}\"/><text x=\"{}\" y=\"{ly}\">{}</text>",ly-9.,lx+14.,escape(&name)));
            if side_legend {
                ly += font_size + 10.;
            } else {
                lx += item_width;
            }
        }
    }
    out.push_str("</g>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chart_strings_and_color_fallbacks_cannot_inject_svg_markup() {
        let element = serde_json::json!({"chart_type":"Pie","data":{"labels":["<script>category</script>"],"datasets":[{"name":"unsafe","values":[1],"color":"\"/><script>color</script>"}],"presentation":{"title":"<script>title</script>","show_legend":true,"point_colors":[["\" onload=\"alert(1)"]]}}});
        let svg = render_chart_svg(&element, 0., 0., 500., 300.);
        assert!(!svg.contains("<script>") && !svg.contains(" onload="));
        assert!(svg.contains("&lt;script&gt;title&lt;/script&gt;"));
        assert!(svg.contains("fill=\"#336699\""));
    }
    #[test]
    fn legacy_pie_without_presentation_retains_distinct_slice_colors() {
        let chart = serde_json::json!({"chart_type":"Pie","data":{"labels":["A","B","C"],"datasets":[{"name":"Old chart","values":[50,30,20],"color":null}]}});
        let svg = render_chart_svg(&chart, 0., 0., 500., 300.);
        for color in ["#336699", "#D97706", "#16A34A"] {
            assert!(svg.contains(&format!("fill=\"{color}\"")));
        }
        let mut explicit = chart;
        explicit["data"]["presentation"] = serde_json::json!({"vary_colors":false});
        let svg = render_chart_svg(&explicit, 0., 0., 500., 300.);
        assert!(
            !svg.contains("fill=\"#D97706\""),
            "explicit source color policy must override the legacy default"
        );
    }
}
