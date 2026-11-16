#[test]
fn encrypted_pdf_can_be_identified_for_reading_without_decrypting_its_body() {
    let bytes = include_bytes!("fixtures/password-reader.pdf");
    let info = contrix_export::pdf_import::validate_pdf(bytes).unwrap();
    assert!(info.encrypted);
    assert!(contrix_export::pdf_import::pdf_to_writer(bytes, "Protected").is_err());
}
