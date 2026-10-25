use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Home(_scope: Scope, _auth: Cage<AuthState>) -> impl Widget {
    col().class("landing-page").fill((
        col().class("hero").fill((
            heading("Welcome to Contrix Office"),
            label(
                "Create documents, spreadsheets, presentations, and whiteboards in your browser.",
            ),
            row().fill((anchor("Sign In", "/login"), anchor("Sign Up", "/register"))),
        )),
        row().class("features").fill(vec![
            col().fill((
                heading("Writer"),
                label("Rich documents with formatting, tables, and images."),
            )),
            col().fill((
                heading("Calc"),
                label("Spreadsheets, formulas, charts, and data analysis."),
            )),
            col().fill((
                heading("Slides"),
                label("Presentations, speaker notes, and collaborative editing."),
            )),
            col().fill((
                heading("Whiteboard"),
                label("A freeform canvas for sketching and diagramming."),
            )),
        ]),
        heading("Real-time collaboration"),
        label("Work together while keeping your documents on your own server."),
        anchor(
            "GitHub · MIT / Apache-2.0",
            "https://github.com/contrix-org/office",
        ),
    ))
}
