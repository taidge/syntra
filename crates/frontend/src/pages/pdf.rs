//! PDF bytes stay in the PDF editor; they never pass through the Writer model.
use crate::{components::navbar::Navbar, state::AuthState};
use glory::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(raw_module = "/assets/pdf-editor.js")]
extern "C" {
    #[wasm_bindgen(js_name = mountPdfEditor)]
    fn mount_pdf_editor(selector: &str, file_id: &str, token: &str) -> JsValue;
    #[wasm_bindgen(js_name = destroyPdfEditor)]
    fn destroy_pdf_editor(handle: &JsValue);
}

#[allow(non_snake_case)]
pub fn Pdf(_scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    widget(move |scope| {
        let identity = format!("pdf-editor-{}", uuid::Uuid::new_v4().simple());
        #[cfg(target_arch = "wasm32")]
        {
            let handle = mount_pdf_editor(
                &format!(".{identity}"),
                &id,
                &auth.get().token.unwrap_or_default(),
            );
            let _ = scope.on_cleanup(move || destroy_pdf_editor(&handle));
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = &id;
        col()
            .class("pdf-app")
            .fill((
                Navbar(scope.clone(), auth),
                view().class(format!("pdf-editor-host {identity}")),
            ))
            .show_in(scope)
    })
}
