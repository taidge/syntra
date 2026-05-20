CREATE TABLE IF NOT EXISTS operation_log (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    op_type TEXT NOT NULL,
    op_data TEXT NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_operation_log_doc_version ON operation_log(document_id, version);
