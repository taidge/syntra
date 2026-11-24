//! Durable collaboration: the database document is the authoritative snapshot.
//! An operation and its idempotency record commit together before any ACK.
use crate::state::AppState;
use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use serde_json::Value;
use syntra_collab::{OpType, Operation};
use syntra_core::error::AppError;
use uuid::Uuid;

pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_HISTORY_OPERATIONS: u64 = super::operation_history::MAX_HISTORY_ROWS as u64;
const MAX_HISTORY_BYTES: i64 = super::operation_history::MAX_HISTORY_BYTES;
pub struct Snapshot {
    pub content: Value,
    pub version: i32,
    pub kind: String,
}

pub async fn snapshot(db: &crate::state::DbPool, id: Uuid) -> Result<Snapshot, AppError> {
    use syntra_core::schema::documents::dsl as d;
    let mut db = db.clone();
    let (content, version, kind) = d::documents
        .find(id.to_string())
        .select((d::content, d::version, d::doc_type))
        .first::<(Option<String>, i32, String)>(&mut db)
        .await
        .optional()?
        .ok_or_else(|| AppError::NotFound("Document not found".into()))?;
    let content = content
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| AppError::Internal(format!("Invalid stored document: {e}")))?
        .unwrap_or(Value::Null);
    Ok(Snapshot {
        content,
        version,
        kind,
    })
}

pub async fn commit_operation(
    state: &AppState,
    id: Uuid,
    op: Operation,
) -> Result<Snapshot, AppError> {
    commit_operation_inner(state, id, op, None).await
}

pub async fn commit_operation_with_credential(
    state: &AppState,
    id: Uuid,
    op: Operation,
    credential: &crate::middleware::auth::AuthenticatedCredential,
) -> Result<Snapshot, AppError> {
    commit_operation_inner(state, id, op, Some(credential)).await
}

async fn commit_operation_inner(
    state: &AppState,
    id: Uuid,
    mut op: Operation,
    credential: Option<&crate::middleware::auth::AuthenticatedCredential>,
) -> Result<Snapshot, AppError> {
    let gate = state.collab.document_gate(id).await;
    let _guard = gate.lock().await;
    let mut db = state.db.clone();
    let mut current = snapshot(&db, id).await?;
    validate_operation_budget(&op.op_type)?;
    let original_request = op.clone();
    // Preserve the identity of the original request, before canonicalizing or
    // transforming it. A legitimate retry after OT must match that request,
    // while a reused id with a new payload must never receive a false ACK.
    let request_hash = super::persistence::operation_request_hash(&op)?;
    if super::persistence::operation_receipt(
        &state.db,
        &id.to_string(),
        &op.user_id.to_string(),
        &op,
        &request_hash,
    )
    .await?
    .is_some()
    {
        return snapshot(&state.db, id).await;
    }
    canonicalize_operation(&mut op.op_type, &current.kind);
    let version = u64::try_from(current.version)
        .map_err(|_| AppError::Internal("Invalid document version".into()))?;
    if op.base_version > version {
        return Err(AppError::Conflict("Future document version".into()));
    }
    let mut deletion = if matches!(op.op_type, OpType::Delete { .. }) && op.base_version == version
    {
        Some(DeletionIntent::from_content(
            &current.content,
            &current.kind,
            &op.op_type,
        )?)
    } else {
        None
    };
    if op.base_version < version {
        if version - op.base_version > MAX_HISTORY_OPERATIONS {
            return Err(AppError::Conflict(
                "History compacted or replaced; synchronize before retrying".into(),
            ));
        }
        #[derive(diesel::QueryableByName)]
        struct HistoricalOperation {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            version: i32,
            #[diesel(sql_type = diesel::sql_types::Text)]
            op_data: String,
        }
        // Bound bytes in SQL before bringing legacy/untrusted log payloads into
        // Rust memory. A row count alone could still buffer gigabytes.
        let history = diesel::sql_query("SELECT version, op_data FROM (SELECT version, op_data, SUM(LENGTH(CAST(op_data AS BLOB))) OVER (ORDER BY version ASC ROWS UNBOUNDED PRECEDING) AS bytes FROM operation_log WHERE document_id = ? AND version > ? AND version <= ? ORDER BY version ASC LIMIT 2049) WHERE bytes <= ? ORDER BY version ASC")
            .bind::<diesel::sql_types::Text,_>(id.to_string())
            .bind::<diesel::sql_types::Integer,_>(i32::try_from(op.base_version).map_err(|_| AppError::Conflict("Version out of range".into()))?)
            .bind::<diesel::sql_types::Integer,_>(current.version)
            .bind::<diesel::sql_types::BigInt,_>(MAX_HISTORY_BYTES)
            .load::<HistoricalOperation>(&mut db).await?;
        if history.len() as u64 != version - op.base_version {
            return Err(AppError::Conflict(
                "History compacted or replaced; synchronize before retrying".into(),
            ));
        }
        if matches!(op.op_type, OpType::Delete { .. }) {
            let base = historical_content(&state.db, id, op.base_version).await?;
            deletion = Some(DeletionIntent::from_content(
                &base,
                &current.kind,
                &op.op_type,
            )?);
        }
        for (index, stored) in history.iter().enumerate() {
            if stored.version as u64 != op.base_version + index as u64 + 1 {
                return Err(AppError::Conflict(
                    "History discontinuity; synchronize before retrying".into(),
                ));
            }
            let mut previous: Operation = serde_json::from_str(&stored.op_data)
                .map_err(|_| AppError::Internal("Invalid operation log".into()))?;
            for effect in stored_effects(&stored.op_data, &previous)? {
                previous.op_type = effect;
                validate_operation_budget(&previous.op_type)?;
                canonicalize_operation(&mut previous.op_type, &current.kind);
                if let Some(intent) = &mut deletion {
                    intent.rebase(&op, &previous)?;
                } else {
                    op = rebase_operation(op, &previous)?;
                }
            }
        }
    }
    let effects = deletion
        .map(|intent| intent.effects())
        .unwrap_or_else(|| vec![op.op_type.clone()]);
    for effect in &effects {
        apply(&mut current.content, &current.kind, effect)?;
    }
    op.op_type = effects[0].clone();
    validate_content(&current.kind, &current.content)?;
    op.base_version = version;
    let content =
        serde_json::to_string(&current.content).map_err(|e| AppError::Validation(e.to_string()))?;
    if content.len() > MAX_MESSAGE_BYTES {
        return Err(AppError::Validation(
            "Document exceeds collaboration message budget".into(),
        ));
    }
    let committed_version =
        super::persistence::commit_document_operation_with_effects_and_credential(
            &state.db,
            &state.storage,
            &id.to_string(),
            &op.user_id.to_string(),
            current.version,
            content,
            &op,
            &request_hash,
            &effects,
            &original_request,
            credential,
        )
        .await?;
    // Another process may have committed the identical request (or a newer REST
    // edit) while we awaited storage. Always pair content with the version read
    // from the same durable row instead of publishing our speculative value.
    current = snapshot(&state.db, id).await?;
    if current.version < committed_version {
        return Err(AppError::Internal(
            "Committed document version is unavailable".into(),
        ));
    }
    state
        .collab
        .publish_snapshot(id, current.content.clone(), current.version as u64)
        .await;
    Ok(current)
}

async fn historical_content(
    db: &crate::state::DbPool,
    id: Uuid,
    version: u64,
) -> Result<Value, AppError> {
    #[derive(diesel::QueryableByName)]
    struct HistoricalContent {
        #[diesel(sql_type = diesel::sql_types::Text)]
        content: String,
    }
    let mut db = db.clone();
    let stored = diesel::sql_query("SELECT content FROM versions WHERE document_id = ? AND version_number = ? AND content IS NOT NULL AND LENGTH(CAST(content AS BLOB)) <= ? ORDER BY created_at DESC LIMIT 1")
        .bind::<diesel::sql_types::Text,_>(id.to_string())
        .bind::<diesel::sql_types::Integer,_>(i32::try_from(version).map_err(|_| invalid())?)
        .bind::<diesel::sql_types::BigInt,_>(MAX_MESSAGE_BYTES as i64)
        .get_result::<HistoricalContent>(&mut db).await.optional()?
        .ok_or_else(|| AppError::Conflict("Base snapshot compacted; synchronize before retrying".into()))?;
    serde_json::from_str(&stored.content)
        .map_err(|_| AppError::Internal("Invalid historical document snapshot".into()))
}

fn operation_path_mut(op: &mut OpType) -> &mut Vec<String> {
    match op {
        OpType::Insert { path, .. }
        | OpType::Delete { path, .. }
        | OpType::Replace { path, .. }
        | OpType::SetProperty { path, .. }
        | OpType::DeleteProperty { path, .. }
        | OpType::InsertElement { path, .. }
        | OpType::RemoveElement { path, .. }
        | OpType::MoveElement { path, .. } => path,
    }
}

fn operation_path(op: &OpType) -> &[String] {
    match op {
        OpType::Insert { path, .. }
        | OpType::Delete { path, .. }
        | OpType::Replace { path, .. }
        | OpType::SetProperty { path, .. }
        | OpType::DeleteProperty { path, .. }
        | OpType::InsertElement { path, .. }
        | OpType::RemoveElement { path, .. }
        | OpType::MoveElement { path, .. } => path,
    }
}

fn moved_element(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && index > from && index <= to {
        index - 1
    } else if to < from && index >= to && index < from {
        index + 1
    } else {
        index
    }
}

fn element_after(index: usize, previous: &OpType) -> Result<usize, AppError> {
    match previous {
        OpType::InsertElement {
            index: inserted, ..
        } if index >= *inserted => index.checked_add(1).ok_or_else(invalid),
        OpType::RemoveElement { index: removed, .. } if index == *removed => Err(conflict()),
        OpType::RemoveElement { index: removed, .. } if index > *removed => Ok(index - 1),
        OpType::MoveElement {
            from_index,
            to_index,
            ..
        } => Ok(moved_element(index, *from_index, *to_index)),
        _ => Ok(index),
    }
}

/// Array edits move addressed descendants too, not only operations targeting
/// the array itself. Otherwise an edit to body[1] can silently hit its neighbour
/// after someone inserts, removes or moves body[0].
fn rebase_operation(mut op: Operation, previous: &Operation) -> Result<Operation, AppError> {
    let previous_path = operation_path(&previous.op_type);
    let replaced = match &previous.op_type {
        OpType::Replace { path, .. } => Some(path.clone()),
        OpType::SetProperty { path, key, .. } | OpType::DeleteProperty { path, key } => {
            let mut target = path.clone();
            target.push(key.clone());
            Some(target)
        }
        _ => None,
    };
    if let Some(replaced) = replaced {
        let path = operation_path(&op.op_type);
        if path.starts_with(&replaced) {
            // Replacement requests have their own old-value compare. Positional
            // edits cannot safely infer an index through an arbitrary replacement.
            if !matches!(&op.op_type, OpType::Replace { path, .. } if path == &replaced) {
                return Err(conflict());
            }
        }
    }
    if matches!(
        previous.op_type,
        OpType::InsertElement { .. } | OpType::RemoveElement { .. } | OpType::MoveElement { .. }
    ) {
        let path = operation_path_mut(&mut op.op_type);
        if path.len() > previous_path.len() && path.starts_with(previous_path) {
            let index = path[previous_path.len()]
                .parse::<usize>()
                .map_err(|_| invalid())?;
            path[previous_path.len()] = element_after(index, &previous.op_type)?.to_string();
        } else if path == previous_path {
            match &mut op.op_type {
                OpType::MoveElement {
                    from_index,
                    to_index,
                    ..
                } => {
                    *from_index = element_after(*from_index, &previous.op_type)?;
                    *to_index = element_after(*to_index, &previous.op_type)?;
                    return Ok(op);
                }
                OpType::RemoveElement { index, .. }
                    if matches!(previous.op_type, OpType::MoveElement { .. }) =>
                {
                    *index = element_after(*index, &previous.op_type)?;
                    return Ok(op);
                }
                OpType::InsertElement { index, .. } => {
                    if let OpType::MoveElement {
                        from_index,
                        to_index,
                        ..
                    } = &previous.op_type
                    {
                        let after_removal = if *index > *from_index {
                            *index - 1
                        } else {
                            *index
                        };
                        *index = if after_removal > *to_index {
                            after_removal.checked_add(1).ok_or_else(invalid)?
                        } else {
                            after_removal
                        };
                        return Ok(op);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(op.transform(previous))
}

struct DeletionSpan {
    start: usize,
    end: usize,
    removed: String,
}

/// A selection refers to original characters, not to whatever happens to occupy
/// the same numeric range after another author inserts text inside it.
struct DeletionIntent {
    path: Vec<String>,
    spans: Vec<DeletionSpan>,
}

fn char_byte(text: &str, index: usize) -> Result<usize, AppError> {
    text.char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .nth(index)
        .ok_or_else(invalid)
}

impl DeletionIntent {
    fn from_content(content: &Value, kind: &str, op: &OpType) -> Result<Self, AppError> {
        let OpType::Delete {
            path,
            index,
            length,
            removed_content,
        } = op
        else {
            return Err(invalid());
        };
        let mut content = content.clone();
        let value = target(&mut content, kind, path)?;
        let text = value.as_str().ok_or_else(invalid)?;
        let end = index.checked_add(*length).ok_or_else(invalid)?;
        let selected = &text[char_byte(text, *index)?..char_byte(text, end)?];
        if removed_content
            .as_ref()
            .is_some_and(|removed| removed != selected)
        {
            return Err(conflict());
        }
        Ok(Self {
            path: path.clone(),
            spans: if *length == 0 {
                vec![]
            } else {
                vec![DeletionSpan {
                    start: *index,
                    end,
                    removed: selected.to_owned(),
                }]
            },
        })
    }

    fn rebase(&mut self, template: &Operation, previous: &Operation) -> Result<(), AppError> {
        if operation_path(&previous.op_type) == self.path {
            match &previous.op_type {
                OpType::Insert { index, content, .. } => {
                    let inserted = content.chars().count();
                    let mut next = Vec::with_capacity(self.spans.len() + 1);
                    for span in self.spans.drain(..) {
                        if *index <= span.start {
                            next.push(DeletionSpan {
                                start: span.start.checked_add(inserted).ok_or_else(invalid)?,
                                end: span.end.checked_add(inserted).ok_or_else(invalid)?,
                                removed: span.removed,
                            });
                        } else if *index >= span.end {
                            next.push(span);
                        } else {
                            let split = char_byte(&span.removed, index - span.start)?;
                            next.push(DeletionSpan {
                                start: span.start,
                                end: *index,
                                removed: span.removed[..split].to_owned(),
                            });
                            next.push(DeletionSpan {
                                start: index.checked_add(inserted).ok_or_else(invalid)?,
                                end: span.end.checked_add(inserted).ok_or_else(invalid)?,
                                removed: span.removed[split..].to_owned(),
                            });
                        }
                    }
                    self.spans = next;
                    self.merge_adjacent();
                    return Ok(());
                }
                OpType::Delete { index, length, .. } => {
                    let removed_end = index.checked_add(*length).ok_or_else(invalid)?;
                    let shifted = |position: usize| {
                        if position <= *index {
                            position
                        } else if position >= removed_end {
                            position - length
                        } else {
                            *index
                        }
                    };
                    let mut next = Vec::with_capacity(self.spans.len());
                    for span in self.spans.drain(..) {
                        let left_end = span.end.min(*index);
                        if span.start < left_end {
                            let end_byte = char_byte(&span.removed, left_end - span.start)?;
                            next.push(DeletionSpan {
                                start: shifted(span.start),
                                end: shifted(left_end),
                                removed: span.removed[..end_byte].to_owned(),
                            });
                        }
                        let right_start = span.start.max(removed_end);
                        if right_start < span.end {
                            let start_byte = char_byte(&span.removed, right_start - span.start)?;
                            next.push(DeletionSpan {
                                start: shifted(right_start),
                                end: shifted(span.end),
                                removed: span.removed[start_byte..].to_owned(),
                            });
                        }
                    }
                    self.spans = next;
                    self.merge_adjacent();
                    return Ok(());
                }
                _ => {}
            }
        }
        let mut probe = template.clone();
        probe.op_type = OpType::Delete {
            path: self.path.clone(),
            index: 0,
            length: 0,
            removed_content: None,
        };
        self.path = operation_path(&rebase_operation(probe, previous)?.op_type).to_vec();
        Ok(())
    }

    fn merge_adjacent(&mut self) {
        let mut merged: Vec<DeletionSpan> = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..) {
            if let Some(previous) = merged.last_mut()
                && previous.end == span.start
            {
                previous.end = span.end;
                previous.removed.push_str(&span.removed);
            } else {
                merged.push(span);
            }
        }
        self.spans = merged;
    }

    fn effects(&self) -> Vec<OpType> {
        if self.spans.is_empty() {
            return vec![OpType::Delete {
                path: self.path.clone(),
                index: 0,
                length: 0,
                removed_content: Some(String::new()),
            }];
        }
        self.spans
            .iter()
            .rev()
            .map(|span| OpType::Delete {
                path: self.path.clone(),
                index: span.start,
                length: span.end - span.start,
                removed_content: Some(span.removed.clone()),
            })
            .collect()
    }
}

fn stored_effects(data: &str, previous: &Operation) -> Result<Vec<OpType>, AppError> {
    let stored: Value = serde_json::from_str(data)
        .map_err(|_| AppError::Internal("Invalid operation log".into()))?;
    match stored.get("transformed_ops") {
        Some(effects)
            if stored.get("transform_format").and_then(Value::as_u64) == Some(1)
                && effects
                    .as_array()
                    .is_some_and(|effects| !effects.is_empty() && effects.len() <= 2049) =>
        {
            serde_json::from_value(effects.clone())
                .map_err(|_| AppError::Internal("Invalid transformed operation log".into()))
        }
        Some(_) => Err(AppError::Internal(
            "Invalid transformed operation log".into(),
        )),
        None => Ok(vec![previous.op_type.clone()]),
    }
}

fn canonicalize_operation(op: &mut OpType, kind: &str) {
    let path = operation_path_mut(op);
    if path
        .first()
        .is_some_and(|segment| segment == kind || (kind == "calc" && segment == "workbook"))
    {
        path.remove(0);
    }
}

fn validate_operation_budget(op: &OpType) -> Result<(), AppError> {
    let (path, offsets): (&[String], Vec<usize>) = match op {
        OpType::Insert { path, index, .. }
        | OpType::InsertElement { path, index, .. }
        | OpType::RemoveElement { path, index, .. } => (path, vec![*index]),
        OpType::Delete {
            path,
            index,
            length,
            ..
        } => (path, vec![*index, *length]),
        OpType::MoveElement {
            path,
            from_index,
            to_index,
        } => (path, vec![*from_index, *to_index]),
        OpType::Replace { path, .. }
        | OpType::SetProperty { path, .. }
        | OpType::DeleteProperty { path, .. } => (path, vec![]),
    };
    if path.len() > 128 || offsets.iter().any(|offset| *offset > MAX_MESSAGE_BYTES) {
        return Err(AppError::Validation(
            "Operation path or index exceeds the document budget".into(),
        ));
    }
    Ok(())
}

fn validate_content(kind: &str, value: &Value) -> Result<(), AppError> {
    let result = match kind {
        "writer" => {
            serde_json::from_value::<syntra_doc_engine::WriterDocument>(value.clone()).map(|_| ())
        }
        "calc" => serde_json::from_value::<syntra_doc_engine::Workbook>(value.clone()).map(|_| ()),
        "slides" => {
            serde_json::from_value::<syntra_doc_engine::Presentation>(value.clone()).map(|_| ())
        }
        "whiteboard" => {
            serde_json::from_value::<syntra_doc_engine::Whiteboard>(value.clone()).map(|_| ())
        }
        _ => return Err(AppError::Validation("Unknown document kind".into())),
    };
    result.map_err(|e| AppError::Validation(format!("Invalid document operation: {e}")))
}

fn conflict() -> AppError {
    AppError::Conflict(
        "Operation no longer matches current content; synchronize before retrying".into(),
    )
}
fn invalid() -> AppError {
    AppError::Validation("Invalid document operation path or index".into())
}
fn target<'a>(root: &'a mut Value, kind: &str, path: &[String]) -> Result<&'a mut Value, AppError> {
    let path = if path
        .first()
        .is_some_and(|s| s == kind || (kind == "calc" && s == "workbook"))
    {
        &path[1..]
    } else {
        path
    };
    let mut value = root;
    for part in path {
        value = match value {
            Value::Array(array) => array
                .get_mut(part.parse::<usize>().map_err(|_| invalid())?)
                .ok_or_else(invalid)?,
            Value::Object(object) => object.get_mut(part).ok_or_else(invalid)?,
            _ => return Err(invalid()),
        };
    }
    Ok(value)
}

pub fn apply(root: &mut Value, kind: &str, op: &OpType) -> Result<(), AppError> {
    if let OpType::Replace {
        path,
        old_value,
        new_value,
    } = op
        && kind == "writer"
        && path == &["body_text".to_owned()]
    {
        let old = syntra_doc_engine::writer::writer_content_to_plain(root);
        if old_value.as_str() != Some(old.as_str()) {
            return Err(conflict());
        }
        *root = syntra_doc_engine::writer::writer_content_with_text(
            root,
            new_value.as_str().ok_or_else(invalid)?,
        );
        return Ok(());
    }
    match op {
        OpType::Replace {
            path,
            old_value,
            new_value,
        } => {
            let current = target(root, kind, path)?;
            if current != old_value {
                return Err(conflict());
            }
            *current = new_value.clone();
        }
        OpType::SetProperty {
            path,
            key,
            value,
            old_value,
        } => {
            if kind == "calc" && path.last().is_some_and(|part| part == "cells") {
                let cells = target(root, kind, path)?
                    .as_array_mut()
                    .ok_or_else(invalid)?;
                let (row, col) = key.split_once(':').ok_or_else(invalid)?;
                let (row, col) = (
                    row.parse::<u32>().map_err(|_| invalid())?,
                    col.parse::<u32>().map_err(|_| invalid())?,
                );
                if row >= 1_048_576 || col >= 16_384 {
                    return Err(invalid());
                }
                let index = cells
                    .iter()
                    .position(|pair| pair[0]["row"] == row && pair[0]["col"] == col);
                let existing = index.map(|i| cells[i][1].clone()).unwrap_or(Value::Null);
                if old_value.as_ref().is_some_and(|old| *old != existing) {
                    return Err(conflict());
                }
                if value.is_null() {
                    if let Some(i) = index {
                        cells.remove(i);
                    }
                } else {
                    let pair = serde_json::json!([{"row":row,"col":col},value]);
                    if let Some(i) = index {
                        cells[i] = pair;
                    } else {
                        cells.push(pair);
                    }
                }
            } else {
                let object = target(root, kind, path)?
                    .as_object_mut()
                    .ok_or_else(invalid)?;
                if old_value
                    .as_ref()
                    .is_some_and(|old| object.get(key).unwrap_or(&Value::Null) != old)
                {
                    return Err(conflict());
                }
                object.insert(key.clone(), value.clone());
            }
        }
        OpType::DeleteProperty { path, key } => {
            target(root, kind, path)?
                .as_object_mut()
                .ok_or_else(invalid)?
                .remove(key);
        }
        OpType::Insert {
            path,
            index,
            content,
        } => {
            let text = target(root, kind, path)?;
            let current = text.as_str().ok_or_else(invalid)?;
            let byte = current
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(current.len()))
                .nth(*index)
                .ok_or_else(invalid)?;
            let mut next = current.to_owned();
            next.insert_str(byte, content);
            *text = Value::String(next);
        }
        OpType::Delete {
            path,
            index,
            length,
            removed_content,
        } => {
            let text = target(root, kind, path)?;
            // The OT representation uses an empty delete as the idempotent
            // result of two removals of the same array element.
            if *length == 0 && text.is_array() {
                return Ok(());
            }
            let current = text.as_str().ok_or_else(invalid)?;
            let offsets: Vec<_> = current
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(current.len()))
                .collect();
            let end = index.checked_add(*length).ok_or_else(invalid)?;
            let (start, end) = (
                *offsets.get(*index).ok_or_else(invalid)?,
                *offsets.get(end).ok_or_else(invalid)?,
            );
            if removed_content
                .as_ref()
                .is_some_and(|removed| current[start..end] != *removed)
            {
                return Err(conflict());
            }
            let mut next = current.to_owned();
            next.replace_range(start..end, "");
            *text = Value::String(next);
        }
        OpType::InsertElement {
            path,
            index,
            element,
        } => {
            let array = target(root, kind, path)?
                .as_array_mut()
                .ok_or_else(invalid)?;
            if *index > array.len() {
                return Err(invalid());
            }
            array.insert(*index, element.clone());
        }
        OpType::RemoveElement {
            path,
            index,
            removed_element,
        } => {
            let array = target(root, kind, path)?
                .as_array_mut()
                .ok_or_else(invalid)?;
            let previous = array.get(*index).ok_or_else(invalid)?;
            if removed_element
                .as_ref()
                .is_some_and(|removed| removed != previous)
            {
                return Err(conflict());
            }
            array.remove(*index);
        }
        OpType::MoveElement {
            path,
            from_index,
            to_index,
        } => {
            let array = target(root, kind, path)?
                .as_array_mut()
                .ok_or_else(invalid)?;
            if *from_index >= array.len() || *to_index >= array.len() {
                return Err(invalid());
            }
            let moved = array.remove(*from_index);
            array.insert(*to_index, moved);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_use_unicode_character_offsets_and_check_old_values() {
        let mut value = serde_json::json!({"title":"甲乙"});
        apply(
            &mut value,
            "writer",
            &OpType::Insert {
                path: vec!["title".into()],
                index: 1,
                content: "中".into(),
            },
        )
        .unwrap();
        assert_eq!(value["title"], "甲中乙");
        apply(
            &mut value,
            "writer",
            &OpType::Delete {
                path: vec!["title".into()],
                index: 1,
                length: 1,
                removed_content: Some("中".into()),
            },
        )
        .unwrap();
        assert_eq!(value["title"], "甲乙");
        let before = value.clone();
        assert!(
            apply(
                &mut value,
                "writer",
                &OpType::Replace {
                    path: vec!["writer".into()],
                    old_value: Value::Null,
                    new_value: Value::Null
                }
            )
            .is_err()
        );
        assert_eq!(value, before);
    }

    #[test]
    fn untrusted_indices_are_bounded_before_ot_arithmetic() {
        assert!(
            validate_operation_budget(&OpType::Delete {
                path: vec!["title".into()],
                index: usize::MAX,
                length: usize::MAX,
                removed_content: None,
            })
            .is_err()
        );
        let mut aliased = OpType::Insert {
            path: vec!["writer".into(), "title".into()],
            index: 0,
            content: "x".into(),
        };
        canonicalize_operation(&mut aliased, "writer");
        assert!(matches!(aliased, OpType::Insert { path, .. } if path == ["title"]));
    }

    #[test]
    fn array_reordering_rebases_descendant_paths_and_concurrent_duplicate_removals() {
        let operation = |op_type| Operation::new(Uuid::new_v4(), Uuid::new_v4(), 1, op_type);
        for previous in [
            OpType::InsertElement {
                path: vec!["body".into()],
                index: 0,
                element: serde_json::json!({"text":"X"}),
            },
            OpType::RemoveElement {
                path: vec!["body".into()],
                index: 0,
                removed_element: None,
            },
            OpType::MoveElement {
                path: vec!["body".into()],
                from_index: 0,
                to_index: 1,
            },
        ] {
            let mut root = serde_json::json!({"body":[{"text":"A"},{"text":"B"}]});
            apply(&mut root, "writer", &previous).unwrap();
            let op = operation(OpType::Replace {
                path: vec!["body".into(), "1".into(), "text".into()],
                old_value: serde_json::json!("B"),
                new_value: serde_json::json!("edited-B"),
            });
            let rebased = rebase_operation(op, &operation(previous)).unwrap();
            apply(&mut root, "writer", &rebased.op_type).unwrap();
            assert!(
                root["body"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|block| block["text"] == "edited-B")
            );
            assert!(
                !root["body"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|block| block["text"] == "B")
            );
        }
        let removal = operation(OpType::RemoveElement {
            path: vec!["body".into()],
            index: 0,
            removed_element: None,
        });
        let mut root = serde_json::json!({"body":["A","B"]});
        apply(&mut root, "writer", &removal.op_type).unwrap();
        let no_op = rebase_operation(removal.clone(), &removal).unwrap();
        apply(&mut root, "writer", &no_op.op_type).unwrap();
        assert_eq!(root, serde_json::json!({"body":["B"]}));
    }

    #[test]
    fn concurrent_deletion_preserves_inserts_and_converges_in_both_application_orders() {
        let base = serde_json::json!({"title":"甲乙丙丁"});
        for start in 0..=4 {
            for end in start..=4 {
                for insertion in 0..=4 {
                    let deletion = OpType::Delete {
                        path: vec!["title".into()],
                        index: start,
                        length: end - start,
                        removed_content: None,
                    };
                    let insertion = Operation::new(
                        Uuid::new_v4(),
                        Uuid::new_v4(),
                        1,
                        OpType::Insert {
                            path: vec!["title".into()],
                            index: insertion,
                            content: "新X".into(),
                        },
                    );
                    let mut insert_first = base.clone();
                    apply(&mut insert_first, "writer", &insertion.op_type).unwrap();
                    let mut intent =
                        DeletionIntent::from_content(&base, "writer", &deletion).unwrap();
                    let deleting =
                        Operation::new(Uuid::new_v4(), Uuid::new_v4(), 1, deletion.clone());
                    intent.rebase(&deleting, &insertion).unwrap();
                    for effect in intent.effects() {
                        apply(&mut insert_first, "writer", &effect).unwrap();
                    }
                    let mut delete_first = base.clone();
                    apply(&mut delete_first, "writer", &deletion).unwrap();
                    let inserting = rebase_operation(insertion, &deleting).unwrap();
                    apply(&mut delete_first, "writer", &inserting.op_type).unwrap();
                    assert_eq!(insert_first, delete_first, "delete range {start}..{end}");
                    assert!(insert_first["title"].as_str().unwrap().contains("新X"));
                }
            }
        }
    }
    #[tokio::test]
    async fn missing_or_invalid_stored_snapshot_never_becomes_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::state::DbPool::with_timeouts(
            &dir.path().join("db.sqlite").to_string_lossy(),
            1,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(100),
        )
        .unwrap();
        let mut provider = db.clone();
        diesel::sql_query("CREATE TABLE documents(id TEXT PRIMARY KEY,content TEXT,version INTEGER,doc_type TEXT)").execute(&mut provider).await.unwrap();
        assert!(snapshot(&db, Uuid::new_v4()).await.is_err());
        let id = Uuid::new_v4();
        diesel::sql_query("INSERT INTO documents VALUES (?, 'invalid',1,'writer')")
            .bind::<diesel::sql_types::Text, _>(id.to_string())
            .execute(&mut provider)
            .await
            .unwrap();
        assert!(snapshot(&db, id).await.is_err());
    }
}
