use dioxus::prelude::*;

mod collab_client;
mod components;
mod pages;
mod state;

use pages::{
    Admin, Calc, Dashboard, DocumentEditor, FileManager, Home, Login, Profile, Register, Slides,
    Whiteboard, Writer,
};

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // Provide global authentication state to all child components.
    use_context_provider(|| Signal::new(state::load_auth_state()));
    use_context_provider(|| Signal::new(Vec::<crate::components::toast::Toast>::new()));

    rsx! {
        Router::<Route> {}
        crate::components::ToastRack {}
        crate::components::post_message::PostMessageBridge {}
        crate::components::mobile_bar::MobileBar {}
    }
}

#[derive(Routable, Clone, Debug, PartialEq)]
pub enum Route {
    #[route("/")]
    Home {},
    #[route("/login")]
    Login {},
    #[route("/register")]
    Register {},
    #[route("/dashboard")]
    Dashboard {},
    #[route("/files")]
    FileManager {},
    #[route("/documents/:id")]
    DocumentEditor { id: String },
    #[route("/writer/:id")]
    Writer { id: String },
    #[route("/calc/:id")]
    Calc { id: String },
    #[route("/slides/:id")]
    Slides { id: String },
    #[route("/whiteboard/:id")]
    Whiteboard { id: String },
    #[route("/admin")]
    Admin {},
    #[route("/profile")]
    Profile {},
}
