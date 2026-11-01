//! A small font-free Office icon set. Geometry uses the same 24-unit canvas,
//! rounded 1.8-unit outlines, and inherits its visible color through .ui-icon.
use glory::prelude::*;
use glory::render::{
    GraphicPoint as Point, GraphicProps, GraphicStroke, PathCommand as Path, StrokeCap,
};
use glory::ui::graphic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Office,
    Home,
    Folder,
    Document,
    Spreadsheet,
    Presentation,
    Whiteboard,
    Template,
    Bell,
    Sun,
    Moon,
    Globe,
    User,
    Sparkles,
    Plus,
    Close,
    ChevronRight,
    ChevronDown,
    ArrowLeft,
    Upload,
    Download,
    Grid,
    List,
    Search,
    Pencil,
    Trash,
    Check,
    Info,
    Warning,
    Logout,
}

impl Icon {
    pub fn document(kind: &str) -> Self {
        match kind {
            "folder" => Self::Folder,
            "writer" => Self::Document,
            "calc" => Self::Spreadsheet,
            "slides" => Self::Presentation,
            "whiteboard" => Self::Whiteboard,
            _ => Self::Document,
        }
    }
}

fn line(path: &mut Vec<Path>, points: &[(f64, f64)]) {
    if let Some(&(x, y)) = points.first() {
        path.push(Path::Move(Point::new(x, y)));
        path.extend(
            points
                .iter()
                .skip(1)
                .map(|&(x, y)| Path::Line(Point::new(x, y))),
        );
    }
}
fn rect(path: &mut Vec<Path>, x: f64, y: f64, width: f64, height: f64) {
    line(
        path,
        &[
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
        ],
    );
    path.push(Path::Close);
}
fn curve(path: &mut Vec<Path>, first: (f64, f64), second: (f64, f64), end: (f64, f64)) {
    path.push(Path::Cubic {
        first: Point::new(first.0, first.1),
        second: Point::new(second.0, second.1),
        end: Point::new(end.0, end.1),
    });
}
fn circle(path: &mut Vec<Path>, x: f64, y: f64, radius: f64) {
    let k = radius * 0.552_284_75;
    path.push(Path::Move(Point::new(x + radius, y)));
    curve(
        path,
        (x + radius, y + k),
        (x + k, y + radius),
        (x, y + radius),
    );
    curve(
        path,
        (x - k, y + radius),
        (x - radius, y + k),
        (x - radius, y),
    );
    curve(
        path,
        (x - radius, y - k),
        (x - k, y - radius),
        (x, y - radius),
    );
    curve(
        path,
        (x + k, y - radius),
        (x + radius, y - k),
        (x + radius, y),
    );
    path.push(Path::Close);
}

fn geometry(kind: Icon) -> Vec<Path> {
    let mut p = Vec::new();
    match kind {
        Icon::Office => {
            rect(&mut p, 7., 3., 13., 17.);
            line(&mut p, &[(4., 7.), (4., 22.), (17., 22.)]);
            for y in [8., 12., 16.] {
                line(&mut p, &[(10., y), (17., y)]);
            }
        }
        Icon::Home => {
            line(&mut p, &[(3., 10.), (12., 3.), (21., 10.)]);
            line(
                &mut p,
                &[
                    (5., 9.),
                    (5., 21.),
                    (10., 21.),
                    (10., 14.),
                    (14., 14.),
                    (14., 21.),
                    (19., 21.),
                    (19., 9.),
                ],
            );
        }
        Icon::Folder => {
            line(
                &mut p,
                &[
                    (3., 7.),
                    (3., 4.),
                    (9., 4.),
                    (12., 7.),
                    (21., 7.),
                    (21., 20.),
                    (3., 20.),
                    (3., 7.),
                    (21., 7.),
                ],
            );
        }
        Icon::Document => {
            line(
                &mut p,
                &[
                    (14., 3.),
                    (5., 3.),
                    (5., 21.),
                    (19., 21.),
                    (19., 8.),
                    (14., 3.),
                    (14., 8.),
                    (19., 8.),
                ],
            );
            for y in [12., 16.] {
                line(&mut p, &[(8., y), (16., y)]);
            }
        }
        Icon::Spreadsheet => {
            rect(&mut p, 3., 3., 18., 18.);
            line(&mut p, &[(3., 9.), (21., 9.)]);
            line(&mut p, &[(3., 15.), (21., 15.)]);
            line(&mut p, &[(9., 3.), (9., 21.)]);
            line(&mut p, &[(15., 9.), (15., 21.)]);
        }
        Icon::Presentation => {
            rect(&mut p, 3., 4., 18., 13.);
            line(&mut p, &[(12., 17.), (12., 21.)]);
            line(&mut p, &[(8., 21.), (16., 21.)]);
            line(&mut p, &[(7., 13.), (10., 10.), (13., 12.), (17., 8.)]);
        }
        Icon::Whiteboard => {
            rect(&mut p, 3., 4., 18., 14.);
            line(&mut p, &[(8., 18.), (6., 22.)]);
            line(&mut p, &[(16., 18.), (18., 22.)]);
            line(&mut p, &[(7., 13.), (10., 9.), (13., 13.), (17., 8.)]);
        }
        Icon::Template | Icon::Grid => {
            for (x, y) in [(3., 3.), (14., 3.), (3., 14.), (14., 14.)] {
                rect(&mut p, x, y, 7., 7.);
            }
        }
        Icon::Bell => {
            line(&mut p, &[(5., 17.), (19., 17.)]);
            curve(&mut p, (16., 14.), (18., 14.), (18., 10.));
            curve(&mut p, (18., 2.), (6., 2.), (6., 10.));
            curve(&mut p, (6., 14.), (8., 14.), (5., 17.));
            line(&mut p, &[(10., 21.), (14., 21.)]);
            line(&mut p, &[(12., 2.), (12., 4.)]);
        }
        Icon::Sun => {
            circle(&mut p, 12., 12., 4.);
            for points in [
                [(12., 2.), (12., 4.)],
                [(12., 20.), (12., 22.)],
                [(2., 12.), (4., 12.)],
                [(20., 12.), (22., 12.)],
                [(5., 5.), (6.5, 6.5)],
                [(17.5, 17.5), (19., 19.)],
                [(5., 19.), (6.5, 17.5)],
                [(17.5, 6.5), (19., 5.)],
            ] {
                line(&mut p, &points);
            }
        }
        Icon::Moon => {
            line(&mut p, &[(20., 14.)]);
            curve(&mut p, (11., 18.), (6., 10.), (10., 3.));
            curve(&mut p, (1., 5.), (1., 17.), (10., 21.));
            curve(&mut p, (14., 23.), (19., 20.), (20., 14.));
            p.push(Path::Close);
        }
        Icon::Globe => {
            circle(&mut p, 12., 12., 9.);
            line(&mut p, &[(3., 12.), (21., 12.)]);
            line(&mut p, &[(12., 3.)]);
            curve(&mut p, (6., 8.), (6., 16.), (12., 21.));
            curve(&mut p, (18., 16.), (18., 8.), (12., 3.));
        }
        Icon::User => {
            circle(&mut p, 12., 7., 4.);
            line(&mut p, &[(4., 21.)]);
            curve(&mut p, (4., 12.), (20., 12.), (20., 21.));
        }
        Icon::Sparkles => {
            line(
                &mut p,
                &[
                    (12., 3.),
                    (14.5, 9.5),
                    (21., 12.),
                    (14.5, 14.5),
                    (12., 21.),
                    (9.5, 14.5),
                    (3., 12.),
                    (9.5, 9.5),
                    (12., 3.),
                ],
            );
            line(&mut p, &[(20., 2.), (20., 6.)]);
            line(&mut p, &[(18., 4.), (22., 4.)]);
        }
        Icon::Plus => {
            line(&mut p, &[(12., 4.), (12., 20.)]);
            line(&mut p, &[(4., 12.), (20., 12.)]);
        }
        Icon::Close => {
            line(&mut p, &[(6., 6.), (18., 18.)]);
            line(&mut p, &[(18., 6.), (6., 18.)]);
        }
        Icon::ChevronRight => line(&mut p, &[(9., 5.), (16., 12.), (9., 19.)]),
        Icon::ChevronDown => line(&mut p, &[(5., 9.), (12., 16.), (19., 9.)]),
        Icon::ArrowLeft => {
            line(&mut p, &[(20., 12.), (4., 12.), (10., 6.)]);
            line(&mut p, &[(4., 12.), (10., 18.)]);
        }
        Icon::Upload | Icon::Download => {
            line(&mut p, &[(4., 16.), (4., 21.), (20., 21.), (20., 16.)]);
            line(&mut p, &[(12., 3.), (12., 16.)]);
            if kind == Icon::Upload {
                line(&mut p, &[(6., 9.), (12., 3.), (18., 9.)]);
            } else {
                line(&mut p, &[(6., 10.), (12., 16.), (18., 10.)]);
            }
        }
        Icon::List => {
            for y in [5., 12., 19.] {
                line(&mut p, &[(8., y), (21., y)]);
                rect(&mut p, 3., y - 1., 2., 2.);
            }
        }
        Icon::Search => {
            circle(&mut p, 10., 10., 6.);
            line(&mut p, &[(15., 15.), (21., 21.)]);
        }
        Icon::Pencil => {
            line(
                &mut p,
                &[
                    (4., 16.),
                    (16., 4.),
                    (20., 8.),
                    (8., 20.),
                    (3., 21.),
                    (4., 16.),
                    (8., 20.),
                ],
            );
            line(&mut p, &[(13., 7.), (17., 11.)]);
        }
        Icon::Trash => {
            line(&mut p, &[(3., 6.), (21., 6.)]);
            line(&mut p, &[(9., 6.), (9., 3.), (15., 3.), (15., 6.)]);
            line(&mut p, &[(5., 6.), (6., 21.), (18., 21.), (19., 6.)]);
            line(&mut p, &[(10., 10.), (10., 17.)]);
            line(&mut p, &[(14., 10.), (14., 17.)]);
        }
        Icon::Check => line(&mut p, &[(4., 12.), (10., 18.), (20., 6.)]),
        Icon::Info => {
            circle(&mut p, 12., 12., 9.);
            line(&mut p, &[(12., 11.), (12., 17.)]);
            circle(&mut p, 12., 7., 0.5);
        }
        Icon::Warning => {
            line(&mut p, &[(12., 3.), (22., 21.), (2., 21.), (12., 3.)]);
            line(&mut p, &[(12., 9.), (12., 14.)]);
            circle(&mut p, 12., 18., 0.5);
        }
        Icon::Logout => {
            line(&mut p, &[(10., 3.), (4., 3.), (4., 21.), (10., 21.)]);
            line(&mut p, &[(9., 12.), (21., 12.), (16., 7.)]);
            line(&mut p, &[(21., 12.), (16., 17.)]);
        }
    }
    p
}

/// Decorative SVG: callers put the accessible name on the enclosing control.
pub fn icon(kind: Icon) -> glory::ui::Graphic {
    icon_sized(kind, 18.)
}

pub fn icon_sized(kind: Icon, side: f64) -> glory::ui::Graphic {
    graphic()
        .class("ui-icon")
        .drawing(icon_drawing(kind))
        .style(
            Style::new()
                .width(Length::Px(side))
                .height(Length::Px(side)),
        )
}

pub fn icon_drawing(kind: Icon) -> GraphicProps {
    GraphicProps {
        width: 24.,
        height: 24.,
        commands: geometry(kind),
        stroke: Some(GraphicStroke {
            color: Color(0, 0, 0, 255),
            width: 1.8,
            cap: StrokeCap::Round,
        }),
        alternative: None,
        ..Default::default()
    }
}
