-- Room enhancements: logos, colors, quotas, lifetime
ALTER TABLE rooms ADD COLUMN logo_url TEXT;
ALTER TABLE rooms ADD COLUMN color TEXT;
ALTER TABLE rooms ADD COLUMN storage_quota INTEGER;
ALTER TABLE rooms ADD COLUMN auto_delete_days INTEGER;
ALTER TABLE rooms ADD COLUMN auto_delete_enabled INTEGER NOT NULL DEFAULT 0;
