use dioxus::prelude::*;

use crate::Route;

/// Public landing page shown to unauthenticated visitors.
#[component]
pub fn Home() -> Element {
    rsx! {
        div { class: "landing-page",
            // Hero section
            header { class: "hero",
                div { class: "hero-content",
                    h1 { class: "hero-title", "Welcome to Contrix Office" }
                    p { class: "hero-subtitle",
                        "A full-featured office suite built entirely in Rust. "
                        "Create documents, spreadsheets, presentations, and whiteboards -- "
                        "all from your browser."
                    }
                    div { class: "hero-buttons",
                        Link { to: Route::Login {}, class: "btn btn-primary btn-lg", "Sign In" }
                        Link { to: Route::Register {}, class: "btn btn-secondary btn-lg", "Sign Up" }
                    }
                }
            }

            // Features section
            section { class: "features",
                h2 { class: "section-title", "Everything you need" }
                div { class: "feature-grid",
                    // Writer
                    div { class: "feature-card",
                        div { class: "feature-icon writer-icon", "W" }
                        h3 { "Writer" }
                        p { "A rich text editor for documents, reports, and letters with formatting, tables, and images." }
                    }
                    // Calc
                    div { class: "feature-card",
                        div { class: "feature-icon calc-icon", "C" }
                        h3 { "Calc" }
                        p { "A powerful spreadsheet with formulas, charts, and data analysis tools for all your number-crunching needs." }
                    }
                    // Slides
                    div { class: "feature-card",
                        div { class: "feature-icon slides-icon", "S" }
                        h3 { "Slides" }
                        p { "Create stunning presentations with transitions, animations, speaker notes, and collaborative editing." }
                    }
                    // Whiteboard
                    div { class: "feature-card",
                        div { class: "feature-icon whiteboard-icon", "B" }
                        h3 { "Whiteboard" }
                        p { "A freeform canvas for brainstorming, sketching, and diagramming with pens, shapes, and text." }
                    }
                }
            }

            // Highlights section
            section { class: "highlights",
                div { class: "highlight",
                    h3 { "Real-time Collaboration" }
                    p { "Work with your team simultaneously on any document. See changes in real time with conflict-free editing." }
                }
                div { class: "highlight",
                    h3 { "Built with Rust" }
                    p { "Blazing fast performance, memory safety, and WebAssembly-powered frontend for a native-like experience." }
                }
                div { class: "highlight",
                    h3 { "Self-Hosted" }
                    p { "Your data stays on your servers. Deploy with Docker in minutes and keep full control of your documents." }
                }
            }

            // Footer
            footer { class: "landing-footer",
                p { "Contrix Office - Open Source Office Suite" }
                p { class: "footer-links",
                    a { href: "https://github.com/contrix-org/office", target: "_blank", "GitHub" }
                    span { " | " }
                    a { href: "#", "Documentation" }
                    span { " | " }
                    a { href: "#", "License: MIT / Apache-2.0" }
                }
            }
        }
    }
}
