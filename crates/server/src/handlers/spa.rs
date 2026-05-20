use salvo::prelude::*;

#[handler]
pub async fn spa_fallback(req: &mut Request, res: &mut Response) {
    let path = req.uri().path().to_string();

    // Try to serve static files first (skip directories)
    let static_path = format!("static{}", path);
    let static_p = std::path::Path::new(&static_path);
    if static_p.exists() && !static_p.is_dir() {
        match tokio::fs::read(&static_path).await {
            Ok(content) => {
                let ct = if path.ends_with(".css") {
                    "text/css; charset=utf-8"
                } else if path.ends_with(".js") {
                    "application/javascript; charset=utf-8"
                } else if path.ends_with(".png") {
                    "image/png"
                } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path.ends_with(".svg") {
                    "image/svg+xml"
                } else if path.ends_with(".ico") {
                    "image/x-icon"
                } else if path.ends_with(".woff2") {
                    "font/woff2"
                } else if path.ends_with(".woff") {
                    "font/woff"
                } else if path.ends_with(".json") {
                    "application/json"
                } else if path.ends_with(".webmanifest") {
                    "application/manifest+json"
                } else if path.ends_with(".wasm") {
                    "application/wasm"
                } else if path.ends_with(".mjs") {
                    "application/javascript; charset=utf-8"
                } else if path.ends_with(".html") {
                    "text/html; charset=utf-8"
                } else {
                    "application/octet-stream"
                };
                res.headers_mut()
                    .insert("content-type", ct.parse().unwrap());
                res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(
                    content,
                )));
                return;
            }
            Err(_) => {}
        }
    }

    // SPA fallback: serve index.html for all non-file routes
    match tokio::fs::read("static/index.html").await {
        Ok(content) => {
            res.headers_mut()
                .insert("content-type", "text/html; charset=utf-8".parse().unwrap());
            res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(
                content,
            )));
        }
        Err(_) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render("Not Found");
        }
    }
}
