-- Multi-tenant support
CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    domain TEXT UNIQUE,
    owner_id TEXT REFERENCES users(id),
    plan TEXT NOT NULL DEFAULT 'free',
    max_users INTEGER NOT NULL DEFAULT 10,
    max_storage_bytes INTEGER NOT NULL DEFAULT 5368709120,
    is_active INTEGER NOT NULL DEFAULT 1,
    settings TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tenants_domain ON tenants(domain);

-- Add tenant_id to users
ALTER TABLE users ADD COLUMN tenant_id TEXT REFERENCES tenants(id);
