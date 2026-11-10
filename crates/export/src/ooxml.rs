//! Shared resource budget for untrusted Office packages, including fallback parsers.
use std::io::{Cursor, Read};
use zip::ZipArchive;

pub const MAX_PARTS: usize = 4096;
pub const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;

/// Enforce the actual inflated byte count independently of ZIP metadata.
pub struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}
pub fn bounded<R: Read>(reader: R) -> BoundedReader<R> {
    bounded_with_limit(reader, MAX_PART_BYTES)
}
pub fn bounded_with_limit<R: Read>(reader: R, limit: u64) -> BoundedReader<R> {
    BoundedReader {
        inner: reader,
        remaining: limit,
    }
}
impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Office part exceeds actual decompression budget",
                ));
            }
            return Ok(0);
        }
        let available = buffer
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let read = self.inner.read(&mut buffer[..available])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

pub fn validate(bytes: &[u8]) -> Result<(), String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    if archive.len() > MAX_PARTS {
        return Err("Office package has too many parts".into());
    }
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name().map_err(|error| error.to_string())?.into_owned();
        if !names.insert(name) {
            return Err("Office package has duplicate part names".into());
        }
        if file.enclosed_name().is_none() {
            return Err("Office package contains an invalid part path".into());
        }
        total = total
            .checked_add(file.size())
            .ok_or("Office package size overflow")?;
        if file.size() > MAX_PART_BYTES || total > MAX_TOTAL_BYTES {
            return Err("Office package exceeds decompression budget".into());
        }
        let expected = file.size();
        let actual = std::io::copy(&mut file.take(expected + 1), &mut std::io::sink())
            .map_err(|error| error.to_string())?;
        if actual != expected {
            return Err("Office part size does not match its contents".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};
    #[test]
    fn rejects_compressed_bomb_before_fallback_parser_allocates() {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "word/document.xml",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        let block = [b' '; 1024 * 1024];
        for _ in 0..65 {
            archive.write_all(&block).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        assert!(bytes.len() < 1024 * 1024);
        assert!(validate(&bytes).unwrap_err().contains("budget"));
        assert!(
            crate::docx::docx_to_writer(&bytes)
                .unwrap_err()
                .contains("budget")
        );
        assert!(
            crate::pptx::pptx_to_presentation(&bytes)
                .unwrap_err()
                .contains("budget")
        );
        assert!(
            crate::xlsx::xlsx_to_workbook(&bytes)
                .unwrap_err()
                .contains("budget")
        );
    }
    #[test]
    fn forged_small_uncompressed_size_cannot_bypass_actual_read_budget() {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "word/document.xml",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        archive.write_all(&vec![b'a'; 2 * 1024 * 1024]).unwrap();
        let mut bytes = archive.finish().unwrap().into_inner();
        let local = bytes.windows(4).position(|w| w == b"PK\x03\x04").unwrap();
        let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        bytes[local + 22..local + 26].copy_from_slice(&1u32.to_le_bytes());
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        assert!(
            validate(&bytes).is_err(),
            "actual inflate count must match declared size"
        );
        let mut bounded = bounded_with_limit(Cursor::new(vec![b'a'; 1025]), 1024);
        let mut data = Vec::new();
        assert!(bounded.read_to_end(&mut data).is_err());
        assert!(
            data.len() <= 1024,
            "only the bounded allocation is retained"
        );
    }
}
