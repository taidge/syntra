use std::fs;
use std::path::{Path, PathBuf};

use contrix_core::error::AppError;
use uuid::Uuid;

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

    /// Persist `data` to disk under `<base>/<owner_id>/<file_name>` and return
    /// the relative storage path.
    pub fn save_file(
        &self,
        owner_id: &Uuid,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, AppError> {
        self.ensure_user_dir(owner_id)?;
        let rel_path = format!("{}/{}", owner_id, file_name);
        let full_path = self.base_path.join(&rel_path);

        // Avoid overwriting an existing file with the same name by appending a
        // short unique suffix when necessary.
        let full_path = if full_path.exists() {
            let stem = Path::new(file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(file_name);
            let ext = Path::new(file_name)
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
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, AppError> {
        let full_path = self.base_path.join(path);
        fs::read(&full_path).map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Delete the file at the given relative `path`.
    pub fn delete_file(&self, path: &str) -> Result<(), AppError> {
        let full_path = self.base_path.join(path);
        if full_path.exists() {
            fs::remove_file(&full_path).map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    /// Ensure the per-user directory exists.
    pub fn ensure_user_dir(&self, owner_id: &Uuid) -> Result<(), AppError> {
        let dir = self.base_path.join(owner_id.to_string());
        if !dir.exists() {
            fs::create_dir_all(&dir).map_err(|e| AppError::Storage(e.to_string()))?;
        }
        Ok(())
    }
}
