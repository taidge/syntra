-- Branding settings
CREATE TABLE IF NOT EXISTS branding_settings (
    id TEXT PRIMARY KEY NOT NULL,
    logo_url TEXT,
    favicon_url TEXT,
    app_name TEXT NOT NULL DEFAULT 'Contrix Office',
    primary_color TEXT,
    custom_css TEXT,
    updated_by TEXT NOT NULL REFERENCES users(id),
    updated_at TEXT NOT NULL
);

-- SMTP settings
CREATE TABLE IF NOT EXISTS smtp_settings (
    id TEXT PRIMARY KEY NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 587,
    username TEXT,
    password TEXT,
    from_email TEXT NOT NULL,
    from_name TEXT NOT NULL DEFAULT 'Contrix Office',
    use_tls INTEGER NOT NULL DEFAULT 1,
    updated_by TEXT NOT NULL REFERENCES users(id),
    updated_at TEXT NOT NULL
);

-- Add ip_address column to activity_logs for enhanced audit
ALTER TABLE activity_logs ADD COLUMN ip_address TEXT;

-- Add user_agent column to activity_logs
ALTER TABLE activity_logs ADD COLUMN user_agent TEXT;
