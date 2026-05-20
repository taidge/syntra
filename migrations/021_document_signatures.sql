-- Document signing
CREATE TABLE IF NOT EXISTS document_signatures (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id),
    user_id TEXT NOT NULL REFERENCES users(id),
    signature_data TEXT NOT NULL,
    signed_hash TEXT NOT NULL,
    signed_at TEXT NOT NULL,
    is_valid INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_doc_signatures_doc ON document_signatures(document_id);
