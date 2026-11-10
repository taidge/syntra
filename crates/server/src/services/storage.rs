#[cfg(test)]
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use contrix_core::config::StorageConfig;
use contrix_core::error::AppError;
use uuid::Uuid;

use crate::middleware::request_observability::with_database_stage;
use crate::services::s3_storage::S3Storage;

/// Scanner state retains at most 64 open directory iterators, never an entire
/// bucket or directory listing. Each call consumes at most page_size entries.
pub enum ScanCursor {
    Local {
        stack: Vec<(String, tokio::fs::ReadDir)>,
    },
    S3 {
        continuation: Option<String>,
        finished: bool,
    },
}
impl ScanCursor {
    pub fn finished(&self) -> bool {
        match self {
            Self::Local { stack } => stack.is_empty(),
            Self::S3 { finished, .. } => *finished,
        }
    }
}
#[derive(Debug)]
pub struct ObjectEntry {
    pub path: String,
    pub modified_at: i64,
}

/// Only application-owned namespaces are eligible for automatic discovery.
/// A shared bucket's unrelated files are never candidates for reconciliation.
pub fn managed_blob(path: &str) -> bool {
    if path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || StorageService::checked_relative_path(path).is_err()
    {
        return false;
    }
    let mut parts = path.split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    if Uuid::parse_str(first).is_ok() {
        return parts.next().is_some();
    }
    match first {
        "uploads" | "chunks" => {
            parts.next().is_some_and(|id| Uuid::parse_str(id).is_ok()) && parts.next().is_some()
        }
        "conversions" | "thumbnails" => parts.next().is_some_and(|name| {
            if name
                .rsplit_once('.')
                .is_some_and(|(stem, _)| Uuid::parse_str(stem).is_ok())
            {
                return parts.next().is_none();
            }
            Uuid::parse_str(name).is_ok()
                && parts.next().is_some_and(|file| {
                    file.rsplit_once('.')
                        .is_some_and(|(stem, _)| Uuid::parse_str(stem).is_ok())
                })
                && parts.next().is_none()
        }),
        _ => false,
    }
}
fn symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// Configured storage backend for durable file blobs.
#[derive(Clone)]
pub enum StorageBackend {
    Local(StorageService),
    S3(S3Storage),
}

impl StorageBackend {
    pub async fn begin_scan(&self) -> Result<ScanCursor, AppError> {
        match self {
            Self::Local(storage) => {
                storage.verified_path("").await?;
                let directory = tokio::fs::read_dir(&storage.base_path)
                    .await
                    .map_err(|e| AppError::Storage(e.to_string()))?;
                Ok(ScanCursor::Local {
                    stack: vec![(String::new(), directory)],
                })
            }
            Self::S3(_) => Ok(ScanCursor::S3 {
                continuation: None,
                finished: false,
            }),
        }
    }
    pub async fn scan_page(
        &self,
        cursor: &mut ScanCursor,
        limit: usize,
    ) -> Result<Vec<ObjectEntry>, AppError> {
        let limit = limit.clamp(1, 1000);
        match (self, cursor) {
            (Self::Local(storage), ScanCursor::Local { stack }) => {
                let mut entries = Vec::new();
                for _ in 0..limit {
                    let Some((parent, directory)) = stack.last_mut() else {
                        break;
                    };
                    let Some(entry) = directory
                        .next_entry()
                        .await
                        .map_err(|e| AppError::Storage(e.to_string()))?
                    else {
                        stack.pop();
                        continue;
                    };
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        continue;
                    };
                    let path = if parent.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{parent}/{name}")
                    };
                    let metadata = match tokio::fs::symlink_metadata(entry.path()).await {
                        Ok(metadata) => metadata,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(AppError::Storage(e.to_string())),
                    };
                    if symlink_or_reparse(&metadata) {
                        continue;
                    }
                    if metadata.is_dir() {
                        if stack.len() >= 64 {
                            continue;
                        }
                        // Ignore foreign top-level namespaces without traversing.
                        if !path.contains('/')
                            && Uuid::parse_str(&path).is_err()
                            && !matches!(
                                path.as_str(),
                                "uploads" | "chunks" | "conversions" | "thumbnails"
                            )
                        {
                            continue;
                        }
                        let verified = storage.verified_path(&path).await?;
                        match tokio::fs::read_dir(verified).await {
                            Ok(directory) => stack.push((path, directory)),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => return Err(AppError::Storage(e.to_string())),
                        }
                    } else if metadata.is_file() && managed_blob(&path) {
                        let modified = metadata
                            .modified()
                            .map_err(|e| AppError::Storage(e.to_string()))?;
                        entries.push(ObjectEntry {
                            path,
                            modified_at: chrono::DateTime::<chrono::Utc>::from(modified)
                                .timestamp(),
                        });
                    }
                }
                Ok(entries)
            }
            (
                Self::S3(storage),
                ScanCursor::S3 {
                    continuation,
                    finished,
                },
            ) => {
                if *finished {
                    return Ok(Vec::new());
                }
                let (entries, next) = storage
                    .list_objects_page(continuation.as_deref(), limit)
                    .await?;
                if next.is_some() && next == *continuation {
                    return Err(AppError::Storage(
                        "S3 listing repeated its continuation token".into(),
                    ));
                }
                *finished = next.is_none();
                *continuation = next;
                Ok(entries)
            }
            _ => Err(AppError::Internal(
                "Storage scan cursor/backend mismatch".into(),
            )),
        }
    }

    /// Attach once before production writes, so every object has a durable lease.
    pub fn attach_database(&self, db: crate::state::DbPool) -> Result<(), AppError> {
        let registry = match self {
            Self::Local(storage) => &storage.registry,
            Self::S3(storage) => &storage.registry,
        };
        if registry.get().is_none() {
            let _ = registry.set(db);
        }
        Ok(())
    }

    fn registry(&self) -> Option<&crate::state::DbPool> {
        match self {
            Self::Local(storage) => storage.registry.get(),
            Self::S3(storage) => storage.registry.get(),
        }
    }
    pub fn from_config(config: &StorageConfig) -> Result<Self, AppError> {
        match config
            .backend
            .as_deref()
            .unwrap_or("local")
            .to_ascii_lowercase()
            .as_str()
        {
            "local" => Ok(Self::Local(StorageService::new(&config.base_path))),
            "s3" => S3Storage::from_config(config)
                .map(Self::S3)
                .ok_or_else(|| AppError::Storage("Invalid S3 storage configuration".into())),
            backend => Err(AppError::Storage(format!(
                "Unsupported storage backend '{backend}'"
            ))),
        }
    }

    /// Readiness checks perform no writes and preserve backend failures.
    pub async fn check_ready(&self) -> Result<(), AppError> {
        match self {
            Self::Local(storage) => {
                let metadata = tokio::fs::metadata(&storage.base_path)
                    .await
                    .map_err(|error| {
                        AppError::Storage(format!("Storage directory unavailable: {error}"))
                    })?;
                if !metadata.is_dir() {
                    return Err(AppError::Storage("Storage root is not a directory".into()));
                }
                let _directory =
                    tokio::fs::read_dir(&storage.base_path)
                        .await
                        .map_err(|error| {
                            AppError::Storage(format!("Storage directory inaccessible: {error}"))
                        })?;
                Ok(())
            }
            Self::S3(storage) => storage.head_bucket().await,
        }
    }

    pub fn sanitize_file_name(file_name: &str) -> String {
        StorageService::sanitize_file_name(file_name)
    }

    pub async fn save_file(
        &self,
        owner_id: &Uuid,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, AppError> {
        // Blob keys never depend on an existence check: separate uploads and
        // document revisions must never share a mutable object.
        let file_name = StorageService::sanitize_file_name(file_name);
        let rel_path = format!("{owner_id}/{}/{file_name}", Uuid::new_v4());
        self.write_file(&rel_path, data, "application/octet-stream")
            .await?;
        Ok(rel_path)
    }

    pub async fn write_file(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), AppError> {
        let path = StorageService::checked_relative_path(path)?
            .to_string_lossy()
            .replace('\\', "/");
        let lease = match self.registry() {
            Some(db) => Some(
                with_database_stage(
                    "blob_write_reservation",
                    super::maintenance::begin_write(db, &path),
                )
                .await?,
            ),
            None => None,
        };
        let result = match self {
            Self::Local(storage) => {
                let full_path = storage.verified_path(&path).await?;
                if let Some(parent) = full_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|error| AppError::Storage(error.to_string()))?;
                }
                tokio::fs::write(full_path, data)
                    .await
                    .map_err(|error| AppError::Storage(error.to_string()))
            }
            Self::S3(storage) => storage.put_object(&path, data, content_type).await,
        };
        result?;
        if let Some(lease) = lease {
            with_database_stage("blob_write_finish", lease.finish()).await?;
        }
        Ok(())
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Local(storage) => tokio::fs::read(storage.verified_path(path).await?)
                .await
                .map_err(|error| AppError::Storage(error.to_string())),
            Self::S3(storage) => {
                let path = StorageService::checked_relative_path(path)?
                    .to_string_lossy()
                    .replace('\\', "/");
                storage.get_object(&path).await
            }
        }
    }

    pub async fn delete_file(&self, path: &str) -> Result<(), AppError> {
        match self {
            Self::Local(storage) => {
                match tokio::fs::remove_file(storage.verified_path(path).await?).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(AppError::Storage(error.to_string())),
                }
            }
            Self::S3(storage) => {
                let path = StorageService::checked_relative_path(path)?
                    .to_string_lossy()
                    .replace('\\', "/");
                storage.delete_object(&path).await
            }
        }
    }

    #[cfg(test)]
    async fn exists(&self, path: &str) -> Result<bool, AppError> {
        match self {
            Self::Local(storage) => tokio::fs::try_exists(storage.resolve_path(path)?)
                .await
                .map_err(|error| AppError::Storage(error.to_string())),
            Self::S3(storage) => {
                let path = StorageService::checked_relative_path(path)?
                    .to_string_lossy()
                    .replace('\\', "/");
                storage.head_object(&path).await
            }
        }
    }
}

/// Service for reading, writing, and deleting files on the local filesystem.
#[derive(Clone)]
pub struct StorageService {
    base_path: PathBuf,
    registry: OnceLock<crate::state::DbPool>,
}

impl StorageService {
    /// Create a new storage service rooted at `base_path`.
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
            registry: OnceLock::new(),
        }
    }

    /// Return a filesystem-safe file name for user-visible upload names.
    pub fn sanitize_file_name(file_name: &str) -> String {
        let candidate = Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file_name)
            .trim();

        let mut sanitized: String = candidate
            .chars()
            .map(|ch| match ch {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                ch if ch.is_control() => '_',
                ch => ch,
            })
            .collect();

        sanitized = sanitized
            .trim_matches(|ch| ch == '.' || ch == ' ')
            .to_string();

        if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
            sanitized = "upload".to_string();
        }

        if is_reserved_windows_name(&sanitized) {
            sanitized = format!("_{sanitized}");
        }

        const MAX_FILE_NAME_CHARS: usize = 255;
        if sanitized.chars().count() > MAX_FILE_NAME_CHARS {
            sanitized = sanitized.chars().take(MAX_FILE_NAME_CHARS).collect();
            sanitized = sanitized
                .trim_matches(|ch| ch == '.' || ch == ' ')
                .to_string();
            if sanitized.is_empty() {
                sanitized = "upload".to_string();
            }
        }

        sanitized
    }

    pub fn checked_relative_path(path: &str) -> Result<PathBuf, AppError> {
        let relative = Path::new(path);
        if relative.as_os_str().is_empty() || relative.is_absolute() {
            return Err(AppError::Storage("Invalid storage path".into()));
        }

        let mut normalized = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                _ => return Err(AppError::Storage("Invalid storage path".into())),
            }
        }

        if normalized.as_os_str().is_empty() {
            Err(AppError::Storage("Invalid storage path".into()))
        } else {
            Ok(normalized)
        }
    }

    /// Reject symlinks and Windows junction/reparse components before I/O.
    /// Upload APIs never create directories as links; reconciliation also never
    /// descends links encountered in the configured, dedicated storage root.
    async fn verified_path(&self, path: &str) -> Result<PathBuf, AppError> {
        let mut current = self.base_path.clone();
        let mut paths = vec![current.clone()];
        if !path.is_empty() {
            for component in Self::checked_relative_path(path)?.components() {
                current.push(component);
                paths.push(current.clone());
            }
        }
        for component in paths {
            match tokio::fs::symlink_metadata(component).await {
                Ok(metadata) if symlink_or_reparse(&metadata) => {
                    return Err(AppError::Storage(
                        "Storage symlinks and reparse points are not permitted".into(),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(AppError::Storage(e.to_string())),
            }
        }
        Ok(current)
    }

    #[cfg(test)]
    fn resolve_path(&self, path: &str) -> Result<PathBuf, AppError> {
        Ok(self.base_path.join(Self::checked_relative_path(path)?))
    }

    /// Persist `data` to disk under `<base>/<owner_id>/<file_name>` and return
    /// the relative storage path.
    #[cfg(test)]
    pub fn save_file(
        &self,
        owner_id: &Uuid,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, AppError> {
        self.ensure_user_dir(owner_id)?;
        let file_name = Self::sanitize_file_name(file_name);
        let rel_path = format!("{}/{}", owner_id, file_name);
        let full_path = self.base_path.join(&rel_path);

        // Avoid overwriting an existing file with the same name by appending a
        // short unique suffix when necessary.
        let full_path = if full_path.exists() {
            let stem = Path::new(&file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&file_name);
            let ext = Path::new(&file_name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let suffix = &Uuid::new_v4().to_string()[..8];
            let new_name = if ext.is_empty() {
                format!("{stem}_{suffix}")
            } else {
                format!("{stem}_{suffix}.{ext}")
            };
            let new_rel = format!("{}/{}", owner_id, new_name);
            let p = self.base_path.join(&new_rel);
            fs::write(&p, data).map_err(|e| AppError::Storage(e.to_string()))?;
            return Ok(new_rel);
        } else {
            full_path
        };

        fs::write(&full_path, data).map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(rel_path)
    }

    /// Read the contents of the file at the given relative `path`.
    #[cfg(test)]
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, AppError> {
        let full_path = self.resolve_path(path)?;
        fs::read(&full_path).map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Delete the file at the given relative `path`.
    #[cfg(test)]
    pub fn delete_file(&self, path: &str) -> Result<(), AppError> {
        let full_path = self.resolve_path(path)?;
        if full_path.exists() {
            fs::remove_file(&full_path).map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    /// Ensure the per-user directory exists.
    #[cfg(test)]
    pub fn ensure_user_dir(&self, owner_id: &Uuid) -> Result<(), AppError> {
        let dir = self.base_path.join(owner_id.to_string());
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }
}

fn is_reserved_windows_name(file_name: &str) -> bool {
    let stem = file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn async_local_files_preserve_collisions_and_path_validation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let storage = StorageBackend::Local(StorageService::new(temp_dir.path().to_str().unwrap()));
        let owner = Uuid::new_v4();
        let first = storage
            .save_file(&owner, "../secret\r\n.txt", b"first")
            .await
            .unwrap();
        let second = storage
            .save_file(&owner, "../secret\r\n.txt", b"second")
            .await
            .unwrap();
        assert!(first.starts_with(&format!("{owner}/")));
        assert!(first.ends_with("/secret__.txt"));
        assert_ne!(first, second);
        assert_eq!(storage.read_file(&first).await.unwrap(), b"first");
        assert_eq!(storage.read_file(&second).await.unwrap(), b"second");

        storage
            .write_file("nested/output.bin", b"output", "application/octet-stream")
            .await
            .unwrap();
        assert_eq!(
            storage.read_file("nested/output.bin").await.unwrap(),
            b"output"
        );
        assert!(storage.exists("nested/output.bin").await.unwrap());
        assert!(
            storage
                .write_file("../outside", b"bad", "text/plain")
                .await
                .is_err()
        );
        assert!(storage.read_file("../outside").await.is_err());
        assert!(storage.delete_file("../outside").await.is_err());
        assert!(storage.exists("../outside").await.is_err());
        storage.delete_file("nested/output.bin").await.unwrap();
        storage.delete_file("nested/output.bin").await.unwrap();
        assert!(!storage.exists("nested/output.bin").await.unwrap());
    }

    #[test]
    fn save_file_sanitizes_upload_name() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let storage = StorageService::new(temp_dir.path().to_str().unwrap());
        let owner = Uuid::new_v4();

        let path = storage
            .save_file(&owner, "../secret\r\n.txt", b"payload")
            .unwrap();

        assert_eq!(path, format!("{owner}/secret__.txt"));
        assert_eq!(storage.read_file(&path).unwrap(), b"payload");
        assert!(!temp_dir.path().join("secret__.txt").exists());
    }

    #[test]
    fn read_and_delete_reject_parent_traversal() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let storage = StorageService::new(temp_dir.path().to_str().unwrap());

        assert!(matches!(
            storage.read_file("../outside.txt"),
            Err(AppError::Storage(_))
        ));
        assert!(matches!(
            storage.delete_file("nested/../../outside.txt"),
            Err(AppError::Storage(_))
        ));
    }

    #[test]
    fn backend_from_config_initializes_local_storage() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = StorageConfig {
            upload_idle_timeout_seconds: 30,
            upload_total_timeout_seconds: 900,
            base_path: temp_dir.path().to_string_lossy().into_owned(),
            max_upload_size_mb: 100,
            default_quota_mb: 1024,
            backend: Some("local".to_string()),
            s3_bucket: None,
            s3_region: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_endpoint: None,
        };

        assert!(matches!(
            StorageBackend::from_config(&config).unwrap(),
            StorageBackend::Local(_)
        ));
    }

    #[test]
    fn backend_from_config_rejects_incomplete_s3_config() {
        let config = StorageConfig {
            upload_idle_timeout_seconds: 30,
            upload_total_timeout_seconds: 900,
            base_path: "data/files".to_string(),
            max_upload_size_mb: 100,
            default_quota_mb: 1024,
            backend: Some("s3".to_string()),
            s3_bucket: Some("bucket".to_string()),
            s3_region: Some("us-east-1".to_string()),
            s3_access_key: None,
            s3_secret_key: None,
            s3_endpoint: None,
        };

        assert!(StorageBackend::from_config(&config).is_err());
    }
    #[test]
    fn reconciliation_never_normalizes_a_foreign_key_into_a_managed_object() {
        let owner = Uuid::new_v4();
        let object = Uuid::new_v4();
        assert!(managed_blob(&format!("{owner}/{object}/Hello 世界.txt")));
        assert!(managed_blob(&format!("conversions/{object}/{owner}.html")));
        for path in [
            format!("{owner}/./victim"),
            format!("{owner}//victim"),
            format!("{owner}/../victim"),
            format!("{owner}/victim:stream"),
            format!("{owner}\\victim"),
            "foreign/keep".into(),
        ] {
            assert!(!managed_blob(&path), "unsafe managed key: {path}");
        }
    }
}
