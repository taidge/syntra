CREATE TABLE IF NOT EXISTS file_read_status (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    read_at TEXT NOT NULL,
    UNIQUE(user_id, file_id)
);
CREATE INDEX IF NOT EXISTS idx_file_read_status_user ON file_read_status(user_id);
