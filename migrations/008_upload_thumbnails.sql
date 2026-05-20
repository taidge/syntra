-- Upload sessions for chunked uploads
CREATE TABLE IF NOT EXISTS upload_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    file_name TEXT NOT NULL,
    mime_type TEXT,
    total_size INTEGER NOT NULL,
    uploaded_size INTEGER NOT NULL DEFAULT 0,
    chunk_count INTEGER NOT NULL DEFAULT 0,
    parent_id TEXT,
    room_id TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_upload_sessions_user ON upload_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_upload_sessions_status ON upload_sessions(status);

-- Add thumbnail_path to files
ALTER TABLE files ADD COLUMN thumbnail_path TEXT;
