fn main() {
    #[cfg(target_arch = "wasm32")]
    if let Err(error) = syntra_frontend::start() {
        web_sys::console::error_1(&error);
    }
    #[cfg(not(target_arch = "wasm32"))]
    println!("Build the browser app with ./scripts/build-frontend.ps1");
}
