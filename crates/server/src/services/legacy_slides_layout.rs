//! Read-only recovery of untouched PPTX imports made before media/layout fixes.
//!
//! Version one plus a retained PPTX source identifies an unedited import. A
//! matching object/text sequence is also required before restoring source media.
//! The saved revision is never rewritten, and a failed optional recovery cannot
//! make the existing document unavailable.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::StorageBackend;
use crate::database::Database;

const MAX_ORIGINAL_BYTES: i64 = 32 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 4;

fn needs_compatibility(version: i32, content: &Value) -> bool {
    version == 1
        && content["slides"].is_array()
        && content["metadata"]["pptx_import_revision"]
            .as_u64()
            .unwrap_or_default()
            < 2
}

#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    file_id: String,
    source_id: String,
    path: String,
    size: i64,
    content_hash: [u8; 32],
}

struct CachedRecovery {
    key: CacheKey,
    content: Arc<Value>,
    bytes: usize,
}

#[derive(Default)]
struct RecoveryCache {
    entries: VecDeque<CachedRecovery>,
    bytes: usize,
}

impl RecoveryCache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<Value>> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let content = Arc::clone(&entry.content);
        self.entries.push_back(entry);
        Some(content)
    }

    fn insert(&mut self, key: CacheKey, content: Value, bytes: usize) {
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            if let Some(old) = self.entries.remove(index) {
                self.bytes -= old.bytes;
            }
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES || self.bytes + bytes > MAX_CACHE_BYTES {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(CachedRecovery {
            key,
            content: Arc::new(content),
            bytes,
        });
    }
}

static CACHE: LazyLock<Mutex<RecoveryCache>> =
    LazyLock::new(|| Mutex::new(RecoveryCache::default()));
static CAPACITY: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

/// Called only after normal viewer authorization, including on a cache hit.
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
    use contrix_core::schema::file_versions::dsl as fv;
    let source = fv::file_versions
        .filter(fv::file_id.eq(file_id))
        .filter(fv::version_number.eq(0))
        .select((fv::id, fv::path, fv::size))
        .first::<(String, String, i64)>(&mut db.clone())
        .await
        .optional();
    let (source_id, path, size) = match source {
        Ok(Some(source)) => source,
        Ok(None) => return content,
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Slides source lookup skipped");
            return content;
        }
    };
    if !(0..=MAX_ORIGINAL_BYTES).contains(&size)
        || !std::path::Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"))
    {
        return content;
    }
    let serialized = match serde_json::to_vec(&content) {
        Ok(serialized) if serialized.len() <= contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES => {
            serialized
        }
        _ => return content,
    };
    let key = CacheKey {
        file_id: file_id.into(),
        source_id,
        path: path.clone(),
        size,
        content_hash: Sha256::digest(&serialized).into(),
    };
    drop(serialized);
    let cached = CACHE.lock().ok().and_then(|mut cache| cache.get(&key));
    if let Some(cached) = cached {
        return (*cached).clone();
    }
    let Ok(permit) = CAPACITY.clone().try_acquire_owned() else {
        return content;
    };
    let bytes = match storage.read_file(&path).await {
        Ok(bytes) if bytes.len() <= MAX_ORIGINAL_BYTES as usize => bytes,
        Ok(_) => return content,
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Slides source read skipped");
            return content;
        }
    };
    let original = match tokio::task::spawn_blocking(move || {
        // Cancellation does not release import capacity until decoding ends.
        let _permit = permit;
        contrix_export::pptx::pptx_to_presentation(&bytes)
            .and_then(|document| serde_json::to_value(document).map_err(|error| error.to_string()))
    })
    .await
    {
        Ok(Ok(original)) => original,
        Ok(Err(error)) => {
            tracing::warn!(%file_id, %error, "Legacy Slides source recovery skipped");
            return content;
        }
        Err(error) => {
            tracing::warn!(%file_id, %error, "Legacy Slides source parser failed");
            return content;
        }
    };
    let Some(recovered) = merge_original_layout(version, &content, &original) else {
        return content;
    };
    let bytes = match serde_json::to_vec(&recovered) {
        Ok(serialized) if serialized.len() <= contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES => {
            serialized.len()
        }
        _ => return content,
    };
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(key, recovered.clone(), bytes);
    }
    recovered
}

fn runs_text(runs: &Value) -> Option<String> {
    if runs.is_null() {
        return Some(String::new());
    }
    runs.as_array()?
        .iter()
        .map(|run| run["text"].as_str())
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.concat())
}

fn same_model_number(left: &Value, right: &Value) -> bool {
    let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
        return false;
    };
    // Geometry/crop belongs to the f32 document model. SQLite JSON has passed
    // through an f64 parser while freshly imported values were widened directly
    // from f32; f64 Numbers can differ by one extra rounding of the same model.
    let (model_left, model_right) = (left as f32, right as f32);
    left.is_finite()
        && right.is_finite()
        && model_left.is_finite()
        && model_right.is_finite()
        && model_left.to_bits() == model_right.to_bits()
}

fn same_geometry(current: &Value, source: &Value) -> bool {
    ["x", "y", "width", "height", "rotation"].iter().all(|key| {
        match (current.get(key), source.get(key)) {
            // Groups have no geometry, and Video has no rotation field.
            (None, None) => true,
            (Some(left), Some(right)) => same_model_number(left, right),
            _ => false,
        }
    })
}

fn same_crop(current: &Value, source: &Value) -> bool {
    match (current.get("crop"), source.get("crop")) {
        (None, None) | (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Object(current)), Some(Value::Object(source))) => {
            ["left", "right", "top", "bottom"].iter().all(|key| {
                match (current.get(*key), source.get(*key)) {
                    (Some(left), Some(right)) => same_model_number(left, right),
                    _ => false,
                }
            })
        }
        _ => false,
    }
}

fn same_image(current: &Value, source: &Value) -> bool {
    current["type"] == "Image"
        && same_geometry(current, source)
        && current["alt_text"] == source["alt_text"]
        && same_crop(current, source)
}

/// Overlay source fields while keeping unknown saved extension fields.
fn overlay(current: &Value, source: &Value) -> Value {
    match (current.as_object(), source.as_object()) {
        (Some(old), Some(new)) => {
            let mut output = old.clone();
            for (key, value) in new {
                output.insert(
                    key.clone(),
                    old.get(key)
                        .map(|old| overlay(old, value))
                        .unwrap_or_else(|| value.clone()),
                );
            }
            Value::Object(output)
        }
        _ if current
            .as_array()
            .is_some_and(|old| source.as_array().is_some_and(|new| old.len() == new.len())) =>
        {
            Value::Array(
                current
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(source.as_array().unwrap())
                    .map(|(old, new)| overlay(old, new))
                    .collect(),
            )
        }
        _ => source.clone(),
    }
}

fn preserve_id(
    current: &Value,
    source: &Value,
    output: &mut Value,
    ids: &mut HashMap<String, String>,
) {
    if let (Some(old_id), Some(source_id)) = (current["id"].as_str(), source["id"].as_str()) {
        ids.insert(source_id.into(), old_id.into());
        output["id"] = Value::String(old_id.into());
    }
}

fn stable_image_id(
    presentation_id: &str,
    slide_index: usize,
    path: &[usize],
    image: &Value,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"contrix-legacy-pptx-image-v1\0");
    hash.update(presentation_id.as_bytes());
    hash.update([0]);
    hash.update((slide_index as u64).to_le_bytes());
    for index in path {
        hash.update((*index as u64).to_le_bytes());
    }
    for key in ["x", "y", "width", "height", "rotation", "alt_text"] {
        hash.update([0]);
        hash.update(key.as_bytes());
        if let Ok(value) = serde_json::to_vec(&image[key]) {
            hash.update(value);
        }
    }
    let digest = hash.finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    // UUIDv8 permits a custom deterministic layout. Source random IDs cannot
    // change the version-one response after cache eviction or server restart.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn merge_elements(
    current: &[Value],
    source: &[Value],
    ids: &mut HashMap<String, String>,
    presentation_id: &str,
    slide_index: usize,
    parent_path: &[usize],
) -> Option<Vec<Value>> {
    let mut output = Vec::with_capacity(source.len());
    let mut old_index = 0;
    for (source_index, source) in source.iter().enumerate() {
        let mut path = parent_path.to_vec();
        path.push(source_index);
        let current = current.get(old_index);
        if source["type"] == "Image" && !current.is_some_and(|current| same_image(current, source))
        {
            // Only source images may be added; every existing object must still
            // match in sequence, so an unexpected saved object aborts recovery.
            let stable_id = stable_image_id(presentation_id, slide_index, &path, source);
            let mut added = source.clone();
            if let Some(source_id) = source["id"].as_str() {
                ids.insert(source_id.into(), stable_id.clone());
            }
            added["id"] = Value::String(stable_id);
            output.push(added);
            continue;
        }
        let current = current?;
        if current["type"] != source["type"] || !same_geometry(current, source) {
            tracing::debug!(slide = slide_index, element_path = ?path,
                "Legacy Slides recovery skipped: object type or geometry differs");
            return None;
        }
        let mut merged = overlay(current, source);
        match source["type"].as_str()? {
            "TextBox" if runs_text(&current["content"])? != runs_text(&source["content"])? => {
                tracing::debug!(slide = slide_index, element_path = ?path,
                    "Legacy Slides recovery skipped: text differs");
                return None;
            }
            "Shape"
                if current["shape_type"] != source["shape_type"]
                    || runs_text(&current["text"])? != runs_text(&source["text"])? =>
            {
                tracing::debug!(slide = slide_index, element_path = ?path,
                    "Legacy Slides recovery skipped: shape type or text differs");
                return None;
            }
            "Chart"
                if current["chart_type"] != source["chart_type"]
                    || current["data"] != source["data"] =>
            {
                tracing::debug!(slide = slide_index, element_path = ?path,
                    "Legacy Slides recovery skipped: chart differs");
                return None;
            }
            "Group" => {
                merged["elements"] = Value::Array(merge_elements(
                    current["elements"].as_array()?,
                    source["elements"].as_array()?,
                    ids,
                    presentation_id,
                    slide_index,
                    &path,
                )?);
            }
            "Video" if current["url"] != source["url"] => return None,
            "TextBox" | "Shape" | "Chart" | "Video" | "Image" => {}
            _ => return None,
        }
        preserve_id(current, source, &mut merged, ids);
        output.push(merged);
        old_index += 1;
    }
    (old_index == current.len()).then_some(output)
}

fn remap_animation_targets(value: &mut Value, ids: &HashMap<String, String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "target_id" {
                    if let Some(id) = value.as_str().and_then(|id| ids.get(id)) {
                        *value = Value::String(id.clone());
                    }
                } else {
                    remap_animation_targets(value, ids);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                remap_animation_targets(value, ids);
            }
        }
        _ => {}
    }
}

fn merge_original_layout(version: i32, current: &Value, source: &Value) -> Option<Value> {
    if !needs_compatibility(version, current) {
        return None;
    }
    let old_slides = current["slides"].as_array()?;
    let source_slides = source["slides"].as_array()?;
    if old_slides.len() != source_slides.len() {
        tracing::debug!("Legacy Slides recovery skipped: slide count differs");
        return None;
    }
    let mut output = overlay(current, source);
    let mut ids = HashMap::new();
    preserve_id(current, source, &mut output, &mut ids);
    for key in ["title"] {
        if let Some(value) = current.get(key) {
            output[key] = value.clone();
        }
    }
    for key in ["created_at", "modified_at"] {
        if let Some(value) = current["metadata"].get(key) {
            output["metadata"][key] = value.clone();
        }
    }
    // These enum/optional fields replace their old representation as a whole;
    // merging an old Solid variant into a new Image would make invalid JSON.
    if let Some(background) = source["master"].get("background") {
        output["master"]["background"] = background.clone();
    }
    if source["metadata"].get("import_warnings").is_none() {
        if let Some(metadata) = output["metadata"].as_object_mut() {
            metadata.remove("import_warnings");
        }
    }
    let mut slides = Vec::with_capacity(old_slides.len());
    let presentation_id = current["id"].as_str()?;
    for (slide_index, (current, source)) in old_slides.iter().zip(source_slides).enumerate() {
        if current["notes"] != source["notes"] || current["layout"] != source["layout"] {
            tracing::debug!(
                slide = slide_index,
                "Legacy Slides recovery skipped: notes or layout differs"
            );
            return None;
        }
        let mut merged = overlay(current, source);
        if let Some(background) = source.get("background") {
            merged["background"] = background.clone();
        }
        merged["elements"] = Value::Array(merge_elements(
            current["elements"].as_array()?,
            source["elements"].as_array()?,
            &mut ids,
            presentation_id,
            slide_index,
            &[],
        )?);
        preserve_id(current, source, &mut merged, &mut ids);
        slides.push(merged);
    }
    output["slides"] = Value::Array(slides);
    remap_animation_targets(&mut output["slides"], &ids);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(id: &str, text: &str) -> Value {
        json!({"type":"TextBox","id":id,"x":10,"y":20,"width":200,"height":60,
            "rotation":0,"content":[{"text":text,"font_size":18}],"custom_element":7})
    }

    fn picture(id: &str, alt: &str) -> Value {
        json!({"type":"Image","id":id,"x":0,"y":0,"width":1280,"height":720,
            "rotation":0,"alt_text":alt,"url":"data:image/png;base64,old","crop":null})
    }

    fn presentation() -> Value {
        json!({"id":"saved-presentation","title":"Saved title","custom_document":true,
            "metadata":{"created_at":"old","modified_at":"old","custom_metadata":9},
            "slides":[{"id":"saved-slide","notes":"Notes","layout":"Blank",
                "background":{"Solid":"#FFFFFF"},"custom_slide":true,
                "elements":[text("saved-text","Heading"),picture("saved-picture","diagram")],
                "animations":[]}]})
    }

    fn original() -> Value {
        let mut source = presentation();
        source.as_object_mut().unwrap().remove("custom_document");
        source["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("custom_metadata");
        source["slides"][0]
            .as_object_mut()
            .unwrap()
            .remove("custom_slide");
        source["id"] = json!("source-presentation");
        source["title"] = json!("Source title");
        source["metadata"]["pptx_import_revision"] = json!(2);
        source["metadata"]["created_at"] = json!("new");
        source["slides"][0]["id"] = json!("source-slide");
        source["slides"][0]["background"] = json!({"Solid":"#123456"});
        let mut heading = text("source-text", "Heading");
        heading.as_object_mut().unwrap().remove("custom_element");
        heading["text_layout"] = json!({"wrap":true});
        heading["content"][0]["font_size"] = json!(24);
        let mut diagram = picture("source-picture", "diagram");
        diagram["url"] = json!("data:image/jpeg;base64,new");
        source["slides"][0]["elements"] =
            json!([picture("background", "background"), heading, diagram]);
        source["slides"][0]["animations"] = json!([{"target_id":"source-text","effect":"Fade"}]);
        source
    }

    #[test]
    fn recovers_missing_images_and_layout_preserving_identity_and_extensions() {
        let current = presentation();
        let recovered = merge_original_layout(1, &current, &original()).unwrap();
        assert_eq!(recovered["id"], "saved-presentation");
        assert_eq!(recovered["title"], "Saved title");
        assert_eq!(recovered["metadata"]["created_at"], "old");
        assert_eq!(recovered["metadata"]["custom_metadata"], 9);
        assert_eq!(recovered["metadata"]["pptx_import_revision"], 2);
        assert_eq!(recovered["custom_document"], true);
        let slide = &recovered["slides"][0];
        assert_eq!(slide["id"], "saved-slide");
        assert_eq!(slide["custom_slide"], true);
        assert_eq!(slide["background"]["Solid"], "#123456");
        assert_eq!(slide["elements"].as_array().unwrap().len(), 3);
        assert_eq!(slide["elements"][1]["id"], "saved-text");
        assert_eq!(slide["elements"][1]["content"][0]["font_size"], 24);
        assert_eq!(slide["elements"][1]["text_layout"]["wrap"], true);
        assert_eq!(slide["elements"][1]["custom_element"], 7);
        assert_eq!(slide["elements"][2]["id"], "saved-picture");
        assert_eq!(slide["elements"][2]["url"], "data:image/jpeg;base64,new");
        assert_eq!(slide["animations"][0]["target_id"], "saved-text");
        assert_eq!(
            current["slides"][0]["elements"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn edited_mismatched_reordered_and_already_recovered_models_are_unchanged() {
        let current = presentation();
        let source = original();
        assert!(merge_original_layout(2, &current, &source).is_none());
        let mut changed = current.clone();
        changed["slides"][0]["elements"][0]["content"][0]["text"] = json!("Edited");
        assert!(merge_original_layout(1, &changed, &source).is_none());
        let mut moved = current.clone();
        moved["slides"][0]["elements"][0]["x"] = json!(11);
        assert!(merge_original_layout(1, &moved, &source).is_none());
        let mut reordered = current.clone();
        reordered["slides"][0]["elements"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(merge_original_layout(1, &reordered, &source).is_none());
        let mut newer = current;
        newer["metadata"]["pptx_import_revision"] = json!(2);
        assert!(merge_original_layout(1, &newer, &source).is_none());
    }

    #[test]
    fn geometry_matches_model_f32_after_database_json_roundtrip() {
        // These are actual coordinates in the retained 20-slide PPTX. A DB
        // read decodes the expanded f32 decimal through serde_json's f64 parser.
        let saved: Value = serde_json::from_str(
            r#"{"x":229.66697692871094,"y":384.6669921875,"width":137.60000610351562,"height":76.80000305175781,"rotation":0.0}"#,
        ).unwrap();
        let source = json!({"x":229.66698_f32,"y":384.667_f32,"width":137.6_f32,
            "height":76.8_f32,"rotation":0.0_f32});
        assert!(same_geometry(&saved, &source));
        let saved_again: Value =
            serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
        assert!(same_geometry(&saved_again, &source));

        let mut moved = source.clone();
        let x = source["x"].as_f64().unwrap() as f32;
        moved["x"] = json!(f32::from_bits(x.to_bits() + 1));
        assert!(!same_geometry(&saved, &moved));
        moved["x"] = json!(f64::MAX);
        assert!(!same_geometry(&saved, &moved));
    }

    #[test]
    fn image_crop_matches_model_precision_but_rejects_changed_or_missing_crop() {
        let mut saved = picture("saved", "diagram");
        saved["crop"] = serde_json::from_str(
            r#"{"left":0.3333333432674408,"right":0.10000000149011612,"top":0.0,"bottom":0.0}"#,
        )
        .unwrap();
        let mut source = picture("source", "diagram");
        source["crop"] =
            json!({"left":0.33333334_f32,"right":0.1_f32,"top":0.0_f32,"bottom":0.0_f32});
        assert!(same_image(&saved, &source));
        source["crop"]["left"] = json!(f32::from_bits(0.33333334_f32.to_bits() + 1));
        assert!(!same_image(&saved, &source));
        source["crop"] = Value::Null;
        assert!(!same_image(&saved, &source));
        saved["crop"] = Value::Null;
        assert!(same_image(&saved, &source));
        source.as_object_mut().unwrap().remove("crop");
        assert!(!same_image(&saved, &source));
    }

    #[test]
    fn group_recovery_rejects_deleted_saved_objects_and_preserves_nested_ids() {
        let mut current = presentation();
        current["slides"][0]["elements"] = json!([{"type":"Group","id":"old-group",
            "elements":[text("old-child","Nested")]}]);
        let mut source = original();
        source["slides"][0]["elements"] = json!([{"type":"Group","id":"new-group",
            "elements":[picture("new-image","Nested image"),text("new-child","Nested")]}]);
        let recovered = merge_original_layout(1, &current, &source).unwrap();
        let group = &recovered["slides"][0]["elements"][0];
        assert_eq!(group["id"], "old-group");
        assert_eq!(group["elements"][1]["id"], "old-child");
        source["slides"][0]["elements"][0]["elements"] = json!([]);
        assert!(merge_original_layout(1, &current, &source).is_none());
    }

    #[test]
    fn recovery_cache_evicts_by_count_and_bytes_and_keys_saved_content() {
        let key = |index: u8| CacheKey {
            file_id: "file".into(),
            source_id: "source".into(),
            path: "source.pptx".into(),
            size: 10,
            content_hash: [index; 32],
        };
        let mut cache = RecoveryCache::default();
        for index in 0..5 {
            cache.insert(key(index), json!({"index":index}), 100);
        }
        assert!(cache.get(&key(0)).is_none());
        assert_eq!(cache.get(&key(4)).unwrap()["index"], 4);
        assert!(cache.get(&key(10)).is_none());
        cache.insert(key(6), json!({}), MAX_CACHE_BYTES);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_CACHE_BYTES);
        cache.insert(key(7), json!({}), MAX_CACHE_BYTES + 1);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.get(&key(6)).is_some());
    }

    #[test]
    fn recovered_ids_and_animation_targets_survive_new_random_source_ids() {
        let current = presentation();
        let mut first_source = original();
        first_source["slides"][0]["animations"] =
            json!([{"target_id":"background","effect":"Fade"}]);
        let mut second_source = first_source.clone();
        second_source["id"] = json!("another-source-presentation");
        second_source["slides"][0]["id"] = json!("another-source-slide");
        second_source["slides"][0]["elements"][0]["id"] = json!("another-background");
        second_source["slides"][0]["elements"][1]["id"] = json!("another-source-text");
        second_source["slides"][0]["elements"][2]["id"] = json!("another-source-picture");
        second_source["slides"][0]["animations"][0]["target_id"] = json!("another-background");
        let first = merge_original_layout(1, &current, &first_source).unwrap();
        let second = merge_original_layout(1, &current, &second_source).unwrap();
        assert_eq!(first, second);
        let id = first["slides"][0]["elements"][0]["id"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok());
        assert_eq!(first["slides"][0]["animations"][0]["target_id"], id);
    }

    #[test]
    fn image_background_replaces_solid_enum_and_obsolete_media_warnings() {
        let mut current = presentation();
        current["metadata"]["import_warnings"] = json!(["Old media budget"]);
        current["slides"][0]["elements"][0]["content"][0]["custom_run"] = json!(true);
        let mut source = original();
        source["slides"][0]["background"] = json!({"Image":"data:image/jpeg;base64,new"});
        let recovered = merge_original_layout(1, &current, &source).unwrap();
        assert!(recovered["slides"][0]["background"].get("Solid").is_none());
        assert_eq!(
            recovered["slides"][0]["background"]["Image"],
            "data:image/jpeg;base64,new"
        );
        assert!(recovered["metadata"].get("import_warnings").is_none());
        assert_eq!(
            recovered["slides"][0]["elements"][1]["content"][0]["custom_run"],
            true
        );
    }
}
