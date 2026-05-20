-- Tags
CREATE TABLE IF NOT EXISTS tags (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    color TEXT,
    owner_id TEXT NOT NULL REFERENCES users(id),
    created_at TEXT NOT NULL,
    UNIQUE(name, owner_id)
);

CREATE TABLE IF NOT EXISTS file_tags (
    id TEXT PRIMARY KEY NOT NULL,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    UNIQUE(file_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_tags_owner ON tags(owner_id);
CREATE INDEX IF NOT EXISTS idx_file_tags_file ON file_tags(file_id);
CREATE INDEX IF NOT EXISTS idx_file_tags_tag ON file_tags(tag_id);

-- Favorites
CREATE TABLE IF NOT EXISTS favorites (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    UNIQUE(user_id, file_id)
);

CREATE INDEX IF NOT EXISTS idx_favorites_user ON favorites(user_id);

-- Recent files
CREATE TABLE IF NOT EXISTS recent_files (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    accessed_at TEXT NOT NULL,
    UNIQUE(user_id, file_id)
);

CREATE INDEX IF NOT EXISTS idx_recent_files_user ON recent_files(user_id);
