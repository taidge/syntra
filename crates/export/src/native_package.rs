//! Native model caches are used only while every OOXML part is unchanged.
//! An Office application may retain custom parts after editing the visible
//! document; trusting that stale cache would otherwise discard those edits.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use sha2::{Digest, Sha256};
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn seal(bytes: Vec<u8>, native_path: &str) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut parts = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
        if name != native_path {
            hashes.insert(name.clone(), digest(&data));
        }
        parts.insert(name, data);
    }
    let native = parts
        .get(native_path)
        .ok_or_else(|| "Missing native document part".to_string())?;
    let model: serde_json::Value =
        serde_json::from_slice(native).map_err(|error| error.to_string())?;
    parts.insert(
        native_path.into(),
        serde_json::to_vec(&serde_json::json!({"model":model,"parts":hashes}))
            .map_err(|error| error.to_string())?,
    );
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut output);
        for (name, data) in parts {
            writer
                .start_file(name, SimpleFileOptions::default())
                .map_err(|error| error.to_string())?;
            writer.write_all(&data).map_err(|error| error.to_string())?;
        }
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(output.into_inner())
}

pub fn read(bytes: &[u8], native_path: &str) -> Option<serde_json::Value> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let wrapper: serde_json::Value = {
        let mut file = archive.by_name(native_path).ok()?;
        if file.size() > 20 * 1024 * 1024 {
            return None;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).ok()?;
        serde_json::from_slice(&data).ok()?
    };
    let hashes = wrapper.get("parts")?.as_object()?;
    if hashes.len() + 1 != archive.len() {
        return None;
    }
    let mut total = 0u64;
    for (name, expected) in hashes {
        let mut file = archive.by_name(name).ok()?;
        total = total.checked_add(file.size())?;
        if total > 200 * 1024 * 1024 {
            return None;
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).ok()?;
        if expected.as_str()? != digest(&data) {
            return None;
        }
    }
    wrapper.get("model").cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externally_edited_ooxml_does_not_use_stale_native_model() {
        let path = "customXml/model.json";
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("document.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"before").unwrap();
        zip.start_file(path, SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"{"title":"before"}"#).unwrap();
        let sealed = seal(zip.finish().unwrap().into_inner(), path).unwrap();
        assert_eq!(read(&sealed, path).unwrap()["title"], "before");
        let mut source = ZipArchive::new(Cursor::new(&sealed)).unwrap();
        let mut edited = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..source.len() {
            let mut file = source.by_index(index).unwrap();
            let name = file.name().to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            edited
                .start_file(&name, SimpleFileOptions::default())
                .unwrap();
            edited
                .write_all(if name == "document.xml" {
                    b"after"
                } else {
                    &data
                })
                .unwrap();
        }
        assert!(read(&edited.finish().unwrap().into_inner(), path).is_none());
    }
}
