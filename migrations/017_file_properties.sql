CREATE TABLE IF NOT EXISTS file_properties (
    id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    prop_key TEXT NOT NULL,
    prop_value TEXT NOT NULL,
    UNIQUE(file_id, prop_key)
);
CREATE INDEX IF NOT EXISTS idx_file_properties_file ON file_properties(file_id);
