-- Timestamp equality cannot distinguish earlier deletions from a subtree
-- deletion. Retain explicit membership only for files trashed by that action.
CREATE TABLE IF NOT EXISTS trash_batch_members (
    file_id TEXT PRIMARY KEY NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    batch_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trash_batch_members_batch
    ON trash_batch_members(batch_id);
