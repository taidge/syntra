-- File locks for collaborative editing
CREATE TABLE IF NOT EXISTS file_locks (
    id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    locked_by TEXT NOT NULL REFERENCES users(id),
    lock_token TEXT NOT NULL UNIQUE,
    lock_type TEXT NOT NULL DEFAULT 'exclusive',
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_locks_file ON file_locks(file_id);
CREATE INDEX IF NOT EXISTS idx_file_locks_token ON file_locks(lock_token);

-- User presence per document
CREATE TABLE IF NOT EXISTS user_presence (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id),
    session_id TEXT,
    cursor_position TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    last_seen_at TEXT NOT NULL,
    joined_at TEXT NOT NULL,
    UNIQUE(document_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_user_presence_doc ON user_presence(document_id);
CREATE INDEX IF NOT EXISTS idx_user_presence_user ON user_presence(user_id);

-- User settings (per-user key-value store)
CREATE TABLE IF NOT EXISTS user_settings (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    setting_key TEXT NOT NULL,
    setting_value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(user_id, setting_key)
);

CREATE INDEX IF NOT EXISTS idx_user_settings_user ON user_settings(user_id);

-- Watermark configuration
CREATE TABLE IF NOT EXISTS watermark_settings (
    id TEXT PRIMARY KEY NOT NULL,
    text TEXT,
    opacity REAL NOT NULL DEFAULT 0.2,
    font_size INTEGER NOT NULL DEFAULT 48,
    rotation INTEGER NOT NULL DEFAULT -45,
    is_enabled INTEGER NOT NULL DEFAULT 0,
    updated_by TEXT NOT NULL REFERENCES users(id),
    updated_at TEXT NOT NULL
);

-- Client error logs
CREATE TABLE IF NOT EXISTS client_logs (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT,
    session_id TEXT,
    level TEXT NOT NULL DEFAULT 'error',
    message TEXT NOT NULL,
    stack_trace TEXT,
    user_agent TEXT,
    url TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_client_logs_created ON client_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_client_logs_level ON client_logs(level);

-- Language tool / translation service configuration
CREATE TABLE IF NOT EXISTS integration_settings (
    id TEXT PRIMARY KEY NOT NULL,
    service_name TEXT NOT NULL UNIQUE,
    is_enabled INTEGER NOT NULL DEFAULT 0,
    api_url TEXT,
    api_key TEXT,
    extra_config TEXT,
    updated_by TEXT NOT NULL REFERENCES users(id),
    updated_at TEXT NOT NULL
);

-- Conversion jobs
CREATE TABLE IF NOT EXISTS conversion_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    source_file_id TEXT NOT NULL REFERENCES files(id),
    target_format TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    output_path TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_conversion_jobs_user ON conversion_jobs(user_id);
CREATE INDEX IF NOT EXISTS idx_conversion_jobs_status ON conversion_jobs(status);
