#[cfg(test)]
use std::fs;
use std::path::{Component, Path, PathBuf};

use contrix_core::config::StorageConfig;
use contrix_core::error::AppError;
use uuid::Uuid;

use crate::services::s3_storage::S3Storage;

/// Configured storage backend for durable file blobs.
pub enum StorageBackend {
    Local(StorageService),
    S3(S3Storage),
}

impl StorageBackend {
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

    pub fn sanitize_file_name(file_name: &str) -> String {
        StorageService::sanitize_file_name(file_name)
    }

    pub async fn save_file(
        &self,
        owner_id: &Uuid,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, AppError> {
        match self {
            Self::Local(_) => {
                let file_name = StorageService::sanitize_file_name(file_name);
                let rel_path =
                    unique_relative_path(self, &format!("{owner_id}/{file_name}")).await?;
                self.write_file(&rel_path, data, "application/octet-stream")
                    .await?;
                Ok(rel_path)
            }
            Self::S3(storage) => {
                let file_name = StorageService::sanitize_file_name(file_name);
                let rel_path =
                    unique_relative_path(self, &format!("{owner_id}/{file_name}")).await?;
                storage
                    .put_object(&rel_path, data, "application/octet-stream")
                    .await?;
                Ok(rel_path)
            }
        }
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
        match self {
            Self::Local(storage) => {
                let full_path = storage.resolve_path(&path)?;
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
        }
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, AppError> {
        match self {
            Self::Local(storage) => tokio::fs::read(storage.resolve_path(path)?)
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
            Self::Local(storage) => match tokio::fs::remove_file(storage.resolve_path(path)?).await
            {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(AppError::Storage(error.to_string())),
            },
            Self::S3(storage) => {
                let path = StorageService::checked_relative_path(path)?
                    .to_string_lossy()
                    .replace('\\', "/");
                storage.delete_object(&path).await
            }
        }
    }

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
pub struct StorageService {
    base_path: PathBuf,
}

impl StorageService {
    /// Create a new storage service rooted at `base_path`.
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
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

async fn unique_relative_path(
    storage: &StorageBackend,
    preferred_path: &str,
) -> Result<String, AppError> {
    let preferred_path = StorageService::checked_relative_path(preferred_path)?
        .to_string_lossy()
        .replace('\\', "/");
    if !storage.exists(&preferred_path).await? {
        return Ok(preferred_path);
    }

    let path = Path::new(&preferred_path);
    let parent = path
        .parent()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Storage("Invalid storage path".into()))?;
    let stem = Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let ext = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let suffix = &Uuid::new_v4().to_string()[..8];
    let new_name = if ext.is_empty() {
        format!("{stem}_{suffix}")
    } else {
        format!("{stem}_{suffix}.{ext}")
    };

    if parent.is_empty() {
        Ok(new_name)
    } else {
        Ok(format!("{parent}/{new_name}"))
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
        assert_eq!(first, format!("{owner}/secret__.txt"));
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
}
