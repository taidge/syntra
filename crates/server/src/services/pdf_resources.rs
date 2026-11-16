//! Resolve only authenticated application image links before offline printing.
use crate::{database::Database, handlers::access::load_accessible_file, services::StorageBackend};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

/// Use the same recovered layout as the editor for untouched Office imports.
pub async fn prepare_document(
    db: &Database,
    storage: &StorageBackend,
    user: &str,
    file_id: &str,
    version: i32,
    kind: &str,
    serialized: String,
) -> Result<String, String> {
    let mut content: Value = serde_json::from_str(&serialized)
        .map_err(|error| format!("Invalid document content: {error}"))?;
    content = match kind {
        "writer" => {
            super::legacy_writer_layout::for_response(db, storage, file_id, version, content).await
        }
        "slides" => {
            super::legacy_slides_layout::for_response(db, storage, file_id, version, content).await
        }
        _ => content,
    };
    inline_images(db, storage, user, &mut content).await?;
    serde_json::to_string(&content).map_err(|error| error.to_string())
}

fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn image_links(value: &Value, path: &str, output: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("Image") {
                if let Some(url) = object
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|url| !url.starts_with("data:"))
                {
                    output.push((format!("{path}/url"), url.into()));
                }
            }
            // Background is an externally tagged enum, not a drawing element.
            if let Some(url) = object
                .get("background")
                .and_then(|background| background.get("Image"))
                .and_then(Value::as_str)
                .filter(|url| !url.starts_with("data:"))
            {
                output.push((format!("{path}/background/Image"), url.into()));
            }
            for (key, value) in object {
                image_links(value, &format!("{path}/{}", escape_pointer(key)), output);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                image_links(value, &format!("{path}/{index}"), output);
            }
        }
        _ => {}
    }
}

pub async fn inline_images(
    db: &Database,
    storage: &StorageBackend,
    user: &str,
    content: &mut Value,
) -> Result<(), String> {
    let mut links = Vec::new();
    image_links(content, "", &mut links);
    if links.len() > 512 {
        return Err("Too many linked images for one PDF export".into());
    }
    let mut total = 0usize;
    let mut cache = std::collections::HashMap::<String, String>::new();
    for (pointer, url) in links {
        let encoded = if let Some(value) = cache.get(&url) {
            value.clone()
        } else {
            let path = url.split(['?', '#']).next().unwrap_or_default();
            let id = path.strip_prefix("/api/v1/files/").and_then(|tail| tail.strip_suffix("/download")).and_then(|id| uuid::Uuid::parse_str(id).ok()).ok_or_else(|| "PDF images must be embedded or use an authenticated /api/v1/files/{id}/download link; external image URLs are not fetched by the server".to_owned())?;
            let row = load_accessible_file(&mut db.clone(), &id.to_string(), user, "viewer")
                .await
                .map_err(|error| error.to_string())?
                .filter(|file| !file.is_folder && !file.is_trashed)
                .ok_or("PDF image is missing or access is denied")?;
            if row.size < 0 || row.size > 8 * 1024 * 1024 {
                return Err("Linked PDF image exceeds the 8 MiB limit".into());
            }
            let bytes = storage
                .read_file(&row.path)
                .await
                .map_err(|error| error.to_string())?;
            total = total
                .checked_add(bytes.len())
                .ok_or("PDF image size overflow")?;
            if bytes.len() > 8 * 1024 * 1024 || total > 32 * 1024 * 1024 {
                return Err("Linked PDF images exceed the export budget".into());
            }
            let mime = match image::guess_format(&bytes)
                .map_err(|_| "Linked PDF image is not a supported bitmap")?
            {
                image::ImageFormat::Png => "image/png",
                image::ImageFormat::Jpeg => "image/jpeg",
                image::ImageFormat::Gif => "image/gif",
                _ => return Err("Linked PDF image must be PNG, JPEG or GIF".into()),
            };
            let value = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
            cache.insert(url.clone(), value.clone());
            value
        };
        *content
            .pointer_mut(&pointer)
            .ok_or("PDF image location changed during export")? = Value::String(encoded);
    }
    Ok(())
}
