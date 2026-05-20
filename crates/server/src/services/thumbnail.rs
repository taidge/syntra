use std::path::Path;

use image::imageops::FilterType;

use contrix_core::error::AppError;

/// Service that generates thumbnails for uploaded image files.
///
/// Thumbnails are stored in a `thumbnails/` subdirectory under the storage base
/// path. Each thumbnail is a 200x200 JPEG named after the file's UUID.
pub struct ThumbnailService {
    base_path: String,
}

impl ThumbnailService {
    /// Create a new thumbnail service. Ensures the thumbnails directory exists.
    pub fn new(base_path: &str) -> Self {
        let thumb_dir = format!("{}/thumbnails", base_path);
        let _ = std::fs::create_dir_all(&thumb_dir);
        Self {
            base_path: base_path.to_string(),
        }
    }

    /// Generate a thumbnail for an image file.
    ///
    /// `file_path` is the relative storage path (e.g. `<owner>/<file>`).
    /// Returns the relative path to the generated thumbnail.
    pub fn generate_thumbnail(
        &self,
        file_path: &str,
        file_id: &str,
    ) -> Result<String, AppError> {
        let full_path = Path::new(&self.base_path).join(file_path);
        let img = image::open(&full_path)
            .map_err(|e| AppError::Storage(format!("Cannot open image: {e}")))?;
        let thumb = img.resize(200, 200, FilterType::Lanczos3);

        let thumb_rel = format!("thumbnails/{}.jpg", file_id);
        let thumb_path = Path::new(&self.base_path).join(&thumb_rel);
        thumb
            .save(&thumb_path)
            .map_err(|e| AppError::Storage(format!("Cannot save thumbnail: {e}")))?;

        Ok(thumb_rel)
    }
}
