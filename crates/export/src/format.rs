//! Export format definitions and helpers.

use serde::{Deserialize, Serialize};

/// All output formats supported by the export engine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Html,
    Markdown,
    PlainText,
    Csv,
    Json,
    Docx,
    Xlsx,
    Pptx,
    Png,
    Svg,
}

impl ExportFormat {
    /// Return the IANA media type for this format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Html => "text/html",
            Self::Markdown => "text/markdown",
            Self::PlainText => "text/plain",
            Self::Csv => "text/csv",
            Self::Json => "application/json",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Pptx => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
            Self::Png => "image/png",
            Self::Svg => "image/svg+xml",
        }
    }

    /// Return the conventional file extension (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Markdown => "md",
            Self::PlainText => "txt",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Png => "png",
            Self::Svg => "svg",
        }
    }

    /// Attempt to determine the export format from a file extension string.
    ///
    /// The lookup is case-insensitive and accepts the extension with or
    /// without a leading dot.
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.strip_prefix('.').unwrap_or(ext);
        match ext.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "html" | "htm" => Some(Self::Html),
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" => Some(Self::PlainText),
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "docx" => Some(Self::Docx),
            "xlsx" => Some(Self::Xlsx),
            "pptx" => Some(Self::Pptx),
            "png" => Some(Self::Png),
            "svg" => Some(Self::Svg),
            _ => None,
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        let base = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
        match base.as_str() {
            "application/pdf" => Some(Self::Pdf),
            "text/html" => Some(Self::Html),
            "text/markdown" => Some(Self::Markdown),
            "text/plain" => Some(Self::PlainText),
            "text/csv" => Some(Self::Csv),
            "application/json" => Some(Self::Json),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some(Self::Docx)
            }
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
                Some(Self::Xlsx)
            }
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Some(Self::Pptx)
            }
            "image/png" => Some(Self::Png),
            "image/svg+xml" => Some(Self::Svg),
            _ => None,
        }
    }
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_extension() {
        for format in [
            ExportFormat::Pdf,
            ExportFormat::Html,
            ExportFormat::Markdown,
            ExportFormat::PlainText,
            ExportFormat::Csv,
            ExportFormat::Json,
            ExportFormat::Docx,
            ExportFormat::Xlsx,
            ExportFormat::Pptx,
            ExportFormat::Png,
            ExportFormat::Svg,
        ] {
            let ext = format.extension();
            let parsed = ExportFormat::from_extension(ext)
                .unwrap_or_else(|| panic!("from_extension({ext:?}) returned None"));
            assert_eq!(parsed, format, "round-trip failed for {ext}");
        }
    }

    #[test]
    fn from_extension_case_insensitive() {
        assert_eq!(ExportFormat::from_extension("PDF"), Some(ExportFormat::Pdf));
        assert_eq!(
            ExportFormat::from_extension(".Html"),
            Some(ExportFormat::Html)
        );
    }

    #[test]
    fn from_extension_unknown() {
        assert_eq!(ExportFormat::from_extension("xyz"), None);
    }

    #[test]
    fn mime_types_are_nonempty() {
        assert!(!ExportFormat::Pdf.mime_type().is_empty());
        assert!(ExportFormat::Docx.mime_type().contains("officedocument"));
    }

    #[test]
    fn from_mime_round_trip() {
        for f in [
            ExportFormat::Pdf,
            ExportFormat::Html,
            ExportFormat::Markdown,
            ExportFormat::PlainText,
            ExportFormat::Csv,
            ExportFormat::Json,
            ExportFormat::Docx,
            ExportFormat::Xlsx,
            ExportFormat::Pptx,
            ExportFormat::Png,
            ExportFormat::Svg,
        ] {
            assert_eq!(ExportFormat::from_mime(f.mime_type()), Some(f));
        }
    }

    #[test]
    fn from_mime_strips_parameters() {
        assert_eq!(
            ExportFormat::from_mime("text/html; charset=utf-8"),
            Some(ExportFormat::Html)
        );
    }

    #[test]
    fn from_mime_unknown() {
        assert_eq!(ExportFormat::from_mime("application/foo"), None);
    }
}
