use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;

fn feature(
    icon: &'static str,
    kind: &'static str,
    title: &'static str,
    description: &'static str,
) -> impl Widget {
    view().class("feature-card").fill((
        view()
            .class(format!("feature-icon {kind}-icon"))
            .fill(label(icon)),
        text().kind(TextKind::Heading(3)).content(title),
        text().kind(TextKind::Paragraph).content(description),
    ))
}
fn highlight(title: &'static str, description: &'static str) -> impl Widget {
    view().class("highlight").fill((
        text().kind(TextKind::Heading(3)).content(title),
        text().kind(TextKind::Paragraph).content(description),
    ))
}
#[allow(non_snake_case)]
pub fn Home(_scope: Scope, _auth: Cage<AuthState>) -> impl Widget {
    view().class("landing-page").fill((
        view().class("hero").fill(view().class("hero-content").fill((
            heading("Welcome to Contrix Office").class("hero-title"),
            text().kind(TextKind::Paragraph).class("hero-subtitle").content("A full-featured office suite built entirely in Rust. Create documents, spreadsheets, presentations, and whiteboards -- all from your browser."),
            view().class("hero-buttons").fill((anchor("Sign In", "/login").class("btn btn-primary btn-lg"), anchor("Sign Up", "/register").class("btn btn-secondary btn-lg"))),
        ))),
        view().class("features").fill((
            text().kind(TextKind::Heading(2)).class("section-title").content("Everything you need"),
            view().class("feature-grid").fill((
                feature("W", "writer", "Writer", "A rich text editor for documents, reports, and letters with formatting, tables, and images."),
                feature("C", "calc", "Calc", "A powerful spreadsheet with formulas, charts, and data analysis tools for all your number-crunching needs."),
                feature("S", "slides", "Slides", "Create stunning presentations with transitions, animations, speaker notes, and collaborative editing."),
                feature("B", "whiteboard", "Whiteboard", "A freeform canvas for brainstorming, sketching, and diagramming with pens, shapes, and text."),
            )),
        )),
        view().class("highlights").fill((
            highlight("Real-time Collaboration", "Work with your team simultaneously on any document. See changes in real time with conflict-free editing."),
            highlight("Built with Rust", "Blazing fast performance, memory safety, and WebAssembly-powered frontend for a native-like experience."),
            highlight("Self-Hosted", "Your data stays on your servers. Deploy with Docker in minutes and keep full control of your documents."),
        )),
        view().class("landing-footer").fill((
            text().kind(TextKind::Paragraph).content("Contrix Office - Open Source Office Suite"),
            view().class("footer-links").fill((anchor("GitHub", "https://github.com/contrix-org/office"), label(" | "), anchor("Documentation", "#"), label(" | "), anchor("License: MIT / Apache-2.0", "#"))),
        )),
    ))
}
