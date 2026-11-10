-- A durable fence closes the gap between reference checks and object deletion.
-- Deleted rows remain as tombstones, preventing delayed writers reviving a key.
CREATE TABLE IF NOT EXISTS blob_inventory (
    path TEXT PRIMARY KEY NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('observed','writing','ready','deleting','deleted')),
    first_seen INTEGER NOT NULL,
    lease_until INTEGER NOT NULL DEFAULT 0,
    token TEXT
);
ALTER TABLE storage_gc ADD COLUMN claim_token TEXT;
ALTER TABLE storage_gc ADD COLUMN claim_until INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_blob_inventory_lease ON blob_inventory(state,lease_until);
CREATE INDEX IF NOT EXISTS idx_versions_retention ON versions(document_id,version_number DESC,created_at DESC,id DESC);
CREATE INDEX IF NOT EXISTS idx_file_versions_retention ON file_versions(file_id,version_number DESC,created_at DESC,id DESC);
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT path,'ready',0 FROM files WHERE path IS NOT NULL AND path != '';
CREATE TRIGGER IF NOT EXISTS fence_files_path_insert
BEFORE INSERT ON files
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_files_path_update
BEFORE UPDATE OF path ON files
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT thumbnail_path,'ready',0 FROM files WHERE thumbnail_path IS NOT NULL AND thumbnail_path != '';
CREATE TRIGGER IF NOT EXISTS fence_files_thumbnail_path_insert
BEFORE INSERT ON files
WHEN NEW.thumbnail_path IS NOT NULL AND NEW.thumbnail_path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.thumbnail_path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_files_thumbnail_path_update
BEFORE UPDATE OF thumbnail_path ON files
WHEN NEW.thumbnail_path IS NOT NULL AND NEW.thumbnail_path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.thumbnail_path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT path,'ready',0 FROM file_versions WHERE path IS NOT NULL AND path != '';
CREATE TRIGGER IF NOT EXISTS fence_file_versions_path_insert
BEFORE INSERT ON file_versions
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_file_versions_path_update
BEFORE UPDATE OF path ON file_versions
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT path,'ready',0 FROM upload_chunks WHERE path IS NOT NULL AND path != '';
CREATE TRIGGER IF NOT EXISTS fence_upload_chunks_path_insert
BEFORE INSERT ON upload_chunks
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_upload_chunks_path_update
BEFORE UPDATE OF path ON upload_chunks
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT output_path,'ready',0 FROM conversion_jobs WHERE output_path IS NOT NULL AND output_path != '';
CREATE TRIGGER IF NOT EXISTS fence_conversion_jobs_output_path_insert
BEFORE INSERT ON conversion_jobs
WHEN NEW.output_path IS NOT NULL AND NEW.output_path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.output_path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_conversion_jobs_output_path_update
BEFORE UPDATE OF output_path ON conversion_jobs
WHEN NEW.output_path IS NOT NULL AND NEW.output_path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.output_path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) SELECT path,'ready',0 FROM retained_blobs WHERE path IS NOT NULL AND path != '';
CREATE TRIGGER IF NOT EXISTS fence_retained_blobs_path_insert
BEFORE INSERT ON retained_blobs
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE TRIGGER IF NOT EXISTS fence_retained_blobs_path_update
BEFORE UPDATE OF path ON retained_blobs
WHEN NEW.path IS NOT NULL AND NEW.path != '' AND EXISTS (
    SELECT 1 FROM blob_inventory WHERE path=NEW.path AND state IN ('writing','deleting','deleted')
)
BEGIN
    SELECT RAISE(ABORT, 'blob is not attachable, retry with a new object key');
END;
CREATE INDEX IF NOT EXISTS idx_files_blob_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_thumbnail_path ON files(thumbnail_path);
CREATE INDEX IF NOT EXISTS idx_file_versions_blob_path ON file_versions(path);
CREATE INDEX IF NOT EXISTS idx_upload_chunks_blob_path ON upload_chunks(path);
CREATE INDEX IF NOT EXISTS idx_conversion_output_path ON conversion_jobs(output_path);
CREATE INDEX IF NOT EXISTS idx_retained_blob_path ON retained_blobs(path);
CREATE INDEX IF NOT EXISTS idx_storage_gc_ready ON storage_gc(claim_until,created_at,path);
CREATE INDEX IF NOT EXISTS idx_storage_gc_order ON storage_gc(created_at,path);

-- Preserve any legacy originals, but prevent new duplicate version-zero rows.
CREATE TRIGGER IF NOT EXISTS file_versions_one_new_original
BEFORE INSERT ON file_versions
WHEN NEW.version_number=0 AND EXISTS(SELECT 1 FROM file_versions WHERE file_id=NEW.file_id AND version_number=0)
BEGIN
    SELECT RAISE(ABORT, 'original version already exists');
END;
CREATE TRIGGER IF NOT EXISTS file_versions_one_updated_original
BEFORE UPDATE OF file_id,version_number ON file_versions
WHEN NEW.version_number=0 AND EXISTS(SELECT 1 FROM file_versions WHERE file_id=NEW.file_id AND version_number=0 AND id!=NEW.id)
BEGIN
    SELECT RAISE(ABORT, 'original version already exists');
END;
CREATE INDEX IF NOT EXISTS idx_upload_reservations ON upload_sessions(user_id,status,expires_at);
CREATE INDEX IF NOT EXISTS idx_upload_expiration ON upload_sessions(status,expires_at,id);

-- Retiring a referenced GC hint is safe because every path reference update or
-- deletion re-enqueues the old key in the same transaction. Triggers also cover
-- metadata changes outside the normal handler helpers.
CREATE TRIGGER IF NOT EXISTS retire_files_path_update
AFTER UPDATE OF path ON files
WHEN OLD.path IS NOT NULL AND OLD.path != '' AND OLD.is_folder=0 AND OLD.path IS NOT NEW.path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_files_path_delete
AFTER DELETE ON files
WHEN OLD.path IS NOT NULL AND OLD.path != '' AND OLD.is_folder=0
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_files_thumbnail_path_update
AFTER UPDATE OF thumbnail_path ON files
WHEN OLD.thumbnail_path IS NOT NULL AND OLD.thumbnail_path != '' AND OLD.thumbnail_path IS NOT NEW.thumbnail_path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.thumbnail_path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_files_thumbnail_path_delete
AFTER DELETE ON files
WHEN OLD.thumbnail_path IS NOT NULL AND OLD.thumbnail_path != ''
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.thumbnail_path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_file_versions_path_update
AFTER UPDATE OF path ON file_versions
WHEN OLD.path IS NOT NULL AND OLD.path != '' AND OLD.path IS NOT NEW.path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_file_versions_path_delete
AFTER DELETE ON file_versions
WHEN OLD.path IS NOT NULL AND OLD.path != ''
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_upload_chunks_path_update
AFTER UPDATE OF path ON upload_chunks
WHEN OLD.path IS NOT NULL AND OLD.path != '' AND OLD.path IS NOT NEW.path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_upload_chunks_path_delete
AFTER DELETE ON upload_chunks
WHEN OLD.path IS NOT NULL AND OLD.path != ''
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_conversion_jobs_output_path_update
AFTER UPDATE OF output_path ON conversion_jobs
WHEN OLD.output_path IS NOT NULL AND OLD.output_path != '' AND OLD.output_path IS NOT NEW.output_path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.output_path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_conversion_jobs_output_path_delete
AFTER DELETE ON conversion_jobs
WHEN OLD.output_path IS NOT NULL AND OLD.output_path != ''
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.output_path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_retained_blobs_path_update
AFTER UPDATE OF path ON retained_blobs
WHEN OLD.path IS NOT NULL AND OLD.path != '' AND OLD.path IS NOT NEW.path
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
CREATE TRIGGER IF NOT EXISTS retire_retained_blobs_path_delete
AFTER DELETE ON retained_blobs
WHEN OLD.path IS NOT NULL AND OLD.path != ''
BEGIN
    INSERT OR IGNORE INTO storage_gc(path,created_at) VALUES (OLD.path,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
END;
