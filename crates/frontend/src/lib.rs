//! Syntra Office's Glory + Sapid browser application.

pub mod collab_client;
pub mod components;
pub mod i18n;
pub mod notification_client;
pub mod pages;
pub mod state;

use glory::prelude::*;

/// Application URLs shared by the shell and document links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Home {},
    Login {},
    Register {},
    Dashboard {},
    FileManager {},
    DocumentEditor { id: String },
    Writer { id: String },
    Calc { id: String },
    Slides { id: String },
    Whiteboard { id: String },
    Pdf { id: String },
    Admin {},
    Profile {},
    NotFound {},
}

impl Route {
    pub fn requires_auth(&self) -> bool {
        !matches!(
            self,
            Self::Home {} | Self::Login {} | Self::Register {} | Self::NotFound {}
        )
    }
    pub fn parse(path: &str) -> Self {
        let path = path.split(['?', '#']).next().unwrap_or("/");
        match path.trim_end_matches('/') {
            "" => Self::Home {},
            "/login" => Self::Login {},
            "/register" => Self::Register {},
            "/dashboard" => Self::Dashboard {},
            "/files" => Self::FileManager {},
            "/admin" => Self::Admin {},
            "/profile" => Self::Profile {},
            path => match path.rsplit_once('/') {
                Some((kind, id)) if !id.is_empty() && uuid::Uuid::parse_str(id).is_ok() => {
                    let id = id.to_owned();
                    match kind {
                        "/documents" => Self::DocumentEditor { id },
                        "/writer" => Self::Writer { id },
                        "/calc" => Self::Calc { id },
                        "/slides" => Self::Slides { id },
                        "/whiteboard" => Self::Whiteboard { id },
                        "/pdf" => Self::Pdf { id },
                        _ => Self::NotFound {},
                    }
                }
                _ => Self::NotFound {},
            },
        }
    }

    pub fn url(&self) -> String {
        match self {
            Self::Home {} => "/".into(),
            Self::Login {} => "/login".into(),
            Self::Register {} => "/register".into(),
            Self::Dashboard {} => "/dashboard".into(),
            Self::FileManager {} => "/files".into(),
            Self::Admin {} => "/admin".into(),
            Self::Profile {} => "/profile".into(),
            Self::DocumentEditor { id } => format!("/documents/{id}"),
            Self::Writer { id } => format!("/writer/{id}"),
            Self::Calc { id } => format!("/calc/{id}"),
            Self::Slides { id } => format!("/slides/{id}"),
            Self::Whiteboard { id } => format!("/whiteboard/{id}"),
            Self::Pdf { id } => format!("/pdf/{id}"),
            Self::NotFound {} => "/404".into(),
        }
    }
}
impl std::fmt::Display for Route {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.url())
    }
}

/// Mount each page under its own Owner; controls and tasks retire with it.
pub fn app(route: Route, initial_auth: state::AuthState) -> impl Widget {
    widget(move |scope| {
        let auth = scope.cage(initial_auth.clone());
        let route = if route.requires_auth() && initial_auth.token.is_none() {
            Route::Login {}
        } else {
            route.clone()
        };
        let protected = route.requires_auth();
        let mut page: Box<dyn Widget> = match route {
            Route::Home {} => Box::new(pages::Home(scope.clone(), auth)),
            Route::Login {} => Box::new(pages::Login(scope.clone(), auth)),
            Route::Register {} => Box::new(pages::Register(scope.clone(), auth)),
            Route::Dashboard {} => Box::new(pages::Dashboard(scope.clone(), auth)),
            Route::FileManager {} => Box::new(pages::FileManager(scope.clone(), auth)),
            Route::Admin {} => Box::new(pages::Admin(scope.clone(), auth)),
            Route::Profile {} => Box::new(pages::Profile(scope.clone(), auth)),
            Route::DocumentEditor { id } => {
                Box::new(pages::DocumentEditor(scope.clone(), auth, id))
            }
            Route::Writer { id } => Box::new(pages::Writer(scope.clone(), auth, id)),
            Route::Calc { id } => Box::new(pages::Calc(scope.clone(), auth, id)),
            Route::Slides { id } => Box::new(pages::Slides(scope.clone(), auth, id)),
            Route::Whiteboard { id } => Box::new(pages::Whiteboard(scope.clone(), auth, id)),
            Route::Pdf { id } => Box::new(pages::Pdf(scope.clone(), auth, id)),
            Route::NotFound {} => Box::new(col().class("not-found").fill((
                text().content("Page not found"),
                link().target("/").label("Syntra Office"),
            ))),
        };
        page.build(scope)?;
        let bus = scope.cage(Vec::<components::toast::Toast>::new());
        components::toast::ToastRack(scope.clone(), bus).build(scope)?;
        components::post_message::PostMessageBridge(scope.clone()).build(scope)?;
        if protected {
            components::mobile_bar::MobileBar(scope.clone(), auth).build(scope)?;
        }
        Ok(())
    })
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    // Dropping WebApp stops its frame driver and releases all listeners/tasks.
    static WEB_APP: std::cell::RefCell<Option<glory::web::WebApp>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start() -> std::result::Result<(), wasm_bindgen::JsValue> {
    WEB_APP.with(|slot| {
        if slot.borrow().is_some() {
            return Ok(());
        }
        let window = web_sys::window().ok_or("missing browser window")?;
        let document = window.document().ok_or("missing document")?;
        let root = document.get_element_by_id("main").ok_or("missing #main")?;
        let route = Route::parse(&window.location().pathname()?);
        let initial_auth = state::load_auth_state();
        if route.requires_auth() && initial_auth.token.is_none() {
            window.location().replace("/login")?;
            return Ok(());
        }
        let guard = glory::web::launch(root, app(route, initial_auth))
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))?;
        slot.replace(Some(guard));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_keep_document_links_and_reject_malformed_ids() {
        let id = "a90545b8-2dce-47ab-8ca2-a8c15d5dbdb4";
        for kind in ["documents", "writer", "calc", "slides", "whiteboard", "pdf"] {
            let url = format!("/{kind}/{id}");
            assert_eq!(Route::parse(&url).url(), url);
        }
        assert_eq!(Route::parse("/calc/no-such-id"), Route::NotFound {});
        assert_eq!(Route::parse("/files/?sort=name"), Route::FileManager {});
        assert_eq!(Route::parse("/"), Route::Home {});
    }

    #[cfg(feature = "ssr")]
    #[test]
    fn public_pages_render_real_semantic_controls() {
        let home = glory::ssr::render(app(Route::Home {}, state::AuthState::default())).unwrap();
        assert!(home.contains("href=\"/login\""));
        assert!(home.contains("href=\"/register\""));
        for route in [Route::Login {}, Route::Register {}, Route::Dashboard {}] {
            let html = glory::ssr::render(app(route, state::AuthState::default())).unwrap();
            assert!(html.contains("type=\"password\""));
            assert!(html.contains("inputmode=\"email\""));
            assert!(html.contains("<button"));
        }
    }
}
