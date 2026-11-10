use std::io::Cursor;

use image::imageops::FilterType;

use contrix_core::error::AppError;

use crate::services::StorageBackend;

/// Service that generates thumbnails for uploaded image files.
pub struct ThumbnailService;

impl Default for ThumbnailService {
    fn default() -> Self {
        Self::new()
    }
}

impl ThumbnailService {
    pub fn new() -> Self {
        Self
    }

    /// Generate a thumbnail for an image file.
    ///
    /// `file_path` is the relative storage path (e.g. `<owner>/<file>`).
    /// Returns the relative path to the generated thumbnail.
    pub async fn generate_thumbnail(
        &self,
        storage: &StorageBackend,
        file_path: &str,
        file_id: &str,
    ) -> Result<String, AppError> {
        let source = storage.read_file(file_path).await?;
        let img = image::load_from_memory(&source)
            .map_err(|e| AppError::Storage(format!("Cannot open image: {e}")))?;
        let thumb = img.resize(200, 200, FilterType::Lanczos3);

        let thumb_rel = format!("thumbnails/{file_id}/{}.jpg", uuid::Uuid::new_v4());
        let mut output = Cursor::new(Vec::new());
        thumb
            .write_to(&mut output, image::ImageFormat::Jpeg)
            .map_err(|e| AppError::Storage(format!("Cannot encode thumbnail: {e}")))?;
        storage
            .write_file(&thumb_rel, &output.into_inner(), "image/jpeg")
            .await?;

        Ok(thumb_rel)
    }
}
