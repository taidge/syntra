-- Support bounded retention reads and deletes, including equal-version rows
-- left by older databases. The id tie-breaker avoids sorting an unbounded tie.
CREATE INDEX IF NOT EXISTS idx_operation_log_retention
ON operation_log(document_id, version DESC, id DESC);
