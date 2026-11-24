//! Read-only formatting compatibility for untouched, early DOCX imports.
//!
//! This does not migrate stored content or create a revision. An old version-one
//! import can borrow formatting from its retained source only when every text
//! block still matches in order. Edited documents always use their saved model.

use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use serde_json::{Value, json};

use super::StorageBackend;
use crate::database::Database;

const MAX_ORIGINAL_BYTES: i64 = 32 * 1024 * 1024;

fn needs_compatibility(version: i32, content: &Value) -> bool {
    version == 1
        && content["body"].is_array()
        && content
            .get("professional")
            .is_none_or(|layout| layout.is_object() && layout.get("paragraph_formats").is_none())
}

/// Called only after the document's normal viewer authorization has succeeded.
/// Capacity, source I/O or parsing failures must never make a saved document
/// unavailable merely because its optional formatting could not be recovered.
pub async fn for_response(
    db: &Database,
    storage: &StorageBackend,
    file_id: &str,
    version: i32,
    content: Value,
) -> Value {
    if !needs_compatibility(version, &content) {
        return content;
    }
    static CAPACITY: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
    let Ok(permit) = CAPACITY.clone().try_acquire_owned() else {
        return content;
    };
    use syntra_core::schema::file_versions::dsl as fv;
    let source = fv::file_versions
        .filter(fv::file_id.eq(file_id))
        .filter(fv::version_number.eq(0))
        .select((fv::path, fv::size))
        .first::<(String, i64)>(&mut db.clone())
        .await
        .optional();
    let (path, size) = match source {
        Ok(Some(source)) => source,
        Ok(None) => return content,
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Writer source lookup skipped");
            return content;
        }
    };
    if !(0..=MAX_ORIGINAL_BYTES).contains(&size)
        || !std::path::Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("docx"))
    {
        return content;
    }
    let bytes = match storage.read_file(&path).await {
        Ok(bytes) if bytes.len() <= MAX_ORIGINAL_BYTES as usize => bytes,
        Ok(_) => return content,
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Writer source read skipped");
            return content;
        }
    };
    let original = match tokio::task::spawn_blocking(move || {
        // Keep the permit alive even if the request is cancelled during parsing.
        let _permit = permit;
        syntra_export::docx::docx_to_writer(&bytes)
            .and_then(|document| serde_json::to_value(document).map_err(|error| error.to_string()))
    })
    .await
    {
        Ok(Ok(original)) => original,
        Ok(Err(error)) => {
            tracing::warn!(%file_id, %error, "Legacy Writer source formatting skipped");
            return content;
        }
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Writer source parser failed");
            return content;
        }
    };
    merge_original_formatting(version, content, &original)
}

fn runs_text(runs: &Value) -> Option<String> {
    runs.as_array()?
        .iter()
        .map(|run| run["text"].as_str())
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.concat())
}

fn list_text(items: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for item in items.as_array()? {
        parts.push(runs_text(&item["runs"])?);
        if item["children"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            parts.push(list_text(&item["children"])?);
        }
    }
    Some(parts.join("\n"))
}

fn block_text(block: &Value) -> Option<String> {
    match block["type"].as_str()? {
        "Paragraph" | "Heading" => runs_text(&block["runs"]),
        "List" => list_text(&block["items"]),
        "CodeBlock" => block["code"].as_str().map(str::to_owned),
        "Blockquote" => block["blocks"]
            .as_array()?
            .iter()
            .map(block_text)
            .collect::<Option<Vec<_>>>()
            .map(|blocks| blocks.join("\n")),
        // Images/tables are preserved verbatim, never reconstructed from source.
        _ => None,
    }
}

fn compatible_blocks(current: &Value, original: &Value) -> bool {
    let current_text = block_text(current);
    let original_text = block_text(original);
    match (&current_text, &original_text) {
        (Some(left), Some(right)) if left == right => {
            current["type"] == original["type"]
                || matches!(
                    (current["type"].as_str(), original["type"].as_str()),
                    (
                        Some("List" | "Heading" | "Paragraph"),
                        Some("Paragraph" | "Heading")
                    )
                )
        }
        (None, None) => current["type"] == original["type"],
        _ => false,
    }
}

const RUN_FORMATS: &[&str] = &[
    "bold",
    "italic",
    "underline",
    "strikethrough",
    "font_family",
    "font_size",
    "color",
    "background_color",
    "link",
    "superscript",
    "subscript",
];

/// Split at both old and source Unicode character boundaries. Starting from an
/// old run preserves extension fields while each segment receives source style.
fn merge_runs(current: &[Value], original: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    let source_lengths = original
        .iter()
        .map(|run| run["text"].as_str().unwrap_or_default().chars().count())
        .collect::<Vec<_>>();
    let mut source_index = 0usize;
    let mut source_offset = 0usize;
    for old in current {
        let text = old["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .collect::<Vec<_>>();
        if text.is_empty() {
            output.push(old.clone());
            continue;
        }
        let mut offset = 0usize;
        while offset < text.len() {
            while source_index < original.len() && source_offset >= source_lengths[source_index] {
                source_index += 1;
                source_offset = 0;
            }
            let Some(source) = original.get(source_index) else {
                return current.to_vec();
            };
            let available = source_lengths[source_index] - source_offset;
            let length = available.min(text.len() - offset);
            let mut run = old.clone();
            run["text"] = json!(text[offset..offset + length].iter().collect::<String>());
            for key in RUN_FORMATS {
                if let Some(value) = source.get(*key) {
                    run[*key] = value.clone();
                }
            }
            output.push(run);
            offset += length;
            source_offset += length;
        }
    }
    output
}

fn flatten_list_runs(items: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            output.push(json!({"text":"\n"}));
        }
        output.extend(item["runs"].as_array().into_iter().flatten().cloned());
        if let Some(children) = item["children"]
            .as_array()
            .filter(|children| !children.is_empty())
        {
            output.push(json!({"text":"\n"}));
            output.extend(flatten_list_runs(children));
        }
    }
    output
}

fn merge_list_items(current: &mut Value, original: &Value) {
    if let (Some(current), Some(original)) = (current.as_array_mut(), original.as_array()) {
        if current.len() != original.len() {
            return;
        }
        for (current, original) in current.iter_mut().zip(original) {
            if runs_text(&current["runs"]) == runs_text(&original["runs"]) {
                let runs = merge_runs(
                    current["runs"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    original["runs"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                );
                current["runs"] = json!(runs);
                merge_list_items(&mut current["children"], &original["children"]);
            }
        }
    }
}

pub fn merge_original_formatting(version: i32, mut current: Value, original: &Value) -> Value {
    if !needs_compatibility(version, &current) {
        return current;
    }
    let Some(old_blocks) = current["body"].as_array() else {
        return current;
    };
    let Some(source_blocks) = original["body"].as_array() else {
        return current;
    };
    if old_blocks.len() != source_blocks.len()
        || !old_blocks
            .iter()
            .zip(source_blocks)
            .all(|(old, source)| compatible_blocks(old, source))
    {
        return current;
    }
    let mut ids = std::collections::HashMap::new();
    for (old, source) in current["body"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(source_blocks)
    {
        if let (Some(old_id), Some(source_id)) = (old["id"].as_str(), source["id"].as_str()) {
            ids.insert(source_id.to_owned(), old_id.to_owned());
        }
        match source["type"].as_str() {
            Some("Paragraph" | "Heading") => {
                let old_runs = if let Some(runs) = old["runs"].as_array() {
                    runs.clone()
                } else {
                    flatten_list_runs(
                        old["items"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                    )
                };
                old["runs"] = json!(merge_runs(
                    &old_runs,
                    source["runs"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                ));
                for key in ["type", "alignment", "indent_level", "spacing", "level"] {
                    if let Some(value) = source.get(key) {
                        old[key] = value.clone();
                    }
                }
                if let Some(object) = old.as_object_mut() {
                    object.remove("items");
                    object.remove("ordered");
                    if source["type"] != "Heading" {
                        object.remove("level");
                    }
                }
            }
            Some("List") => merge_list_items(&mut old["items"], &source["items"]),
            _ => {}
        }
    }
    let formats = original
        .pointer("/professional/paragraph_formats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|format| {
            let id = ids.get(format["block_id"].as_str()?)?;
            let mut format = format.clone();
            format["block_id"] = json!(id);
            Some(format)
        })
        .collect::<Vec<_>>();
    if current.get("professional").is_none() {
        current["professional"] = json!({});
    }
    current["professional"]["paragraph_formats"] = json!(formats);
    if current["styles"].as_array().is_some_and(Vec::is_empty) {
        if let Some(styles) = original.get("styles") {
            current["styles"] = styles.clone();
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy() -> Value {
        json!({"body":[{"type":"Paragraph","id":"old","alignment":"Left","indent_level":0,
            "spacing":{"before":0,"after":8,"line_height":1.15},"extension":"kept",
            "runs":[{"text":"A世界B","font_family":null,"bold":false,"custom_run":7}]}],
            "professional":{"header":"Private"},"styles":[],"custom_document":true})
    }
    fn original() -> Value {
        json!({"body":[{"type":"Paragraph","id":"source","alignment":"Center","indent_level":0,
            "spacing":{"before":0,"after":0,"line_height":1.5},
            "runs":[{"text":"A世","font_family":"宋体","font_size":12,"bold":false},
            {"text":"界B","font_family":"宋体","font_size":16,"bold":true}]}],
            "professional":{"paragraph_formats":[{"block_id":"source","first_line_indent_chars":2}]},
            "styles":[{"name":"Normal","font_family":"宋体"}]})
    }

    #[test]
    fn style_recovery_preserves_text_identity_and_extensions() {
        let recovered = merge_original_formatting(1, legacy(), &original());
        let block = &recovered["body"][0];
        assert_eq!(runs_text(&block["runs"]).as_deref(), Some("A世界B"));
        assert_eq!(block["id"], "old");
        assert_eq!(block["extension"], "kept");
        assert_eq!(block["alignment"], "Center");
        assert_eq!(block["runs"][0]["text"], "A世");
        assert_eq!(block["runs"][1]["text"], "界B");
        assert_eq!(block["runs"][1]["font_size"], 16);
        assert_eq!(block["runs"][1]["bold"], true);
        assert_eq!(block["runs"][0]["custom_run"], 7);
        assert_eq!(block["runs"][1]["custom_run"], 7);
        assert_eq!(recovered["professional"]["header"], "Private");
        assert_eq!(
            recovered["professional"]["paragraph_formats"][0]["block_id"],
            "old"
        );
        assert_eq!(recovered["custom_document"], true);
    }

    #[test]
    fn edited_mismatched_and_already_formatted_documents_are_unchanged() {
        let current = legacy();
        assert_eq!(
            merge_original_formatting(2, current.clone(), &original()),
            current
        );
        let mut changed = current.clone();
        changed["body"][0]["runs"][0]["text"] = json!("Edited");
        assert_eq!(
            merge_original_formatting(1, changed.clone(), &original()),
            changed
        );
        let mut formatted = current;
        formatted["professional"]["paragraph_formats"] = json!([]);
        assert_eq!(
            merge_original_formatting(1, formatted.clone(), &original()),
            formatted
        );
    }

    #[test]
    fn canceled_numbering_recovers_a_paragraph_without_replacing_its_identity() {
        let mut current = legacy();
        current["body"][0] = json!({"type":"List","id":"old","ordered":false,
            "items":[{"runs":[{"text":"A世界B","custom_run":"kept"}],"children":[]}],"extension":"kept"});
        let recovered = merge_original_formatting(1, current, &original());
        assert_eq!(recovered["body"][0]["type"], "Paragraph");
        assert_eq!(recovered["body"][0]["id"], "old");
        assert_eq!(
            runs_text(&recovered["body"][0]["runs"]).as_deref(),
            Some("A世界B")
        );
        assert!(recovered["body"][0].get("items").is_none());
        assert_eq!(recovered["body"][0]["runs"][1]["custom_run"], "kept");
    }

    #[test]
    fn source_non_text_objects_never_replace_saved_objects() {
        let mut current = legacy();
        let saved_table = json!({"type":"Table","id":"saved-table","rows":[],"custom_table":9});
        current["body"]
            .as_array_mut()
            .unwrap()
            .push(saved_table.clone());
        let mut source = original();
        source["body"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"Table","id":"source-table","rows":[{"cells":[]}]}));
        let recovered = merge_original_formatting(1, current, &source);
        assert_eq!(recovered["body"][1], saved_table);
        assert_eq!(recovered["body"][0]["runs"][0]["font_family"], "宋体");
    }
}
