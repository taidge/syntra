//! Shared SQLite migration execution for production startup and test servers.
use anyhow::{Context, bail};
use diesel::SqliteConnection;
use diesel::connection::SimpleConnection;
use std::path::Path;

pub const MIGRATION_FILES: &[&str] = &[
    "001_init.sql",
    "002_rooms_groups.sql",
    "003_tags_favorites_recent.sql",
    "004_security.sql",
    "005_webhooks.sql",
    "006_admin.sql",
    "007_templates_invitations.sql",
    "008_upload_thumbnails.sql",
    "009_wopi_locking_presence.sql",
    "010_password_resets.sql",
    "011_share_passwords.sql",
    "012_notification_preferences.sql",
    "013_file_versions.sql",
    "014_operation_log.sql",
    "015_api_keys.sql",
    "016_login_attempts.sql",
    "017_file_properties.sql",
    "018_file_ordering.sql",
    "019_read_status.sql",
    "020_room_enhancements.sql",
    "021_document_signatures.sql",
    "022_tenants.sql",
    "023_remove_oauth.sql",
    "024_remove_integration_settings.sql",
    "025_atomic_persistence.sql",
    "026_storage_maintenance.sql",
    "027_operation_history_retention.sql",
    "028_trash_deletion_batches.sql",
    "029_syntra_branding.sql",
];

#[derive(Clone, Copy, PartialEq)]
enum LexicalState {
    Sql,
    Single,
    Double,
    Backtick,
    Bracket,
    LineComment,
    BlockComment,
}

/// Split only outside SQLite strings, quoted identifiers and comments. Trigger
/// bodies may span fragments; SQLite's parser decides when they are complete.
fn sql_fragments(sql: &str) -> anyhow::Result<Vec<String>> {
    use LexicalState::*;
    let bytes = sql.as_bytes();
    let mut fragments = Vec::new();
    let mut current = Vec::new();
    let mut state = Sql;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            Sql => match (byte, next) {
                (b'-', Some(b'-')) => {
                    current.push(b' ');
                    state = LineComment;
                    index += 2;
                    continue;
                }
                (b'/', Some(b'*')) => {
                    current.push(b' ');
                    state = BlockComment;
                    index += 2;
                    continue;
                }
                (b'\'', _) => state = Single,
                (b'"', _) => state = Double,
                (b'`', _) => state = Backtick,
                (b'[', _) => state = Bracket,
                (b';', _) => {
                    current.push(byte);
                    let fragment = String::from_utf8(std::mem::take(&mut current))?;
                    if !fragment.trim().trim_matches(';').trim().is_empty() {
                        fragments.push(fragment);
                    }
                    index += 1;
                    continue;
                }
                _ => {}
            },
            LineComment => {
                if matches!(byte, b'\n' | b'\r') {
                    current.push(byte);
                    state = Sql;
                }
                index += 1;
                continue;
            }
            BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    state = Sql;
                    index += 2;
                    continue;
                }
                if matches!(byte, b'\n' | b'\r') {
                    current.push(byte);
                }
                index += 1;
                continue;
            }
            Single | Double | Backtick => {
                let quote = match state {
                    Single => b'\'',
                    Double => b'"',
                    _ => b'`',
                };
                if byte == quote {
                    if next == Some(quote) {
                        current.extend_from_slice(&[byte, byte]);
                        index += 2;
                        continue;
                    }
                    state = Sql;
                }
            }
            Bracket => {
                if byte == b']' {
                    state = Sql;
                }
            }
        }
        current.push(byte);
        index += 1;
    }
    if !matches!(state, Sql | LineComment) {
        bail!("Unterminated SQL string, quoted identifier or block comment");
    }
    let tail = String::from_utf8(current)?;
    if !tail.trim().is_empty() {
        fragments.push(tail);
    }
    Ok(fragments)
}

fn duplicate_add_column(error: &diesel::result::Error, statement: &str) -> bool {
    let normalized = statement
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    normalized.starts_with("alter table ")
        && normalized.contains(" add column ")
        && error
            .to_string()
            .to_ascii_lowercase()
            .contains("duplicate column")
}

/// A file commits atomically. Duplicate ADD COLUMN is the sole compatibility
/// exception for databases initialized before a migration ledger existed.
pub fn run_migration_file(
    conn: &mut SqliteConnection,
    name: &str,
    sql: &str,
) -> anyhow::Result<()> {
    let fragments = sql_fragments(sql).with_context(|| format!("Cannot parse migration {name}"))?;
    conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
        let mut pending = String::new();
        let mut incomplete = None;
        for fragment in fragments {
            pending.push_str(&fragment);
            match conn.batch_execute(&pending) {
                Ok(()) => {
                    pending.clear();
                    incomplete = None;
                }
                Err(error) if error.to_string().eq_ignore_ascii_case("incomplete input") => {
                    // SQLite, rather than a second SQL grammar, recognizes
                    // BEGIN/CASE/END inside CREATE TRIGGER. No partial trigger
                    // is executed before SQLite accepts its complete body.
                    incomplete = Some(error);
                }
                Err(error) if duplicate_add_column(&error, &pending) => {
                    tracing::debug!(migration = name, %error, "Existing migration column retained");
                    pending.clear();
                    incomplete = None;
                }
                Err(error) => {
                    let preview: String = pending.chars().take(240).collect();
                    return Err(anyhow::Error::new(error)
                        .context(format!("Migration {name} failed; SQL: {preview}")));
                }
            }
        }
        if let Some(error) = incomplete {
            return Err(anyhow::Error::new(error).context(format!("Incomplete migration {name}")));
        }
        Ok(())
    })
}

pub fn run_directory(conn: &mut SqliteConnection, directory: &Path) -> anyhow::Result<()> {
    for name in MIGRATION_FILES {
        let sql = std::fs::read_to_string(directory.join(name))
            .with_context(|| format!("Cannot read required migration {name}"))?;
        run_migration_file(conn, name, &sql)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::Connection;
    use diesel::sql_types::{BigInt, Text};

    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        total: i64,
    }
    fn count(conn: &mut SqliteConnection, sql: &str) -> i64 {
        diesel::RunQueryDsl::get_result::<Count>(diesel::sql_query(sql), conn)
            .unwrap()
            .total
    }

    #[test]
    fn comments_quotes_unicode_and_trigger_bodies_use_sqlite_boundaries() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        run_migration_file(
            &mut conn,
            "quotes",
            r#"
            -- a comment; that is not SQL
            CREATE TABLE "中文; table" ([value;col] TEXT, `other;col` TEXT);
            /* block; comment */ CREATE TABLE audit(note TEXT);
            CREATE TRIGGER log AFTER INSERT ON "中文; table" BEGIN
                INSERT INTO audit VALUES ('quoted ''; text');
                INSERT INTO audit VALUES (CASE WHEN 1 THEN '中文;值' ELSE 'else;' END);
            END;
            INSERT INTO "中文; table" VALUES ('one;two', 'three;four'); -- tail;
        "#,
        )
        .unwrap();
        assert_eq!(count(&mut conn, "SELECT COUNT(*) AS total FROM audit"), 2);
        #[derive(diesel::QueryableByName)]
        struct Note {
            #[diesel(sql_type = Text)]
            note: String,
        }
        let note = diesel::RunQueryDsl::get_result::<Note>(
            diesel::sql_query("SELECT note FROM audit ORDER BY rowid LIMIT 1"),
            &mut conn,
        )
        .unwrap();
        assert_eq!(note.note, "quoted '; text");
    }

    #[test]
    fn failures_roll_back_the_file_and_only_duplicate_added_columns_are_tolerated() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        let sql = "CREATE TABLE IF NOT EXISTS entries(id INTEGER); ALTER TABLE entries ADD COLUMN note TEXT DEFAULT 'a;b';";
        run_migration_file(&mut conn, "repeat", sql).unwrap();
        run_migration_file(&mut conn, "repeat", sql).unwrap();
        assert!(
            run_migration_file(
                &mut conn,
                "bad",
                "CREATE TABLE transient(id INTEGER); INSERT INTO absent VALUES (1);"
            )
            .is_err()
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM sqlite_master WHERE name='transient'"
            ),
            0
        );
        assert!(run_migration_file(&mut conn,"trigger","CREATE TABLE another(id INTEGER); CREATE TRIGGER unfinished AFTER INSERT ON another BEGIN SELECT 1;").is_err());
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM sqlite_master WHERE name='another'"
            ),
            0
        );
        assert!(run_migration_file(&mut conn, "quote", "SELECT 'unterminated;").is_err());
        assert!(run_migration_file(&mut conn, "comment", "SELECT 1; /* unterminated;").is_err());
    }

    #[test]
    fn all_real_migrations_support_two_starts_without_losing_business_rows() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
        run_directory(&mut conn, &directory).unwrap();
        conn.batch_execute("INSERT INTO users(id,username,email,password_hash) VALUES ('survivor','survivor','survivor@example.test','hash')").unwrap();
        run_directory(&mut conn, &directory).unwrap();
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM users WHERE id='survivor'"
            ),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM sqlite_master WHERE type='table' AND name IN ('retained_blobs','operation_receipts','operation_clients','storage_gc','upload_chunks','upload_results')"
            ),
            6
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM pragma_table_info('users') WHERE name='tenant_id'"
            ),
            1
        );
    }

    #[test]
    fn trash_batches_upgrade_legacy_databases_without_guessing_membership() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
        for name in MIGRATION_FILES
            .iter()
            .filter(|name| **name != "028_trash_deletion_batches.sql")
        {
            let sql = std::fs::read_to_string(directory.join(name)).unwrap();
            run_migration_file(&mut conn, name, &sql).unwrap();
        }
        conn.batch_execute("INSERT INTO users(id,username,email,password_hash) VALUES ('legacy','legacy','legacy@example.test','hash');
            INSERT INTO files(id,name,path,owner_id,is_folder,is_trashed,trashed_at) VALUES ('root','root','','legacy',1,1,'2026-01-01T00:00:00Z');
            INSERT INTO files(id,name,path,owner_id,parent_id,is_trashed,trashed_at) VALUES ('child','child','','legacy','root',1,'2026-01-01T00:00:00Z');").unwrap();
        run_directory(&mut conn, &directory).unwrap();
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM trash_batch_members"
            ),
            0
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM files WHERE is_trashed=1 AND trashed_at='2026-01-01T00:00:00Z'"
            ),
            2
        );
        conn.batch_execute(
            "INSERT INTO trash_batch_members(file_id,batch_id) VALUES ('root','persisted-batch')",
        )
        .unwrap();
        run_directory(&mut conn, &directory).unwrap();
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM trash_batch_members WHERE file_id='root' AND batch_id='persisted-batch'"
            ),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS total FROM files WHERE id='child' AND parent_id='root' AND is_trashed=1"
            ),
            1
        );
    }
}
