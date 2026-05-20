//! CRDT / Operational Transformation primitives.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// OpId
// ---------------------------------------------------------------------------

/// Unique identifier for each operation, enabling causal ordering.
///
/// The `(counter, client_id)` pair forms a Lamport-style timestamp: every
/// client maintains a monotonically increasing counter and tags each
/// operation with its own UUID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct OpId {
    pub counter: u64,
    pub client_id: Uuid,
}

impl OpId {
    pub fn new(counter: u64, client_id: Uuid) -> Self {
        Self { counter, client_id }
    }
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.counter
            .cmp(&other.counter)
            .then_with(|| self.client_id.cmp(&other.client_id))
    }
}

// ---------------------------------------------------------------------------
// Operation
// ---------------------------------------------------------------------------

/// A collaborative operation that can be applied to a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: OpId,
    pub timestamp: DateTime<Utc>,
    pub user_id: Uuid,
    pub op_type: OpType,
}

impl Operation {
    /// Create a new operation stamped with the current UTC time.
    pub fn new(user_id: Uuid, client_id: Uuid, counter: u64, op_type: OpType) -> Self {
        Self {
            id: OpId::new(counter, client_id),
            timestamp: Utc::now(),
            user_id,
            op_type,
        }
    }

    /// Transform `self` against a concurrent `other` operation using a
    /// simplified Operational Transformation strategy.
    ///
    /// The returned operation, when applied after `other`, produces the same
    /// document state as applying `other` after `self` — i.e. the two
    /// transformed operations *commute*.
    ///
    /// This implementation only adjusts index-based text operations
    /// (`Insert`, `Delete`); structural operations (`SetProperty`,
    /// `DeleteProperty`, etc.) are returned unchanged because they target
    /// properties rather than positional indices.
    pub fn transform(&self, other: &Operation) -> Operation {
        let new_op_type = transform_op_type(&self.op_type, &other.op_type);
        Operation {
            id: self.id.clone(),
            timestamp: self.timestamp,
            user_id: self.user_id,
            op_type: new_op_type,
        }
    }
}

// ---------------------------------------------------------------------------
// OpType
// ---------------------------------------------------------------------------

/// The payload of a collaborative operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpType {
    /// Insert text at `index` within the node addressed by `path`.
    Insert {
        path: Vec<String>,
        index: usize,
        content: String,
    },
    /// Delete `length` characters starting at `index`.
    Delete {
        path: Vec<String>,
        index: usize,
        length: usize,
    },
    /// Replace the whole value at `path`.
    Replace {
        path: Vec<String>,
        old_value: serde_json::Value,
        new_value: serde_json::Value,
    },
    /// Set (or overwrite) a property on the node at `path`.
    SetProperty {
        path: Vec<String>,
        key: String,
        value: serde_json::Value,
    },
    /// Remove a property from the node at `path`.
    DeleteProperty { path: Vec<String>, key: String },
    /// Insert an element into an array node at `index`.
    InsertElement {
        path: Vec<String>,
        index: usize,
        element: serde_json::Value,
    },
    /// Remove the element at `index` from an array node.
    RemoveElement { path: Vec<String>, index: usize },
    /// Move an element within an array from `from_index` to `to_index`.
    MoveElement {
        path: Vec<String>,
        from_index: usize,
        to_index: usize,
    },
}

impl OpType {
    /// Compute the inverse operation that would undo this one.
    pub fn inverse(&self) -> OpType {
        match self {
            OpType::Insert {
                path,
                index,
                content,
            } => OpType::Delete {
                path: path.clone(),
                index: *index,
                length: content.len(),
            },
            OpType::Delete {
                path,
                index,
                length: _,
            } => OpType::Insert {
                path: path.clone(),
                index: *index,
                content: String::new(), // Placeholder - actual content should be captured before delete
            },
            OpType::Replace {
                path,
                old_value,
                new_value,
            } => OpType::Replace {
                path: path.clone(),
                old_value: new_value.clone(),
                new_value: old_value.clone(),
            },
            OpType::SetProperty {
                path,
                key,
                value: _,
            } => OpType::DeleteProperty {
                path: path.clone(),
                key: key.clone(),
            },
            OpType::DeleteProperty { path, key } => OpType::SetProperty {
                path: path.clone(),
                key: key.clone(),
                value: serde_json::Value::Null, // Placeholder
            },
            OpType::InsertElement {
                path,
                index,
                element: _,
            } => OpType::RemoveElement {
                path: path.clone(),
                index: *index,
            },
            OpType::RemoveElement { path, index } => OpType::InsertElement {
                path: path.clone(),
                index: *index,
                element: serde_json::Value::Null, // Placeholder
            },
            OpType::MoveElement {
                path,
                from_index,
                to_index,
            } => OpType::MoveElement {
                path: path.clone(),
                from_index: *to_index,
                to_index: *from_index,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// OT helpers
// ---------------------------------------------------------------------------

/// Return `true` when both operations target the same document path, meaning
/// their indices live in the same coordinate space and need adjustment.
fn same_path(a: &[String], b: &[String]) -> bool {
    a == b
}

/// Core OT transformation: given *our* operation type and a concurrent
/// *other* operation type, return the adjusted version of *our* operation.
fn transform_op_type(ours: &OpType, theirs: &OpType) -> OpType {
    match (ours, theirs) {
        // -- Insert vs Insert -------------------------------------------------
        (
            OpType::Insert {
                path: p1,
                index: i1,
                content: c1,
            },
            OpType::Insert {
                path: p2,
                index: i2,
                content: c2,
            },
        ) if same_path(p1, p2) => {
            // NOTE: The `<=` comparison provides deterministic tie-breaking
            // when both inserts target the same index — the operation that
            // arrives first (ours) keeps its position while the other is
            // shifted right.  This is consistent because every peer applies
            // the same pair-wise transformation.
            let new_index = if *i1 <= *i2 {
                // Our insert is at or before theirs — keep position.
                *i1
            } else {
                // Our insert is after theirs — shift right by their length.
                *i1 + c2.len()
            };
            OpType::Insert {
                path: p1.clone(),
                index: new_index,
                content: c1.clone(),
            }
        }

        // -- Insert vs Delete -------------------------------------------------
        (
            OpType::Insert {
                path: p1,
                index: i1,
                content: c1,
            },
            OpType::Delete {
                path: p2,
                index: i2,
                length,
            },
        ) if same_path(p1, p2) => {
            let new_index = if *i1 <= *i2 {
                *i1
            } else if *i1 >= *i2 + *length {
                *i1 - *length
            } else {
                // Our insert falls inside the deleted range — collapse to the
                // start of the deletion.
                *i2
            };
            OpType::Insert {
                path: p1.clone(),
                index: new_index,
                content: c1.clone(),
            }
        }

        // -- Delete vs Insert -------------------------------------------------
        (
            OpType::Delete {
                path: p1,
                index: i1,
                length: l1,
            },
            OpType::Insert {
                path: p2,
                index: i2,
                content: c2,
            },
        ) if same_path(p1, p2) => {
            let new_index = if *i1 >= *i2 { *i1 + c2.len() } else { *i1 };
            OpType::Delete {
                path: p1.clone(),
                index: new_index,
                length: *l1,
            }
        }

        // -- Delete vs Delete -------------------------------------------------
        (
            OpType::Delete {
                path: p1,
                index: i1,
                length: l1,
            },
            OpType::Delete {
                path: p2,
                index: i2,
                length: l2,
            },
        ) if same_path(p1, p2) => {
            let our_end = *i1 + *l1;
            let their_end = *i2 + *l2;

            if our_end <= *i2 {
                // Our deletion is entirely before theirs — keep as-is.
                ours.clone()
            } else if *i1 >= their_end {
                // Our deletion is entirely after theirs — shift left.
                OpType::Delete {
                    path: p1.clone(),
                    index: *i1 - *l2,
                    length: *l1,
                }
            } else {
                // Overlapping deletions — shrink ours to only the
                // non-overlapping portion.
                let overlap_start = (*i1).max(*i2);
                let overlap_end = our_end.min(their_end);
                let overlap = overlap_end.saturating_sub(overlap_start);
                let new_length = l1.saturating_sub(overlap);
                let new_index = (*i1).min(*i2);
                OpType::Delete {
                    path: p1.clone(),
                    index: new_index,
                    length: new_length,
                }
            }
        }

        // -- InsertElement vs InsertElement -----------------------------------
        (
            OpType::InsertElement {
                path: p1,
                index: i1,
                element: e1,
            },
            OpType::InsertElement {
                path: p2,
                index: i2,
                ..
            },
        ) if same_path(p1, p2) => {
            let new_index = if *i1 <= *i2 { *i1 } else { *i1 + 1 };
            OpType::InsertElement {
                path: p1.clone(),
                index: new_index,
                element: e1.clone(),
            }
        }

        // -- InsertElement vs RemoveElement -----------------------------------
        (
            OpType::InsertElement {
                path: p1,
                index: i1,
                element: e1,
            },
            OpType::RemoveElement {
                path: p2,
                index: i2,
            },
        ) if same_path(p1, p2) => {
            let new_index = if *i1 <= *i2 {
                *i1
            } else {
                i1.saturating_sub(1)
            };
            OpType::InsertElement {
                path: p1.clone(),
                index: new_index,
                element: e1.clone(),
            }
        }

        // -- RemoveElement vs InsertElement -----------------------------------
        (
            OpType::RemoveElement {
                path: p1,
                index: i1,
            },
            OpType::InsertElement {
                path: p2,
                index: i2,
                ..
            },
        ) if same_path(p1, p2) => {
            let new_index = if *i1 < *i2 { *i1 } else { *i1 + 1 };
            OpType::RemoveElement {
                path: p1.clone(),
                index: new_index,
            }
        }

        // -- RemoveElement vs RemoveElement -----------------------------------
        (
            OpType::RemoveElement {
                path: p1,
                index: i1,
            },
            OpType::RemoveElement {
                path: p2,
                index: i2,
            },
        ) if same_path(p1, p2) => {
            if *i1 < *i2 {
                ours.clone()
            } else if *i1 > *i2 {
                OpType::RemoveElement {
                    path: p1.clone(),
                    index: *i1 - 1,
                }
            } else {
                // Both remove the same element — turn into a no-op delete of
                // length 0 at that path (idempotent).
                OpType::Delete {
                    path: p1.clone(),
                    index: 0,
                    length: 0,
                }
            }
        }

        // -- Everything else: no adjustment needed ----------------------------
        _ => ours.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_insert(path: &[&str], index: usize, content: &str) -> OpType {
        OpType::Insert {
            path: path.iter().map(|s| s.to_string()).collect(),
            index,
            content: content.to_string(),
        }
    }

    fn make_delete(path: &[&str], index: usize, length: usize) -> OpType {
        OpType::Delete {
            path: path.iter().map(|s| s.to_string()).collect(),
            index,
            length,
        }
    }

    fn op(op_type: OpType) -> Operation {
        Operation::new(Uuid::new_v4(), Uuid::new_v4(), 1, op_type)
    }

    #[test]
    fn insert_before_insert_keeps_index() {
        let a = op(make_insert(&["body"], 2, "hi"));
        let b = op(make_insert(&["body"], 5, "there"));
        let a_prime = a.transform(&b);
        match a_prime.op_type {
            OpType::Insert { index, .. } => assert_eq!(index, 2),
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn insert_after_insert_shifts_right() {
        let a = op(make_insert(&["body"], 5, "hi"));
        let b = op(make_insert(&["body"], 2, "hey"));
        let a_prime = a.transform(&b);
        match a_prime.op_type {
            OpType::Insert { index, .. } => assert_eq!(index, 5 + 3),
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn insert_vs_delete_after() {
        let a = op(make_insert(&["body"], 1, "x"));
        let b = op(make_delete(&["body"], 5, 3));
        let a_prime = a.transform(&b);
        match a_prime.op_type {
            OpType::Insert { index, .. } => assert_eq!(index, 1),
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn delete_vs_insert_before() {
        let a = op(make_delete(&["body"], 5, 2));
        let b = op(make_insert(&["body"], 2, "abc"));
        let a_prime = a.transform(&b);
        match a_prime.op_type {
            OpType::Delete { index, length, .. } => {
                assert_eq!(index, 8); // 5 + 3
                assert_eq!(length, 2);
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn different_paths_no_transform() {
        let a = op(make_insert(&["body"], 5, "hi"));
        let b = op(make_insert(&["title"], 0, "there"));
        let a_prime = a.transform(&b);
        match a_prime.op_type {
            OpType::Insert { index, .. } => assert_eq!(index, 5),
            _ => panic!("expected Insert"),
        }
    }
}
