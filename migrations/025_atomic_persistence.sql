-- Revoke ambiguous legacy locks rather than selecting an arbitrary holder.
DELETE FROM file_locks WHERE file_id IN (SELECT file_id FROM file_locks GROUP BY file_id HAVING COUNT(*) > 1);
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_locks_exclusive ON file_locks(file_id);
CREATE TABLE IF NOT EXISTS upload_chunks (
    session_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL CHECK(chunk_index > 0),
    path TEXT NOT NULL,
    size INTEGER NOT NULL CHECK(size >= 0),
    digest TEXT NOT NULL,
    PRIMARY KEY(session_id, chunk_index)
);
CREATE TABLE IF NOT EXISTS upload_results (
    session_id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS storage_gc (
    path TEXT PRIMARY KEY NOT NULL,
    created_at TEXT NOT NULL
);

-- Request receipts are independent of the bounded transformed OT history.
CREATE TABLE IF NOT EXISTS operation_receipts (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    counter INTEGER NOT NULL CHECK(counter >= 0),
    user_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    version INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_operation_receipts_document ON operation_receipts(document_id, version);
CREATE TABLE IF NOT EXISTS operation_clients (
    document_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    max_counter INTEGER NOT NULL CHECK(max_counter >= 0),
    PRIMARY KEY(document_id, client_id)
);
-- Legacy logs do not contain a pre-transform request hash. Preserve their
-- high-water marks and require synchronization instead of guessing a receipt.
WITH legacy AS (
    SELECT document_id, user_id,
        CASE WHEN json_valid(op_data) THEN json_extract(op_data, '$.id.client_id') END AS client_id,
        CASE WHEN json_valid(op_data) THEN json_extract(op_data, '$.id.counter') END AS counter
    FROM operation_log
)
INSERT OR IGNORE INTO operation_clients(document_id, client_id, user_id, max_counter)
SELECT document_id, client_id, MIN(user_id), MAX(counter)
FROM legacy WHERE typeof(client_id) = 'text' AND typeof(counter) = 'integer' AND counter >= 0
GROUP BY document_id, client_id;

-- Only additionally charged retired objects enter this ledger, legacy file
-- version rows must not cause deductions from unrelated current-file usage.
CREATE TABLE IF NOT EXISTS retained_blobs (
    owner_id TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL CHECK(size >= 0),
    PRIMARY KEY(owner_id, path)
);
