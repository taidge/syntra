use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/assets/writer-surface.js")]
extern "C" {
    #[wasm_bindgen(js_name = createWriterSurface)]
    pub fn create_surface(selector: &str, changed: &js_sys::Function) -> JsValue;
    #[wasm_bindgen(js_name = updateWriterSurface)]
    pub fn update_surface(handle: &JsValue, document: &str, enabled: bool);
    #[wasm_bindgen(catch, js_name = formatWriterSurface)]
    pub fn format_surface(handle: &JsValue, format: &str) -> std::result::Result<(), JsValue>;
    #[wasm_bindgen(js_name = destroyWriterSurface)]
    pub fn destroy_surface(handle: &JsValue);
}
