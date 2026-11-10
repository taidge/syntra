//! Bounded, namespace-prefix-independent helpers for standard OOXML imports.
use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;
pub type Archive<'a> = ZipArchive<Cursor<&'a [u8]>>;
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<Node>,
    pub text: String,
}
impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str).or_else(|| {
            let local = name.rsplit(':').next()?;
            self.attrs
                .iter()
                .find(|(key, _)| {
                    (!name.contains(':') || key.contains(':'))
                        && key.rsplit(':').next() == Some(local)
                })
                .map(|(_, value)| value.as_str())
        })
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }
    pub fn descendants(&self, name: &str) -> Vec<&Node> {
        let mut found = Vec::new();
        let mut queue: Vec<_> = self.children.iter().rev().collect();
        while let Some(node) = queue.pop() {
            if node.name == name {
                found.push(node);
            }
            queue.extend(node.children.iter().rev());
        }
        found
    }
    pub fn desc(&self, name: &str) -> Option<&Node> {
        self.descendants(name).into_iter().next()
    }
    pub fn all_text(&self) -> String {
        let mut text = String::new();
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            text.push_str(&node.text);
            stack.extend(node.children.iter().rev());
        }
        text
    }
    pub fn num(&self, name: &str) -> Option<f64> {
        self.attr(name)?
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
    }
    pub fn flag(&self, name: &str) -> Option<bool> {
        self.attr(name)
            .map(|value| matches!(value, "1" | "true" | "on"))
    }
}
pub fn parse(xml: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = vec![Node::default()];
    let mut nodes = 0usize;
    loop {
        let event = reader.read_event();
        let empty = matches!(&event, Ok(Event::Empty(_)));
        match event {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                nodes += 1;
                if nodes > 200_000 || stack.len() > 128 {
                    return Err("OOXML XML node/depth budget exceeded".into());
                }
                let mut node = Node {
                    name: std::str::from_utf8(element.local_name().as_ref())
                        .map_err(|e| e.to_string())?
                        .into(),
                    ..Default::default()
                };
                for attr in element.attributes() {
                    let attr = attr.map_err(|e| e.to_string())?;
                    let key = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|e| e.to_string())?
                        .to_owned();
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    node.attrs.insert(key, value);
                }
                if empty {
                    stack
                        .last_mut()
                        .ok_or("Invalid XML root")?
                        .children
                        .push(node);
                } else {
                    stack.push(node);
                }
            }
            Ok(Event::End(_)) => {
                if stack.len() < 2 {
                    return Err("Unbalanced OOXML XML".into());
                }
                let node = stack.pop().ok_or("Invalid XML stack")?;
                stack
                    .last_mut()
                    .ok_or("Invalid XML root")?
                    .children
                    .push(node);
            }
            Ok(Event::Text(text)) => stack
                .last_mut()
                .ok_or("Invalid XML text")?
                .text
                .push_str(&text.decode().map_err(|e| e.to_string())?),
            Ok(Event::CData(text)) => stack
                .last_mut()
                .ok_or("Invalid XML CDATA")?
                .text
                .push_str(&text.decode().map_err(|e| e.to_string())?),
            Ok(Event::GeneralRef(reference)) => {
                let encoded = format!("&{};", reference.decode().map_err(|e| e.to_string())?);
                let text = quick_xml::escape::unescape(&encoded).map_err(|e| e.to_string())?;
                stack
                    .last_mut()
                    .ok_or("Invalid XML reference")?
                    .text
                    .push_str(&text);
            }
            Ok(Event::DocType(_)) => return Err("OOXML DTD declarations are unsupported".into()),
            Ok(Event::Eof) => break,
            Err(error) => return Err(error.to_string()),
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err("Unclosed OOXML XML element".into());
    }
    let mut document = stack.pop().ok_or("Missing XML root")?;
    if document.children.len() != 1 {
        return Err("Expected one OOXML root".into());
    }
    Ok(document.children.remove(0))
}
pub fn bytes(archive: &mut Archive<'_>, path: &str) -> Result<Vec<u8>, String> {
    let file = archive.by_name(path).map_err(|e| format!("{path}: {e}"))?;
    let mut bytes = Vec::new();
    crate::ooxml::bounded(file)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
pub fn bytes_limit(archive: &mut Archive<'_>, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let file = archive.by_name(path).map_err(|e| e.to_string())?;
    if file.size() > limit as u64 {
        return Err("Embedded media exceeds import budget".into());
    }
    let mut bytes = Vec::new();
    crate::ooxml::bounded(file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Embedded media exceeds import budget".into());
    }
    Ok(bytes)
}
// DrawingML geometry is bounded before any f64 -> f32 conversion, including
// table dimensions, text margins, crop percentages, fonts, and gradients. The
// limit comfortably exceeds normal Office coordinates but cannot overflow f32.
fn validate_drawing_numbers(root: &Node) -> Result<(), String> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let attributes: &[&str] = match node.name.as_str() {
            "off" | "chOff" => &["x", "y"],
            "ext" | "chExt" | "sldSz" => &["cx", "cy"],
            "xfrm" => &["rot"],
            "ln" | "gridCol" => &["w"],
            "tr" => &["h"],
            "rPr" | "defRPr" | "endParaRPr" => &["sz"],
            "bodyPr" => &["lIns", "rIns", "tIns", "bIns"],
            "srcRect" => &["l", "t", "r", "b"],
            "lin" => &["ang"],
            _ => &[],
        };
        for attribute in attributes {
            if let Some(raw) = node.attr(attribute) {
                let value = raw.parse::<f64>().map_err(|_| {
                    format!(
                        "Invalid DrawingML numeric attribute {}.{attribute}",
                        node.name
                    )
                })?;
                if !value.is_finite() || value.abs() > 1_000_000_000_000.0 {
                    return Err(format!(
                        "DrawingML numeric attribute {}.{attribute} exceeds the finite geometry budget",
                        node.name
                    ));
                }
            }
        }
        stack.extend(&node.children);
    }
    Ok(())
}
pub fn part(archive: &mut Archive<'_>, path: &str) -> Result<Node, String> {
    let bytes = bytes(archive, path)?;
    let root = parse(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)?;
    validate_drawing_numbers(&root)?;
    Ok(root)
}
pub fn optional(archive: &mut Archive<'_>, path: &str) -> Result<Option<Node>, String> {
    if archive.by_name(path).is_err() {
        return Ok(None);
    }
    part(archive, path).map(Some)
}
#[derive(Clone, Debug)]
pub struct Relationship {
    pub kind: String,
    pub target: String,
    pub external: bool,
}
pub fn resolve(source: &str, target: &str) -> Result<String, String> {
    if target.contains(['\\', '\0', '?', '#']) {
        return Err("Unsupported OOXML part target".into());
    }
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map(|(base, _)| base.split('/').collect())
            .unwrap_or_default()
    };
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("OOXML relationship escapes package".into());
                }
            }
            value => parts.push(value),
        }
    }
    Ok(parts.join("/"))
}
pub fn relationships(
    archive: &mut Archive<'_>,
    source: &str,
) -> Result<BTreeMap<String, Relationship>, String> {
    let (base, file) = source.rsplit_once('/').unwrap_or(("", source));
    let path = if base.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{base}/_rels/{file}.rels")
    };
    let Some(root) = optional(archive, &path)? else {
        return Ok(BTreeMap::new());
    };
    let mut result = BTreeMap::new();
    for node in root
        .children
        .iter()
        .filter(|node| node.name == "Relationship")
    {
        let id = node.attr("Id").ok_or("Relationship has no Id")?;
        let target = node.attr("Target").ok_or("Relationship has no target")?;
        let external = node.attr("TargetMode") == Some("External");
        let target = if external {
            target.into()
        } else {
            resolve(source, target)?
        };
        if result
            .insert(
                id.into(),
                Relationship {
                    kind: node.attr("Type").unwrap_or_default().into(),
                    target,
                    external,
                },
            )
            .is_some()
        {
            return Err("Duplicate OOXML relationship Id".into());
        }
    }
    Ok(result)
}
pub fn warn(warnings: &mut Vec<String>, message: impl Into<String>) {
    let message = message.into();
    if !warnings.contains(&message) {
        warnings.push(message);
    }
}
pub fn safe_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")
}
pub fn text(node: &Node, name: &str) -> String {
    node.descendants(name)
        .into_iter()
        .map(Node::all_text)
        .collect::<Vec<_>>()
        .join("")
}

/// Count serialized bytes without allocating a second model-sized buffer.
pub fn model_budget(model: &impl serde::Serialize) -> Result<(), String> {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other(
                    "Imported model exceeds document content budget",
                ));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(
        Budget(contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES),
        model,
    )
    .map_err(|e| e.to_string())
}
