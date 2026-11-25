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
            .pointer("/professional/docx_import_revision")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            < 3
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
                if let Some(layout) = original.get("layout") {
                    let mut merged = current
                        .get("layout")
                        .cloned()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    if let Some(properties) = layout.as_object() {
                        for (field, value) in properties {
                            merged[field] = value.clone();
                        }
                    }
                    current["layout"] = merged;
                }
                merge_list_items(&mut current["children"], &original["children"]);
            }
        }
    }
}

fn same_geometry(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => {
            left.is_finite()
                && right.is_finite()
                && (left as f32).to_bits() == (right as f32).to_bits()
        }
        (None, None) => left == right,
        _ => false,
    }
}

/// The old importer emitted one physical cell for each vMerge continuation.
/// Project the new logical table back to that shape before comparing content.
fn physical_table_rows(table: &Value) -> Option<Vec<Vec<Option<Value>>>> {
    let mut active = std::collections::BTreeMap::<usize, (u32, u32)>::new();
    let mut output = Vec::new();
    for row in table["rows"].as_array()? {
        let mut cells = row["cells"].as_array()?.iter().peekable();
        let mut next = std::collections::BTreeMap::new();
        let mut physical = Vec::new();
        let mut column = 0;
        while cells.peek().is_some() || active.range(column..).next().is_some() {
            if column > 10_000 {
                return None;
            }
            if let Some(&(remaining, span)) = active.get(&column) {
                physical.push(None);
                if remaining > 1 {
                    next.insert(column, (remaining - 1, span));
                }
                column += span as usize;
            } else {
                let cell = cells.next()?;
                let span = cell["col_span"].as_u64()?.clamp(1, 1000) as u32;
                let rows = cell["row_span"].as_u64()?.clamp(1, 1000) as u32;
                if rows > 1 {
                    next.insert(column, (rows - 1, span));
                }
                physical.push(Some(cell.clone()));
                column += span as usize;
            }
        }
        output.push(physical);
        active = next;
    }
    Some(output)
}

fn recover_blocks(
    old: &[Value],
    source: &[Value],
    ids: &mut std::collections::HashMap<String, String>,
    depth: usize,
) -> Option<Vec<Value>> {
    if depth > 32 {
        return None;
    }
    let mut output = Vec::new();
    let mut index = 0;
    for source in source {
        if source["type"] == "PageBreak" {
            match old.get(index) {
                Some(block) if block["type"] == "PageBreak" => {
                    output.push(block.clone());
                    index += 1;
                }
                Some(block)
                    if block["type"] == "Paragraph"
                        && runs_text(&block["runs"]).as_deref() == Some("\n") =>
                {
                    let mut recovered = block.clone();
                    recovered["type"] = json!("PageBreak");
                    if let Some(object) = recovered.as_object_mut() {
                        for field in ["runs", "alignment", "spacing", "indent_level"] {
                            object.remove(field);
                        }
                    }
                    output.push(recovered);
                    index += 1;
                }
                // pageBreakBefore had no old body object. Inserting the source
                // break is safe only if all following content also matches.
                _ => output.push(source.clone()),
            }
            continue;
        }
        let old = old.get(index)?;
        index += 1;
        let mut recovered = old.clone();
        if let (Some(source_id), Some(old_id)) = (source["id"].as_str(), old["id"].as_str()) {
            ids.insert(source_id.to_owned(), old_id.to_owned());
        }
        match source["type"].as_str()? {
            "Paragraph" | "Heading" if compatible_blocks(old, source) => {
                let old_runs = old["runs"].as_array().cloned().unwrap_or_else(|| {
                    flatten_list_runs(
                        old["items"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                    )
                });
                recovered["runs"] = json!(merge_runs(&old_runs, source["runs"].as_array()?));
                for field in ["type", "alignment", "indent_level", "spacing", "level"] {
                    if let Some(value) = source.get(field) {
                        recovered[field] = value.clone();
                    }
                }
                if let Some(object) = recovered.as_object_mut() {
                    object.remove("items");
                    object.remove("ordered");
                    if source["type"] != "Heading" {
                        object.remove("level");
                    }
                }
            }
            "List" if compatible_blocks(old, source) => {
                merge_list_items(&mut recovered["items"], &source["items"])
            }
            "Image" if old["type"] == "Image" => {
                if old["url"] != source["url"]
                    || old["alt_text"] != source["alt_text"]
                    || !same_geometry(&old["width"], &source["width"])
                    || !same_geometry(&old["height"], &source["height"])
                {
                    return None;
                }
                recovered["alignment"] = source["alignment"].clone();
            }
            "Table" if old["type"] == "Table" => {
                let projected = physical_table_rows(source)?;
                let old_rows = old["rows"].as_array()?;
                if old_rows.len() != projected.len() {
                    return None;
                }
                let mut rows = Vec::new();
                for (row_index, (old_row, physical)) in old_rows.iter().zip(projected).enumerate() {
                    let old_cells = old_row["cells"].as_array()?;
                    let owned = source["rows"][row_index]["cells"].as_array()?;
                    let physical = if old_cells.len() == owned.len()
                        && old_cells.iter().zip(owned).all(|(a, b)| {
                            a["col_span"] == b["col_span"] && a["row_span"] == b["row_span"]
                        }) {
                        owned.iter().cloned().map(Some).collect::<Vec<_>>()
                    } else {
                        physical
                    };
                    if old_cells.len() != physical.len() {
                        return None;
                    }
                    let mut row = old_row.clone();
                    let mut cells = Vec::new();
                    for (old_cell, source_cell) in old_cells.iter().zip(physical) {
                        if let Some(source_cell) = source_cell {
                            let blocks = recover_blocks(
                                old_cell["blocks"].as_array()?,
                                source_cell["blocks"].as_array()?,
                                ids,
                                depth + 1,
                            )?;
                            let mut cell = old_cell.clone();
                            cell["blocks"] = json!(blocks);
                            for field in ["col_span", "row_span", "background_color", "layout"] {
                                cell[field] = source_cell[field].clone();
                            }
                            cells.push(cell);
                        } else if !old_cell["blocks"].as_array()?.iter().all(|block| {
                            block["type"] == "Paragraph"
                                && runs_text(&block["runs"]).as_deref() == Some("")
                        }) {
                            return None;
                        }
                    }
                    row["cells"] = json!(cells);
                    rows.push(row);
                }
                recovered["rows"] = json!(rows);
                recovered["column_widths"] = source["column_widths"].clone();
            }
            "Blockquote" if old["type"] == "Blockquote" => {
                recovered["blocks"] = json!(recover_blocks(
                    old["blocks"].as_array()?,
                    source["blocks"].as_array()?,
                    ids,
                    depth + 1
                )?)
            }
            "HorizontalRule" if old["type"] == source["type"] => {}
            "CodeBlock"
                if old["type"] == "CodeBlock"
                    && old["code"] == source["code"]
                    && old["language"] == source["language"] => {}
            _ => return None,
        }
        output.push(recovered);
    }
    (index == old.len()).then_some(output)
}

fn recover_revision_two(mut current: Value, source: &Value) -> Value {
    let mut ids = std::collections::HashMap::new();
    let Some(body) = current["body"]
        .as_array()
        .zip(source["body"].as_array())
        .and_then(|(old, source)| recover_blocks(old, source, &mut ids, 0))
    else {
        return current;
    };
    // Nonempty saved furniture must still agree with the retained original.
    for field in ["header", "footer"] {
        let old = current["professional"][field].as_str().unwrap_or_default();
        if !old.is_empty() && old != source["professional"][field].as_str().unwrap_or_default() {
            return current;
        }
    }
    current["body"] = json!(body);
    if !current["professional"].is_object() {
        current["professional"] = json!({});
    }
    let formats = source["professional"]["paragraph_formats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|format| {
            let id = ids.get(format["block_id"].as_str()?)?;
            let mut merged = current["professional"]["paragraph_formats"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|old| old["block_id"].as_str() == Some(id))
                .cloned()
                .unwrap_or_else(|| json!({}));
            for (field, value) in format.as_object()? {
                merged[field] = value.clone();
            }
            merged["block_id"] = json!(id);
            Some(merged)
        })
        .collect::<Vec<_>>();
    current["professional"]["paragraph_formats"] = json!(formats);
    for field in [
        "header",
        "footer",
        "header_alignment",
        "footer_alignment",
        "font_metrics",
        "language",
        "east_asia_language",
        "page_numbers",
        "different_first_page",
        "docx_import_revision",
    ] {
        if let Some(value) = source["professional"].get(field) {
            current["professional"][field] = value.clone();
        }
    }
    current
}

pub fn merge_original_formatting(version: i32, mut current: Value, original: &Value) -> Value {
    if !needs_compatibility(version, &current) {
        return current;
    }
    if original
        .pointer("/professional/docx_import_revision")
        .and_then(Value::as_u64)
        >= Some(2)
    {
        return recover_revision_two(current, original);
    }
    // Retain the narrow, original compatibility contract for older callers.
    if current.pointer("/professional/paragraph_formats").is_some() {
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

    fn audit_source() -> Value {
        serde_json::to_value(
            syntra_export::docx::docx_to_writer(include_bytes!(
                "../../../export/tests/fixtures/render-audit.docx"
            ))
            .unwrap(),
        )
        .unwrap()
    }

    fn audit_legacy() -> Value {
        serde_json::from_str(include_str!(
            "../../../export/tests/fixtures/render-audit-legacy.json"
        ))
        .unwrap()
    }

    #[test]
    fn revision_two_recovers_real_legacy_tables_breaks_fonts_and_furniture() {
        let mut old = audit_legacy();
        let table_index = old["body"]
            .as_array()
            .unwrap()
            .iter()
            .position(|block| block["type"] == "Table")
            .unwrap();
        old["body"][table_index]["custom_table"] = json!(17);
        old["body"][table_index]["rows"][0]["cells"][0]["custom_cell"] = json!(18);
        old["professional"]["custom_layout"] = json!(19);
        let old_title_id = old["body"][0]["id"].clone();
        let old_cell_id =
            old["body"][table_index]["rows"][0]["cells"][0]["blocks"][0]["id"].clone();
        let recovered = merge_original_formatting(1, old, &audit_source());
        assert_eq!(recovered["professional"]["docx_import_revision"], 3);
        assert!(
            recovered["body"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["type"] == "PageBreak")
        );
        let table = &recovered["body"][table_index];
        assert_eq!(table["rows"][0]["cells"][0]["col_span"], 3);
        assert_eq!(table["rows"][0]["cells"][0]["background_color"], "#1E3A5F");
        assert_eq!(table["rows"][2]["cells"][0]["row_span"], 2);
        assert_eq!(table["rows"][3]["cells"].as_array().unwrap().len(), 2);
        assert_eq!(recovered["body"][0]["id"], old_title_id);
        assert_eq!(table["rows"][0]["cells"][0]["blocks"][0]["id"], old_cell_id);
        assert_eq!(table["custom_table"], 17);
        assert_eq!(table["rows"][0]["cells"][0]["custom_cell"], 18);
        assert_eq!(recovered["professional"]["custom_layout"], 19);
        assert_eq!(recovered["professional"]["page_numbers"], true);
        assert_eq!(recovered["professional"]["header_alignment"], "Left");
        assert_eq!(recovered["professional"]["footer_alignment"], "Right");
        let list = recovered["body"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "List")
            .unwrap();
        assert_eq!(list["items"][0]["layout"]["spacing"]["after"], 6.);
        assert_eq!(
            list["items"][0]["layout"]["format"]["contextual_spacing"],
            true
        );
        assert_eq!(
            list["items"][0]["layout"]["format"]["first_line_indent_pt"],
            -18.
        );
        assert_eq!(
            recovered["professional"]["font_metrics"]["simsun"]["cjk"],
            true
        );
        serde_json::from_value::<syntra_doc_engine::WriterDocument>(recovered).unwrap();
    }

    #[test]
    fn revision_three_upgrades_owned_merged_cells_without_overwriting_edits() {
        let source = serde_json::to_value(
            syntra_export::docx::docx_to_writer(include_bytes!(
                "../../../export/tests/fixtures/render-audit.docx"
            ))
            .unwrap(),
        )
        .unwrap();
        let mut old = source.clone();
        old["professional"]["docx_import_revision"] = json!(2);
        for block in old["body"].as_array_mut().unwrap() {
            if block["type"] == "Table" {
                for row in block["rows"].as_array_mut().unwrap() {
                    for cell in row["cells"].as_array_mut().unwrap() {
                        cell.as_object_mut().unwrap().remove("layout");
                    }
                }
            }
        }
        let fixed = merge_original_formatting(1, old.clone(), &source);
        assert_eq!(fixed["professional"]["docx_import_revision"], 3);
        let table = fixed["body"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "Table")
            .unwrap();
        assert_eq!(
            table["rows"][0]["cells"][0]["layout"]["padding"],
            json!([5., 5., 5., 5.])
        );
        assert_eq!(merge_original_formatting(2, old.clone(), &source), old);
    }

    #[test]
    fn revision_two_refuses_changed_text_cells_media_geometry_and_saved_versions() {
        let source = audit_source();
        let old = audit_legacy();
        assert_eq!(merge_original_formatting(2, old.clone(), &source), old);
        for path in [
            "/body/0/runs/0/text",
            "/body/8/url",
            "/body/8/width",
            "/body/13/rows/2/cells/1/blocks/0/runs/0/text",
        ] {
            let mut changed = old.clone();
            let value = changed.pointer_mut(path).expect("legacy fixture path");
            *value = if value.is_number() {
                json!(999)
            } else {
                json!("edited")
            };
            assert_eq!(
                merge_original_formatting(1, changed.clone(), &source),
                changed,
                "{path}"
            );
        }
        let mut upgraded = old.clone();
        upgraded["professional"]["docx_import_revision"] = json!(3);
        assert_eq!(
            merge_original_formatting(1, upgraded.clone(), &source),
            upgraded
        );
    }

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
