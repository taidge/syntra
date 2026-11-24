//! Bounded native PDF inspection and text-layer conversion to Writer.
//!
//! The original PDF remains the layout-preserving source. Conversion extracts
//! selectable text into real Writer paragraphs; it does not pretend to OCR an
//! image-only page or reproduce arbitrary fixed-layout drawing instructions.

use lopdf::{Document, LoadOptions, Object, ObjectId};
use std::cell::RefCell;
use std::collections::BTreeSet;
use syntra_doc_engine::writer::{Block, WriterDocument};

pub const MAX_PDF_SOURCE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PDF_PAGES: usize = 500;
const MAX_PDF_OBJECTS: usize = 50_000;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_OBJECT_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES;
const MAX_PAGE_TEXT_BYTES: usize = 1024 * 1024;
const MAX_FONT_MAP_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BLOCKS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfInfo {
    /// Encrypted PDFs cannot expose their page tree without a password.
    pub page_count: usize,
    pub encrypted: bool,
}

#[derive(Debug)]
pub struct PdfImport {
    pub document: WriterDocument,
    pub warnings: Vec<String>,
    pub page_count: usize,
}

#[derive(Default)]
struct LoadBudget {
    objects: usize,
    object_stream_bytes: usize,
    error: Option<String>,
}

thread_local! {
    // lopdf is built without Rayon: its synchronous object filter runs on the
    // calling blocking worker, so imports have independent per-call budgets.
    static LOAD_BUDGET: RefCell<Option<LoadBudget>> = const { RefCell::new(None) };
}

struct LoadGuard(Option<LoadBudget>);
impl Drop for LoadGuard {
    fn drop(&mut self) {
        LOAD_BUDGET.with(|budget| *budget.borrow_mut() = self.0.take());
    }
}

fn object_filter(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    LOAD_BUDGET.with(|budget| {
        let mut budget = budget.borrow_mut();
        let state = budget.as_mut()?;
        if state.error.is_some() {
            return None;
        }
        state.objects += 1;
        if state.objects > MAX_PDF_OBJECTS {
            state.error = Some("PDF exceeds the editable object-count limit".into());
            return None;
        }
        if let Object::Stream(stream) = object
            && stream.dict.has_type(b"ObjStm")
        {
            let remaining = MAX_OBJECT_STREAM_BYTES.saturating_sub(state.object_stream_bytes);
            match stream.get_plain_content_with_limit(MAX_STREAM_BYTES.min(remaining)) {
                Ok(content) => {
                    state.object_stream_bytes += content.len();
                    // The loader then reads the already bounded plain stream;
                    // repeated references cannot decompress it again.
                    stream.set_plain_content(content);
                }
                Err(_) => {
                    state.error = Some("PDF object streams exceed the decompression limit".into());
                    return None;
                }
            }
        }
        Some((id, object.clone()))
    })
}

fn load_pdf(bytes: &[u8]) -> Result<(Document, PdfInfo), String> {
    if bytes.len() > MAX_PDF_SOURCE_BYTES {
        return Err("PDF exceeds the 32 MiB processing limit".into());
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err("The file is not a PDF document".into());
    }
    let previous = LOAD_BUDGET.with(|budget| budget.borrow_mut().replace(LoadBudget::default()));
    let _guard = LoadGuard(previous);
    let parsed = std::panic::catch_unwind(|| {
        Document::load_mem_with_options(
            bytes,
            LoadOptions {
                filter: Some(object_filter),
                max_decompressed_size: Some(MAX_STREAM_BYTES),
                strict: true,
                ..Default::default()
            },
        )
    });
    if let Some(error) = LOAD_BUDGET.with(|budget| {
        budget
            .borrow()
            .as_ref()
            .and_then(|state| state.error.clone())
    }) {
        return Err(error);
    }
    let mut document = parsed
        .map_err(|_| "PDF contains malformed data that could not be processed".to_string())?
        .map_err(|error| format!("PDF could not be parsed: {error}"))?;
    if document.objects.len() > MAX_PDF_OBJECTS {
        return Err("PDF exceeds the editable object-count limit".into());
    }
    // `is_encrypted()` only recognizes an indirect encryption dictionary.
    // Some valid producers (including MuPDF) store /Encrypt directly in the
    // trailer; lopdf then leaves the body unloaded while that method is false.
    // Keep either form readable as original bytes for PDF.js password entry.
    // Also keep automatically unlocked, originally encrypted files read-only.
    if document.trailer.has(b"Encrypt") || document.was_encrypted() {
        return Ok((
            document,
            PdfInfo {
                page_count: 0,
                encrypted: true,
            },
        ));
    }
    // Do not collect this iterator directly: lopdf's size_hint trusts /Count,
    // which is untrusted PDF metadata and can request a huge allocation.
    let mut pages = Vec::new();
    for id in document.page_iter() {
        pages.push(id);
        if pages.len() > MAX_PDF_PAGES {
            break;
        }
    }
    let page_count = pages.len();
    if page_count == 0 {
        return Err("PDF contains no readable pages".into());
    }
    if page_count > MAX_PDF_PAGES {
        return Err("PDF exceeds the 500-page processing limit".into());
    }
    // Its text extractor internally collects the same iterator. Give that
    // extractor a bounded root list of actual leaf pages. Each page retains
    // its original Parent reference, so inherited resources/fonts still work.
    // Only this temporary parsed model changes; the source bytes are retained.
    let root = document
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|_| "PDF page tree is missing".to_string())?;
    let root = document
        .get_dictionary_mut(root)
        .map_err(|_| "PDF page tree is invalid".to_string())?;
    root.set(
        "Kids",
        pages.into_iter().map(Object::Reference).collect::<Vec<_>>(),
    );
    root.set("Count", page_count as i64);
    Ok((
        document,
        PdfInfo {
            page_count,
            encrypted: false,
        },
    ))
}

pub fn validate_pdf(bytes: &[u8]) -> Result<PdfInfo, String> {
    load_pdf(bytes).map(|(_, info)| info)
}

pub fn pdf_to_writer(bytes: &[u8], title: &str) -> Result<PdfImport, String> {
    let (pdf, info) = load_pdf(bytes)?;
    if info.encrypted {
        return Err("Password-protected PDFs can be viewed with a password, but cannot be converted or saved by the native converter. Download an unlocked copy first.".into());
    }
    check_unicode_maps(&pdf)?;
    let mut pages = Vec::with_capacity(info.page_count);
    let mut text_bytes = 0usize;
    for page in 1..=info.page_count {
        let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pdf.extract_text_with_limit(&[page as u32], MAX_PAGE_TEXT_BYTES)
        }))
        .map_err(|_| format!("PDF page {page} contains malformed text data"))?
        .map_err(|error| format!("PDF page {page} text could not be extracted: {error}"))?;
        text_bytes = text_bytes
            .checked_add(text.len())
            .ok_or("PDF text size overflow")?;
        if text_bytes > MAX_TEXT_BYTES {
            return Err("PDF text exceeds the editable text-size limit".into());
        }
        pages.push(text);
    }
    writer_from_pages(title, pages, false)
}

fn check_unicode_maps(pdf: &Document) -> Result<(), String> {
    let mut checked = BTreeSet::new();
    let mut font_bytes = 0usize;
    for object in pdf.objects.values() {
        let Ok(dictionary) = object.as_dict() else {
            continue;
        };
        let Ok(reference) = dictionary.get(b"ToUnicode").and_then(Object::as_reference) else {
            continue;
        };
        if !checked.insert(reference) {
            continue;
        }
        let stream = pdf
            .get_object(reference)
            .and_then(Object::as_stream)
            .map_err(|_| "PDF has an invalid Unicode font map".to_string())?;
        let content = stream
            .get_plain_content_with_limit(
                MAX_PAGE_TEXT_BYTES.min(MAX_FONT_MAP_BYTES.saturating_sub(font_bytes)),
            )
            .map_err(|_| "PDF Unicode font map exceeds the conversion limit".to_string())?;
        font_bytes += content.len();
        // A font can map one tiny glyph code to a long Unicode string. Bound
        // that expansion before the text extractor allocates its output.
        // Eight UTF-16 code units already cover normal ligatures/sequences.
        let mut hexadecimal = None;
        for (index, byte) in content.iter().copied().enumerate() {
            if byte == b'<'
                && content.get(index + 1) != Some(&b'<')
                && (index == 0 || content[index - 1] != b'<')
            {
                hexadecimal = Some(0usize);
            } else if byte == b'>' {
                hexadecimal = None;
            } else if byte.is_ascii_hexdigit()
                && let Some(length) = &mut hexadecimal
            {
                *length += 1;
                if *length > 32 {
                    return Err("PDF Unicode glyph expansion exceeds the conversion limit".into());
                }
            }
        }
    }
    Ok(())
}

/// Accept browser OCR only as an explicitly identified text reconstruction.
/// The caller still supplies the actual PDF so neither its page count nor the
/// unchanged source used for compare-and-swap can be replaced by client data.
pub fn pdf_to_writer_with_pages(
    bytes: &[u8],
    title: &str,
    pages: Vec<String>,
) -> Result<PdfImport, String> {
    let info = validate_pdf(bytes)?;
    if info.encrypted {
        return Err("Unlock the PDF before converting its OCR text".into());
    }
    if pages.len() != info.page_count {
        return Err("OCR text page count must match the source PDF".into());
    }
    writer_from_pages(title, pages, true)
}

fn writer_from_pages(title: &str, pages: Vec<String>, ocr: bool) -> Result<PdfImport, String> {
    if pages.is_empty() || pages.len() > MAX_PDF_PAGES {
        return Err("PDF page count is outside the processing limit".into());
    }
    let page_count = pages.len();
    let mut document = WriterDocument::new(title.to_owned());
    let mut text_bytes = 0usize;
    let mut empty_pages = 0usize;
    for (index, text) in pages.into_iter().enumerate() {
        if text.len() > MAX_PAGE_TEXT_BYTES {
            return Err("A PDF text page exceeds the 1 MiB editable-text limit".into());
        }
        text_bytes = text_bytes
            .checked_add(text.len())
            .ok_or("PDF text size overflow")?;
        if text_bytes > MAX_TEXT_BYTES {
            return Err("PDF text exceeds the editable text-size limit".into());
        }
        if index > 0 {
            document.body.push(Block::PageBreak);
        }
        let mut has_text = false;
        for line in text.lines() {
            let clean: String = line
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect();
            let clean = clean.trim_end();
            if clean.trim().is_empty() {
                continue;
            }
            has_text = true;
            document.add_paragraph(clean);
            if document.body.len() > MAX_TEXT_BLOCKS {
                return Err("PDF contains too many editable text blocks".into());
            }
        }
        if !has_text {
            empty_pages += 1;
        }
    }
    if empty_pages == page_count {
        return Err("This PDF has no extractable text layer. It may be scanned or use outlined glyphs; OCR is required before it can become an editable document.".into());
    }
    document.metadata.page_count = page_count;
    document.metadata.char_count = document
        .body
        .iter()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph
                .runs
                .iter()
                .map(|run| run.text.chars().count())
                .sum(),
            _ => 0,
        })
        .sum();
    document.metadata.word_count = document
        .body
        .iter()
        .map(|block| match block {
            Block::Paragraph(paragraph) => paragraph
                .runs
                .iter()
                .map(|run| run.text.split_whitespace().count())
                .sum(),
            _ => 0,
        })
        .sum();
    let model = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    if model.len() > syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES {
        return Err("Converted PDF exceeds the editable document-size limit".into());
    }
    let mut warnings = if ocr {
        vec!["This editable document was reconstructed from OCR text. Proofread recognition errors, especially mathematical formulas, tables and reading order. Images and the exact layout remain in the PDF original.".into()]
    } else {
        vec!["PDF conversion preserves the selectable text and page boundaries. Fixed layout, drawings, images and original font styling remain in the PDF original.".into()]
    };
    if empty_pages > 0 {
        warnings.push(format!("{empty_pages} page(s) have no extractable text layer and require OCR; their page boundaries are retained."));
    }
    Ok(PdfImport {
        document,
        warnings,
        page_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    pub fn fixture(english: &str, chinese: bool) -> Vec<u8> {
        let mut pdf = Document::with_version("1.5");
        let pages = pdf.new_object_id();
        let latin =
            pdf.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type1","BaseFont"=>"Helvetica"});
        let mut kids = Vec::new();
        let content = pdf.add_object(Stream::new(
            dictionary! {},
            format!("BT /F1 12 Tf 72 720 Td ({english}) Tj ET").into_bytes(),
        ));
        let resources = dictionary! {"Font"=>dictionary!{"F1"=>latin}};
        let page = pdf.add_object(dictionary! {"Type"=>"Page","Parent"=>pages,"Resources"=>resources,"Contents"=>content,"MediaBox"=>vec![0.into(),0.into(),595.into(),842.into()]});
        kids.push(page.into());
        if chinese {
            let cmap = pdf.add_object(Stream::new(dictionary!{}, b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /SyntheticUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfchar <0001> <4F60> <0002> <597D> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end".to_vec()));
            let cid = pdf.add_object(dictionary! {"Type"=>"Font","Subtype"=>"CIDFontType2","BaseFont"=>"SyntheticCJK","CIDSystemInfo"=>dictionary!{"Registry"=>Object::string_literal("Adobe"),"Ordering"=>Object::string_literal("Identity"),"Supplement"=>0}});
            let font = pdf.add_object(dictionary! {"Type"=>"Font","Subtype"=>"Type0","BaseFont"=>"SyntheticCJK","Encoding"=>"Identity-H","DescendantFonts"=>vec![cid.into()],"ToUnicode"=>cmap});
            let content = pdf.add_object(Stream::new(
                dictionary! {},
                b"BT /F2 12 Tf 72 720 Td <00010002> Tj ET".to_vec(),
            ));
            let page = pdf.add_object(dictionary! {"Type"=>"Page","Parent"=>pages,"Resources"=>dictionary!{"Font"=>dictionary!{"F2"=>font}},"Contents"=>content,"MediaBox"=>vec![0.into(),0.into(),595.into(),842.into()]});
            kids.push(page.into());
        }
        let count = kids.len() as i64;
        pdf.objects.insert(
            pages,
            Object::Dictionary(dictionary! {"Type"=>"Pages","Kids"=>kids,"Count"=>count}),
        );
        let catalog = pdf.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
        pdf.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn converts_english_and_chinese_text_with_page_boundaries() {
        let bytes = fixture("Selectable PDF text", true);
        assert_eq!(validate_pdf(&bytes).unwrap().page_count, 2);
        let imported = pdf_to_writer(&bytes, "Imported PDF").unwrap();
        let text = imported
            .document
            .body
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(paragraph) => Some(
                    paragraph
                        .runs
                        .iter()
                        .map(|run| run.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Selectable PDF text"));
        assert!(text.contains("你好"));
        assert_eq!(
            imported
                .document
                .body
                .iter()
                .filter(|block| matches!(block, Block::PageBreak))
                .count(),
            1
        );
    }

    #[test]
    fn refuses_scan_only_and_invalid_documents_instead_of_returning_empty_writer() {
        assert!(
            pdf_to_writer(&fixture("", false), "Scanned")
                .unwrap_err()
                .contains("OCR")
        );
        assert!(validate_pdf(b"not a PDF").is_err());
        assert!(validate_pdf(b"%PDF-1.7\n%%EOF").is_err());
    }

    #[test]
    fn loader_rejects_an_object_stream_before_unbounded_expansion() {
        let mut bytes = fixture("safe", false);
        let pdf = Document::load_mem(&bytes).unwrap();
        let mut stream = Stream::new(
            dictionary! {"Type"=>"ObjStm","N"=>0,"First"=>0},
            vec![b' '; MAX_STREAM_BYTES + 1],
        );
        stream.compress().unwrap();
        // The regular writer intentionally removes existing object streams.
        // Add a real incremental object/xref section instead, so the parser
        // must encounter and bound the compressed stream during loading.
        let id = pdf.max_id + 1;
        let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
        bytes.extend_from_slice(b"\n");
        let offset = bytes.len();
        bytes.extend_from_slice(format!("{id} 0 obj\n<< /Type /ObjStm /N 0 /First 0 /Filter /FlateDecode /Length {} >>\nstream\n",stream.content.len()).as_bytes());
        bytes.extend_from_slice(&stream.content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n{id} 1\n{offset:010} 00000 n \ntrailer\n<< /Size {} /Root {} {} R /Prev {} >>\nstartxref\n{xref}\n%%EOF\n",id+1,root.0,root.1,pdf.xref_start).as_bytes());
        assert!(bytes.len() < 100_000);
        assert!(
            validate_pdf(&bytes)
                .unwrap_err()
                .contains("decompression limit")
        );
    }

    #[test]
    fn ocr_pages_must_match_the_source_and_are_identified_as_reconstructed_text() {
        let source = fixture("", false);
        let import =
            pdf_to_writer_with_pages(&source, "OCR", vec!["识别后的数学文字 1 + 1 = 2".into()])
                .unwrap();
        assert_eq!(import.page_count, 1);
        assert!(import.warnings[0].contains("OCR"));
        assert!(
            pdf_to_writer_with_pages(&source, "OCR", vec![])
                .unwrap_err()
                .contains("page count")
        );
        assert!(
            pdf_to_writer_with_pages(&source, "OCR", vec!["x".repeat(MAX_PAGE_TEXT_BYTES + 1)])
                .unwrap_err()
                .contains("1 MiB")
        );
    }

    #[test]
    fn untrusted_page_counts_cannot_drive_allocation_and_inherited_fonts_survive() {
        let mut pdf = Document::load_mem(&fixture("Inherited bounded text", false)).unwrap();
        let root = pdf
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        let leaf = pdf
            .get_dictionary(root)
            .unwrap()
            .get(b"Kids")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .as_reference()
            .unwrap();
        let child = pdf.new_object_id();
        let resources = pdf
            .get_dictionary_mut(leaf)
            .unwrap()
            .remove(b"Resources")
            .unwrap();
        pdf.get_dictionary_mut(leaf).unwrap().set("Parent", child);
        pdf.objects.insert(child,Object::Dictionary(dictionary!{"Type"=>"Pages","Parent"=>root,"Kids"=>vec![leaf.into()],"Count"=>i64::MAX,"Resources"=>resources}));
        let root = pdf.get_dictionary_mut(root).unwrap();
        root.set("Kids", vec![Object::Reference(child)]);
        root.set("Count", i64::MAX);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        assert_eq!(validate_pdf(&bytes).unwrap().page_count, 1);
        let imported = pdf_to_writer(&bytes, "Count bounds").unwrap();
        let text = serde_json::to_string(&imported.document.body).unwrap();
        assert!(text.contains("Inherited bounded text"));
    }
}
