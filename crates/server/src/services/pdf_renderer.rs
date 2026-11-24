//! Sandboxed browser printing of self-contained, application-generated HTML.
//! This never navigates to a user URL or grants access to arbitrary local files.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_HTML_BYTES: usize = 64 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
const PRINT_TIMEOUT: Duration = Duration::from_secs(35);

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn browser_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SYNTRA_PDF_BROWSER")
        .or_else(|| std::env::var_os("\x43ONTRIX_PDF_BROWSER"))
    {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path).ok_or_else(|| {
            "SYNTRA_PDF_BROWSER must name an installed Chrome, Chromium or Edge executable".into()
        });
    }
    let names = if cfg!(windows) {
        vec!["chrome.exe", "msedge.exe", "chromium.exe"]
    } else {
        vec![
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "microsoft-edge",
        ]
    };
    for name in names {
        if let Some(path) = on_path(name) {
            return Ok(path);
        }
    }
    for path in [
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
        "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
        "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
        "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ] {
        if Path::new(path).is_file() {
            return Ok(path.into());
        }
    }
    // Playwright's installed browser is useful in CI and developer installs.
    // Reading these standard locations does not redirect any cache or temp path.
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").filter(|path| path != "0") {
        roots.push(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(path).join("ms-playwright"));
    }
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        roots.push(PathBuf::from(path).join("ms-playwright"));
    }
    if let Some(path) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(path).join(".cache/ms-playwright"));
    }
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut versions = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_prefix("chromium-")?
                    .parse::<u32>()
                    .ok()
                    .map(|version| (version, entry.path()))
            })
            .collect::<Vec<_>>();
        versions.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        for (_, directory) in versions {
            for relative in [
                "chrome-win/chrome.exe",
                "chrome-win64/chrome.exe",
                "chrome-linux/chrome",
                "chrome-linux64/chrome",
                "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
            ] {
                let path = directory.join(relative);
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err("PDF export requires Chrome, Chromium or Edge on the server. Install a supported browser or set SYNTRA_PDF_BROWSER to its executable.".into())
}

fn bundled_font() -> Option<Vec<u8>> {
    let root = std::env::var_os("SYNTRA_STATIC_DIR")
        .or_else(|| std::env::var_os("\x43ONTRIX_STATIC_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| "static".into());
    let path = root.join("assets/pdf-vendor/syntra-cjk.ttf");
    let size = fs::metadata(&path).ok()?.len();
    if size == 0 || size > 32 * 1024 * 1024 {
        return None;
    }
    fs::read(path).ok()
}

fn diagnostic(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let _ = file.seek(SeekFrom::End(-8192));
    let mut bytes = Vec::new();
    let _ = file.take(8192).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

/// Called inside the conversion worker, which holds a slot until this process
/// has exited, even when the HTTP request is cancelled.
pub fn render(html: &str) -> Result<Vec<u8>, String> {
    if html.len() > MAX_HTML_BYTES {
        return Err("Printable document exceeds the PDF rendering budget".into());
    }
    let browser = browser_path()?;
    let directory = tempfile::Builder::new()
        .prefix("syntra-pdf-")
        .tempdir()
        .map_err(|error| format!("Could not create PDF workspace: {error}"))?;
    let input = directory.path().join("document.html");
    let output = directory.path().join("document.pdf");
    let log_path = directory.path().join("browser.log");
    let mut html = html.to_owned();
    if let Some(font) = bundled_font() {
        let css = format!(
            "<style>@font-face{{font-family:'Office CJK';src:url(data:font/otf;base64,{});font-weight:100 900;font-style:normal;}}</style>",
            STANDARD.encode(font)
        );
        if let Some(position) = html.find("</head>") {
            html.insert_str(position, &css);
        }
    }
    if html.len() > MAX_HTML_BYTES {
        return Err(
            "Printable document including embedded fonts exceeds the PDF rendering budget".into(),
        );
    }
    fs::write(&input, html).map_err(|error| format!("Could not prepare PDF document: {error}"))?;
    let file_url =
        reqwest::Url::from_file_path(&input).map_err(|_| "Invalid PDF workspace path")?;
    let mut print_argument = OsString::from("--print-to-pdf=");
    print_argument.push(output.as_os_str());
    let mut profile_argument = OsString::from("--user-data-dir=");
    profile_argument.push(directory.path().join("profile"));
    let mut command = Command::new(browser);
    command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--no-pdf-header-footer",
            "--print-background",
            "--force-color-profile=srgb",
            "--host-resolver-rules=MAP * ~NOTFOUND",
            "--timeout=25000",
            "--virtual-time-budget=3000",
        ])
        .arg(print_argument)
        .arg(profile_argument)
        .arg(file_url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log_path).map_err(|error| error.to_string())?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start PDF renderer: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    tracing::warn!(diagnostic = %diagnostic(&log_path), "PDF browser exited unsuccessfully");
                    return Err("PDF renderer failed. Check the browser installation and sandbox support on the server.".into());
                }
                break;
            }
            Ok(None) if started.elapsed() < PRINT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("PDF rendering timed out; reduce the document size and retry".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("PDF renderer process failed: {error}"));
            }
        }
    }
    let size = fs::metadata(&output)
        .map_err(|_| {
            tracing::warn!(diagnostic = %diagnostic(&log_path), "PDF browser produced no document");
            "PDF renderer did not produce a document".to_owned()
        })?
        .len();
    if size == 0 || size > MAX_PDF_BYTES {
        return Err("PDF output exceeds the rendering budget".into());
    }
    let bytes = fs::read(output).map_err(|error| error.to_string())?;
    if !bytes.starts_with(b"%PDF-") {
        return Err("PDF renderer returned an invalid document".into());
    }
    Ok(bytes)
}
