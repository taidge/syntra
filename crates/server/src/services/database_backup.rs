use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use chrono::Utc;
use diesel::prelude::*;
use diesel::sql_types::Text;
use uuid::Uuid;

#[derive(QueryableByName)]
struct IntegrityCheck {
    #[diesel(sql_type = Text)]
    quick_check: String,
}

/// Convert a SQLite URL into the filesystem path used by Diesel.
pub fn sqlite_url_to_path(url: &str) -> anyhow::Result<PathBuf> {
    let path = if let Some(path) = url.strip_prefix("sqlite://") {
        path.split('?').next().unwrap_or(path)
    } else {
        if url.contains("://") {
            bail!("Unsupported database URL scheme. This build only supports SQLite");
        }
        url.split('?').next().unwrap_or(url)
    };

    if path.is_empty() {
        bail!("SQLite database path must not be empty");
    }
    Ok(PathBuf::from(path))
}

/// Keep backups beside the configured database instead of in a process-wide
/// relative directory. This isolates test instances and multi-instance setups.
pub fn backup_directory(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("backups")
}

pub fn pending_restore_path(database_path: &Path) -> PathBuf {
    with_suffix(database_path, ".restore-pending")
}

pub fn is_file_backed(database_path: &Path) -> bool {
    database_path != Path::new(":memory:")
}

pub fn is_valid_backup_filename(filename: &str) -> bool {
    let Some(stem) = filename
        .strip_prefix("backup_")
        .and_then(|name| name.strip_suffix(".db"))
    else {
        return false;
    };

    !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b'_' | b'-'))
        && Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(filename)
}

pub fn new_backup_filename(now: chrono::DateTime<Utc>) -> String {
    let unique = Uuid::new_v4().simple().to_string();
    format!(
        "backup_{}_{}.db",
        now.format("%Y%m%d_%H%M%S_%3f"),
        &unique[..8]
    )
}

/// Verify both the SQLite header and SQLite's own structural quick check.
pub fn validate_sqlite_file(path: &Path) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Could not read SQLite backup {}", path.display()))?;
    let mut header = [0u8; 16];
    file.read_exact(&mut header)
        .with_context(|| format!("Could not read SQLite header from {}", path.display()))?;
    if &header != b"SQLite format 3\0" {
        bail!("Backup is not a SQLite 3 database");
    }

    let mut conn = SqliteConnection::establish(&path.to_string_lossy())
        .with_context(|| format!("Could not open SQLite backup {}", path.display()))?;
    diesel::sql_query("PRAGMA query_only = ON")
        .execute(&mut conn)
        .context("Could not make backup validation read-only")?;
    let result = diesel::sql_query("PRAGMA quick_check")
        .get_result::<IntegrityCheck>(&mut conn)
        .context("SQLite backup integrity check failed")?;
    if result.quick_check != "ok" {
        bail!(
            "SQLite backup integrity check reported: {}",
            result.quick_check
        );
    }
    Ok(())
}

/// Copy a selected backup into a validated pending file. The live database is
/// deliberately untouched; the switch happens before the pool opens at restart.
pub async fn stage_restore(backup_path: &Path, database_path: &Path) -> anyhow::Result<PathBuf> {
    let pending = pending_restore_path(database_path);
    let temporary = with_suffix(&pending, &format!(".tmp-{}", Uuid::new_v4().simple()));

    tokio::fs::copy(backup_path, &temporary)
        .await
        .with_context(|| format!("Could not stage backup {}", backup_path.display()))?;
    let validation_path = temporary.clone();
    let validation =
        match tokio::task::spawn_blocking(move || validate_sqlite_file(&validation_path)).await {
            Ok(validation) => validation,
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error).context("Backup validation task failed");
            }
        };
    if let Err(error) = validation {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }

    let displaced = with_suffix(
        &pending,
        &format!(".superseded-{}", Uuid::new_v4().simple()),
    );
    let had_pending = tokio::fs::try_exists(&pending)
        .await
        .context("Could not inspect the previously staged restore")?;
    if had_pending {
        tokio::fs::rename(&pending, &displaced)
            .await
            .context("Could not preserve the previously staged restore")?;
    }
    if let Err(error) = tokio::fs::rename(&temporary, &pending).await {
        if had_pending {
            let _ = tokio::fs::rename(&displaced, &pending).await;
        }
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("Could not finalize the staged restore");
    }
    if had_pending {
        let _ = tokio::fs::remove_file(displaced).await;
    }
    Ok(pending)
}

/// Apply a staged restore before any SQLite connection is opened. The previous
/// database and its WAL/SHM sidecars are retained under a unique recovery name.
pub async fn apply_pending_restore(database_path: &Path) -> anyhow::Result<bool> {
    let pending = pending_restore_path(database_path);
    if !tokio::fs::try_exists(&pending)
        .await
        .context("Could not inspect the pending database restore")?
    {
        return Ok(false);
    }
    validate_sqlite_file(&pending).context("Pending database restore is invalid")?;

    let recovery = with_suffix(
        database_path,
        &format!(
            ".pre-restore-{}-{}",
            Utc::now().format("%Y%m%d_%H%M%S"),
            &Uuid::new_v4().simple().to_string()[..8]
        ),
    );
    let candidates = [
        (database_path.to_path_buf(), recovery.clone()),
        (
            with_suffix(database_path, "-wal"),
            with_suffix(&recovery, "-wal"),
        ),
        (
            with_suffix(database_path, "-shm"),
            with_suffix(&recovery, "-shm"),
        ),
    ];
    let mut moved = Vec::new();
    for (source, destination) in candidates {
        if tokio::fs::try_exists(&source)
            .await
            .with_context(|| format!("Could not inspect {}", source.display()))?
        {
            if let Err(error) = tokio::fs::rename(&source, &destination).await {
                for (rollback_source, rollback_destination) in moved.into_iter().rev() {
                    let _ = tokio::fs::rename(rollback_destination, rollback_source).await;
                }
                return Err(error)
                    .with_context(|| format!("Could not preserve {}", source.display()));
            }
            moved.push((source, destination));
        }
    }

    if let Err(error) = tokio::fs::rename(&pending, database_path).await {
        for (source, destination) in moved.into_iter().rev() {
            let _ = tokio::fs::rename(destination, source).await;
        }
        return Err(error).context("Could not activate the staged database restore");
    }
    Ok(true)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_database(path: &Path, value: &str) {
        let mut conn = SqliteConnection::establish(&path.to_string_lossy()).unwrap();
        diesel::sql_query("CREATE TABLE marker (value TEXT NOT NULL)")
            .execute(&mut conn)
            .unwrap();
        diesel::sql_query("INSERT INTO marker (value) VALUES (?)")
            .bind::<Text, _>(value)
            .execute(&mut conn)
            .unwrap();
    }

    fn marker(path: &Path) -> String {
        #[derive(QueryableByName)]
        struct Marker {
            #[diesel(sql_type = Text)]
            value: String,
        }
        let mut conn = SqliteConnection::establish(&path.to_string_lossy()).unwrap();
        diesel::sql_query("SELECT value FROM marker")
            .get_result::<Marker>(&mut conn)
            .unwrap()
            .value
    }

    #[test]
    fn backup_filenames_are_strict_components() {
        assert!(is_valid_backup_filename(
            "backup_20260808_120000_123_deadbeef.db"
        ));
        assert!(!is_valid_backup_filename("../backup_20260808.db"));
        assert!(!is_valid_backup_filename("backup_20260808.db-wal"));
        assert!(!is_valid_backup_filename("arbitrary.db"));
    }

    #[tokio::test]
    async fn staged_restore_switches_only_at_startup() {
        let temp = TempDir::new().unwrap();
        let live = temp.path().join("live.db");
        let backup = temp.path().join("backup.db");
        create_database(&live, "live");
        create_database(&backup, "backup");

        stage_restore(&backup, &live).await.unwrap();
        assert_eq!(marker(&live), "live");
        tokio::fs::write(with_suffix(&live, "-wal"), b"old wal")
            .await
            .unwrap();
        tokio::fs::write(with_suffix(&live, "-shm"), b"old shm")
            .await
            .unwrap();
        assert!(apply_pending_restore(&live).await.unwrap());
        assert_eq!(marker(&live), "backup");
        assert!(!apply_pending_restore(&live).await.unwrap());
        let recovery_files: Vec<String> = temp
            .path()
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("pre-restore"))
            .collect();
        assert!(
            recovery_files
                .iter()
                .any(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
        );
        assert!(recovery_files.iter().any(|name| name.ends_with("-wal")));
        assert!(recovery_files.iter().any(|name| name.ends_with("-shm")));
    }

    #[tokio::test]
    async fn invalid_pending_restore_never_replaces_live_database() {
        let temp = TempDir::new().unwrap();
        let live = temp.path().join("live.db");
        create_database(&live, "live");
        tokio::fs::write(pending_restore_path(&live), b"not sqlite")
            .await
            .unwrap();

        assert!(apply_pending_restore(&live).await.is_err());
        assert_eq!(marker(&live), "live");
    }
}
