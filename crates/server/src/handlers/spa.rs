use salvo::prelude::*;
use std::path::{Component, Path};

#[handler]
pub async fn spa_fallback(req: &mut Request, res: &mut Response) {
    let path = req.uri().path().to_string();
    let static_root = std::env::var("CONTRIX_STATIC_DIR").unwrap_or_else(|_| "static".into());
    serve_spa(Path::new(&static_root), &path, res).await;
}

async fn serve_spa(static_root: &Path, path: &str, res: &mut Response) {
    // URI paths are untrusted even when the router matched a catch-all. Reject
    // platform separators/prefixes as well as dot segments before touching disk.
    let relative = path.trim_start_matches('/');
    if path.contains(['\\', ':', '\0'])
        || relative.split('/').any(|part| matches!(part, "." | ".."))
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        res.status_code(StatusCode::NOT_FOUND);
        return;
    }
    let root = match tokio::fs::canonicalize(static_root).await {
        Ok(root) => root,
        Err(_) => {
            res.status_code(StatusCode::NOT_FOUND);
            return;
        }
    };
    if let Some(content) = read_contained_file(&root, &root.join(relative)).await {
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

    // Asset fetches must fail explicitly; returning HTML for a missing script
    // or WASM module disguises an incomplete deployment as HTTP success.
    if Path::new(relative).extension().is_some()
        || relative.starts_with("assets/")
        || relative.starts_with("icons/")
    {
        res.status_code(StatusCode::NOT_FOUND);
        return;
    }

    // SPA fallback: serve index.html for non-file routes.
    match read_contained_file(&root, &root.join("index.html")).await {
        Some(content) => {
            res.headers_mut()
                .insert("content-type", "text/html; charset=utf-8".parse().unwrap());
            res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(
                content,
            )));
        }
        None => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render("Not Found");
        }
    }
}

async fn read_contained_file(root: &Path, candidate: &Path) -> Option<Vec<u8>> {
    // Check real paths so a symlink/junction inside static cannot expose files
    // outside the configured static root. Read the checked canonical path.
    let target = tokio::fs::canonicalize(candidate).await.ok()?;
    if !target.starts_with(root) || !tokio::fs::metadata(&target).await.ok()?.is_file() {
        return None;
    }
    tokio::fs::read(target).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use salvo::conn::Acceptor;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct TestStaticRoot(std::path::PathBuf);

    #[handler]
    impl TestStaticRoot {
        async fn handle(&self, req: &mut Request, res: &mut Response) {
            serve_spa(&self.0, req.uri().path(), res).await;
        }
    }

    #[tokio::test]
    async fn raw_http_dot_segments_cannot_escape_static_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("public");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(root.join("index.html"), "safe-index")
            .await
            .unwrap();
        tokio::fs::write(root.join("app.js"), "safe-asset")
            .await
            .unwrap();
        tokio::fs::write(directory.path().join("private.txt"), "outside-secret")
            .await
            .unwrap();
        let router = Router::with_path("{**rest}").get(TestStaticRoot(root));
        let acceptor = TcpListener::new("127.0.0.1:0").bind().await;
        let address = acceptor.holdings()[0]
            .local_addr
            .clone()
            .into_std()
            .unwrap();
        let server = tokio::spawn(Server::new(acceptor).serve(router));
        // This loopback relay forwards the exact request without normalizing it.
        let relay = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_address = relay.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            while let Ok((mut client, _)) = relay.accept().await {
                let mut upstream = tokio::net::TcpStream::connect(address).await.unwrap();
                tokio::io::copy_bidirectional(&mut client, &mut upstream)
                    .await
                    .unwrap();
            }
        });
        for endpoint in [address, proxy_address] {
            for (path, status, body_marker) in [
                ("/../private.txt", Some("404"), None),
                ("/assets/../../private.txt", Some("404"), None),
                ("/assets\\..\\..\\private.txt", None, None),
                ("/C:/private.txt", None, None),
                ("/%2e%2e/private.txt", None, None),
                ("/..%2fprivate.txt", None, None),
                ("/..%5cprivate.txt", None, None),
                ("/%252e%252e/private.txt", None, None),
                ("/app.js", Some("200"), Some("safe-asset")),
                ("/missing.js", Some("404"), None),
                ("/assets/missing.wasm", Some("404"), None),
                ("/icons/missing", Some("404"), None),
                ("/writer/document-id", Some("200"), Some("safe-index")),
            ] {
                let mut socket = tokio::net::TcpStream::connect(endpoint).await.unwrap();
                socket
                    .write_all(
                        format!(
                            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                let mut response = Vec::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    socket.read_to_end(&mut response),
                )
                .await
                .unwrap()
                .unwrap();
                let response = String::from_utf8(response).unwrap();
                let (head, body) = response.split_once("\r\n\r\n").unwrap();
                let received = head.lines().next().unwrap().split_whitespace().nth(1);
                if let Some(status) = status {
                    assert_eq!(received, Some(status), "{endpoint} {path}: {head}");
                } else {
                    assert!(
                        matches!(received, Some("400" | "404")),
                        "{endpoint} {path}: {head}"
                    );
                }
                if let Some(marker) = body_marker {
                    assert!(body.contains(marker), "{endpoint} {path}: {body}");
                }
                assert!(!response.contains("outside-secret"), "{endpoint} {path}");
            }
        }
        proxy.abort();
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_outside_static_root_is_not_read() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("public");
        tokio::fs::create_dir(&root).await.unwrap();
        let secret = directory.path().join("private.txt");
        tokio::fs::write(&secret, "outside-secret").await.unwrap();
        std::os::unix::fs::symlink(&secret, root.join("leak.txt")).unwrap();
        let root = tokio::fs::canonicalize(root).await.unwrap();
        assert!(
            read_contained_file(&root, &root.join("leak.txt"))
                .await
                .is_none()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn junction_outside_static_root_is_not_read() {
        use std::os::windows::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("public");
        let private = directory.path().join("private");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::create_dir(&private).await.unwrap();
        tokio::fs::write(private.join("secret.txt"), "outside-secret")
            .await
            .unwrap();
        let junction = root.join("leak");
        // Junction creation needs no symlink privilege. Pass paths as process
        // environment values, so metacharacters never become shell code.
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let result = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command",
                "New-Item -ItemType Junction -Path $env:OFFICE_TEST_JUNCTION -Value $env:OFFICE_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("OFFICE_TEST_JUNCTION", &junction)
            .env("OFFICE_TEST_TARGET", &private)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Junction creation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let canonical_root = tokio::fs::canonicalize(&root).await.unwrap();
        assert!(
            read_contained_file(&canonical_root, &junction.join("secret.txt"))
                .await
                .is_none()
        );
        // Remove only the link entry, without recursive traversal of its target.
        tokio::fs::remove_dir(&junction).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(private.join("secret.txt"))
                .await
                .unwrap(),
            "outside-secret"
        );
    }
}
