//! Read-only recovery of unedited XLSX imports from their immutable source.
use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::StorageBackend;
use crate::database::Database;

fn needs_recovery(version: i32, content: &Value) -> bool {
    version == 1
        && content["sheets"].is_array()
        && content["metadata"]["xlsx_import_revision"]
            .as_u64()
            .unwrap_or(0)
            < 2
}

/// Normal document authorization must run before calling this optional reader.
pub async fn for_response(
    db: &Database,
    storage: &StorageBackend,
    file_id: &str,
    version: i32,
    content: Value,
) -> Value {
    if !needs_recovery(version, &content) {
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
    let Ok(Some((path, size))) = source else {
        return content;
    };
    if !(0..=32 * 1024 * 1024).contains(&size)
        || !std::path::Path::new(&path)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("xlsx"))
    {
        return content;
    }
    let Ok(bytes) = storage.read_file(&path).await else {
        return content;
    };
    if bytes.len() > 32 * 1024 * 1024 {
        return content;
    }
    let source = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        syntra_export::xlsx::xlsx_to_workbook(&bytes)
            .and_then(|book| serde_json::to_value(book).map_err(|e| e.to_string()))
    })
    .await;
    match source {
        Ok(Ok(source)) => recover(version, &content, &source).unwrap_or(content),
        _ => content,
    }
}

fn overlay(current: &Value, source: &Value) -> Value {
    if let (Some(old), Some(new)) = (current.as_object(), source.as_object()) {
        let mut out = old.clone();
        for (key, value) in new {
            out.insert(
                key.clone(),
                old.get(key)
                    .map(|old| overlay(old, value))
                    .unwrap_or_else(|| value.clone()),
            );
        }
        Value::Object(out)
    } else {
        source.clone()
    }
}

fn cell_map(cells: &Value) -> Option<BTreeMap<(u64, u64), &Value>> {
    let cells = cells.as_array()?;
    if cells.len() > 100_000 {
        return None;
    }
    let mut out = BTreeMap::new();
    for entry in cells {
        let pair = entry.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        let key = (pair[0]["row"].as_u64()?, pair[0]["col"].as_u64()?);
        if out.insert(key, &pair[1]).is_some() {
            return None;
        }
    }
    Some(out)
}

fn same_cell(current: &Value, source: &Value) -> bool {
    let value = same_json(&current["value"], &source["value"])
        || (current["value"]["type"] == "Text"
            && source["value"]["type"] == "Date"
            && current["value"]["value"] == source["value"]["value"]);
    let mut old_format = current["format"].clone();
    if matches!(source["value"]["type"].as_str(), Some("Number" | "Date"))
        && old_format["horizontal_align"] == "Left"
        && source["format"]["horizontal_align"] == "Right"
    {
        // The original importer mapped unspecified/general alignment to Left.
        // A fresh import can now infer the numeric/date default from the same
        // source. Explicit source Left stays Left and cannot enter this case.
        old_format["horizontal_align"] = Value::String("Right".into());
    }
    value
        && current["formula"] == source["formula"]
        && current["comment"] == source["comment"]
        && same_format(&old_format, &source["format"])
}

// Compare values using the same JSON serialization/parser path as storage.
// Besides 2000 versus 2000.0, the configured parser can turn the serialized
// 1172.8319999999999 into 1172.832. This is a specific storage round trip,
// not a numeric epsilon or the f32 comparison used for layout geometry.
fn same_json(current: &Value, source: &Value) -> bool {
    match (current, source) {
        (Value::Number(a), Value::Number(b)) => {
            a == b
                || a.as_f64()
                    .zip(b.as_f64())
                    .is_some_and(|(a, b)| a.is_finite() && b.is_finite() && a == b)
                || serde_json::from_str::<Value>(&b.to_string())
                    .is_ok_and(|stored| stored == Value::Number(a.clone()))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_json(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same_json(a, b)))
        }
        _ => current == source,
    }
}

fn same_format(current: &Value, source: &Value) -> bool {
    let mut supplemented = current.clone();
    if current.is_object() && current["bg_color"].is_null() && source["bg_color"].is_string() {
        supplemented["bg_color"] = source["bg_color"].clone();
    }
    same_metrics(&supplemented, source)
}

fn same_metrics(current: &Value, source: &Value) -> bool {
    match (current, source) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => {
                a.is_finite()
                    && b.is_finite()
                    && (a as f32).is_finite()
                    && (b as f32).is_finite()
                    && (a as f32).to_bits() == (b as f32).to_bits()
            }
            _ => false,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_metrics(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same_metrics(a, b)))
        }
        _ => current == source,
    }
}

fn stable_image_id(book: &Value, sheet: &Value, index: usize, image: &Value) -> String {
    let mut hash = Sha256::new();
    // Keep the original namespace so previously recovered image IDs stay stable.
    hash.update(b"\x63ontrix-xlsx-recovered-image-v1\0");
    hash.update(book["id"].as_str().unwrap_or_default());
    hash.update([0]);
    hash.update(sheet["id"].as_str().unwrap_or_default());
    hash.update((index as u64).to_le_bytes());
    hash.update(image["data_url"].as_str().unwrap_or_default());
    let digest = hash.finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 0x80;
    bytes[8] = (bytes[8] & 63) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn same_conditional_formats(current: &Value, source: &Value) -> bool {
    if current == source {
        return true;
    }
    let (Some(old), Some(new)) = (current.as_array(), source.as_array()) else {
        return false;
    };
    old.len() == new.len()
        && old.iter().zip(new).all(|(old, new)| {
            // Early dxf parsing omitted patternFill.bgColor. Permit that missing
            // fill only; ranges, conditions and every other format must match.
            let mut supplemented = old.clone();
            if old["format"]["bg_color"].is_null() && new["format"]["bg_color"].is_string() {
                supplemented["format"]["bg_color"] = new["format"]["bg_color"].clone();
            }
            same_json(&supplemented, new)
        })
}

fn recover(version: i32, current: &Value, source: &Value) -> Option<Value> {
    if !needs_recovery(version, current) {
        return None;
    }
    let old = current["sheets"].as_array()?;
    let new = source["sheets"].as_array()?;
    // Complex pivots are kept verbatim instead of regenerating their IDs/data.
    if old.len() != new.len()
        || current["pivot_tables"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
        || !same_json(&current["named_ranges"], &source["named_ranges"])
    {
        return None;
    }
    let mut sheets = Vec::new();
    for (old, new) in old.iter().zip(new) {
        for key in [
            "name",
            "column_widths",
            "row_heights",
            "merged_cells",
            "frozen_rows",
            "frozen_cols",
            "data_validations",
        ] {
            if old.get(key).is_some()
                && if matches!(key, "column_widths" | "row_heights") {
                    !same_metrics(&old[key], &new[key])
                } else {
                    !same_json(&old[key], &new[key])
                }
            {
                return None;
            }
        }
        if !same_conditional_formats(&old["conditional_formats"], &new["conditional_formats"]) {
            return None;
        }
        let old_cells = cell_map(&old["cells"])?;
        let new_cells = cell_map(&new["cells"])?;
        if old_cells.len() != new_cells.len()
            || !old_cells
                .iter()
                .all(|(key, value)| new_cells.get(key).is_some_and(|new| same_cell(value, new)))
        {
            return None;
        }
        let old_charts = old["charts"].as_array()?;
        let new_charts = new["charts"].as_array()?;
        if old_charts.len() != new_charts.len() {
            return None;
        }
        let mut charts = Vec::new();
        for (old_chart, new_chart) in old_charts.iter().zip(new_charts) {
            for key in [
                "title",
                "chart_type",
                "data_start",
                "data_end",
                "anchor_start",
                "anchor_end",
                "series_sources",
            ] {
                if old_chart.get(key).is_some() && !same_json(&old_chart[key], &new_chart[key]) {
                    return None;
                }
            }
            if old_chart["cached_data"].is_object()
                && ["labels", "datasets"].iter().any(|key| {
                    !same_json(
                        &old_chart["cached_data"][*key],
                        &new_chart["cached_data"][*key],
                    )
                })
            {
                return None;
            }
            let mut chart = overlay(old_chart, new_chart);
            if let Some(id) = old_chart.get("id") {
                chart["id"] = id.clone();
            }
            charts.push(chart);
        }
        // Historical models had no images. Never replace an existing image.
        if old["images"]
            .as_array()
            .is_some_and(|images| !images.is_empty())
        {
            return None;
        }
        let mut sheet = overlay(old, new);
        sheet["id"] = old["id"].clone();
        sheet["cells"] = Value::Array(new_cells.iter().map(|((row,col),new)| {
            serde_json::json!([{"row":row,"col":col}, overlay(old_cells.get(&(*row,*col)).unwrap(),new)])
        }).collect());
        sheet["charts"] = Value::Array(charts);
        if let Some(images) = sheet["images"].as_array_mut() {
            for (index, image) in images.iter_mut().enumerate() {
                image["id"] = Value::String(stable_image_id(current, old, index, image));
            }
        }
        sheets.push(sheet);
    }
    let mut out = overlay(current, source);
    for key in ["id", "title"] {
        if let Some(value) = current.get(key) {
            out[key] = value.clone();
        }
    }
    for key in ["author", "created_at", "modified_at"] {
        if let Some(value) = current["metadata"].get(key) {
            out["metadata"][key] = value.clone();
        }
    }
    out["sheets"] = Value::Array(sheets);
    if serde_json::to_vec(&out).ok()?.len() > syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn saved() -> Value {
        json!({"id":"old-book","title":"Saved title","metadata":{"created_at":"old"},"named_ranges":[],"pivot_tables":[],"sheets":[{"id":"old-sheet","name":"Sheet1","cells":[[{"row":0,"col":0},{"value":{"type":"Text","value":"2026-11-17T00:00:00.000Z"},"formula":null,"format":{},"comment":null}]],"charts":[]}]})
    }
    fn source() -> Value {
        let mut source = saved();
        source["id"] = json!("source-book");
        source["metadata"]["xlsx_import_revision"] = json!(2);
        source["sheets"][0]["id"] = json!("source-sheet");
        source["sheets"][0]["cells"][0][1]["value"]["type"] = json!("Date");
        source["sheets"][0]["images"] =
            json!([{"id":"random","data_url":"data:image/png;base64,abc"}]);
        source
    }
    #[test]
    fn restores_missing_images_and_iso_dates_without_changing_identity() {
        let old = saved();
        let recovered = recover(1, &old, &source()).unwrap();
        assert_eq!(recovered["id"], "old-book");
        assert_eq!(recovered["sheets"][0]["id"], "old-sheet");
        assert_eq!(
            recovered["sheets"][0]["cells"][0][1]["value"]["type"],
            "Date"
        );
        assert_eq!(recovered["metadata"]["created_at"], "old");
        let mut another = source();
        another["sheets"][0]["images"][0]["id"] = json!("another-random");
        assert_eq!(recovered, recover(1, &old, &another).unwrap());
        assert!(old["sheets"][0].get("images").is_none());
    }
    #[test]
    fn edited_newer_reordered_and_mismatched_cells_are_preserved() {
        let old = saved();
        assert!(recover(2, &old, &source()).is_none());
        let mut edited = old.clone();
        edited["sheets"][0]["cells"][0][1]["value"]["value"] = json!("Changed");
        assert!(recover(1, &edited, &source()).is_none());
        let mut formula = old.clone();
        formula["sheets"][0]["cells"][0][1]["formula"] = json!("1+1");
        assert!(recover(1, &formula, &source()).is_none());
        let mut newer = old;
        newer["metadata"]["xlsx_import_revision"] = json!(2);
        assert!(recover(1, &newer, &source()).is_none());
    }

    #[test]
    fn missing_imported_fill_can_recover_but_a_changed_condition_cannot() {
        let old = json!([{"range_start":{"row":0,"col":0},"range_end":{"row":1,"col":0},
            "rule":{"GreaterThan":10},"format":{"bg_color":null,"bold":true}}]);
        let mut new = old.clone();
        new[0]["format"]["bg_color"] = json!("#DCFCE7");
        assert!(same_conditional_formats(&old, &new));
        new[0]["rule"]["GreaterThan"] = json!(11);
        assert!(!same_conditional_formats(&old, &new));
        assert!(same_format(
            &json!({"bg_color":null,"bold":true}),
            &json!({"bg_color":"#DCFCE7","bold":true})
        ));
        assert!(!same_format(
            &json!({"bg_color":"#000000","bold":true}),
            &json!({"bg_color":"#DCFCE7","bold":true})
        ));
    }

    #[test]
    fn storage_integer_spelling_matches_model_f64_without_rounding_value_edits() {
        let mut old = saved();
        old["sheets"][0]["cells"][0][1]["value"] =
            serde_json::from_str(r#"{"type":"Number","value":2000}"#).unwrap();
        let mut new = source();
        new["sheets"][0]["cells"][0][1]["value"] = json!({"type":"Number","value":2000.0_f64});
        assert!(recover(1, &old, &new).is_some());
        new["sheets"][0]["cells"][0][1]["value"]["value"] = json!(2000.000000001_f64);
        assert!(recover(1, &old, &new).is_none());
        assert!(same_conditional_formats(
            &json!([{"format":{"bg_color":null},"rule":{"GreaterThan":2000}}]),
            &json!([{"format":{"bg_color":"#DCFCE7"},"rule":{"GreaterThan":2000.0_f64}}])
        ));
        let model = json!(1172.8319999999999_f64);
        let stored: Value = serde_json::from_str(&model.to_string()).unwrap();
        assert!(same_json(&stored, &model));
        assert!(!same_json(&json!(1172.832000000001_f64), &model));
    }

    #[test]
    fn recovers_the_independent_workbook_saved_by_the_previous_importer() {
        let old: Value = serde_json::from_str(include_str!(
            "../../../export/tests/fixtures/render-audit-legacy-calc.json"
        ))
        .unwrap();
        let source = serde_json::to_value(
            syntra_export::xlsx::xlsx_to_workbook(include_bytes!(
                "../../../export/tests/fixtures/render-audit.xlsx"
            ))
            .unwrap(),
        )
        .unwrap();
        let recovered = recover(1, &old, &source).expect("an untouched prior import must recover");
        assert_eq!(recovered["metadata"]["xlsx_import_revision"], 2);
        assert_eq!(
            recovered["sheets"][0]["images"].as_array().unwrap().len(),
            1
        );
        let cells = cell_map(&recovered["sheets"][0]["cells"]).unwrap();
        assert_eq!(cells[&(4, 1)]["value"]["type"], "Date");
        assert_eq!(recovered["id"], old["id"]);
        let mut edited = old.clone();
        edited["sheets"][0]["cells"][0][1]["value"] = json!({"type":"Text","value":"Edited"});
        assert!(recover(1, &edited, &source).is_none());
    }
}
