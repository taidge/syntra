-- Sessions
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    token_hash TEXT NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_token_hash ON sessions(token_hash);

-- TOTP secrets
CREATE TABLE IF NOT EXISTS totp_secrets (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) UNIQUE,
    secret TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 0,
    backup_codes TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Password policies
CREATE TABLE IF NOT EXISTS password_policies (
    id TEXT PRIMARY KEY NOT NULL,
    min_length INTEGER NOT NULL DEFAULT 8,
    require_uppercase INTEGER NOT NULL DEFAULT 1,
    require_lowercase INTEGER NOT NULL DEFAULT 1,
    require_digit INTEGER NOT NULL DEFAULT 1,
    require_special INTEGER NOT NULL DEFAULT 0,
    max_age_days INTEGER,
    updated_by TEXT NOT NULL REFERENCES users(id),
    updated_at TEXT NOT NULL
);

-- IP restrictions
CREATE TABLE IF NOT EXISTS ip_restrictions (
    id TEXT PRIMARY KEY NOT NULL,
    ip_range TEXT NOT NULL,
    restriction_type TEXT NOT NULL DEFAULT 'allow',
    description TEXT,
    created_by TEXT NOT NULL REFERENCES users(id),
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ip_restrictions_type ON ip_restrictions(restriction_type);
