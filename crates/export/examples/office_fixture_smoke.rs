//! Generate standard Office files for independent-reader smoke verification.
//! Usage: cargo run -p contrix-export --example office_fixture_smoke -- <output-dir>
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("Pass an explicit output directory")?;
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output)?;
    let workbook = contrix_export::xlsx::xlsx_to_workbook(include_bytes!(
        "../tests/fixtures/external-rich.xlsx"
    ))?;
    let presentation = contrix_export::pptx::pptx_to_presentation(include_bytes!(
        "../tests/fixtures/external-rich.pptx"
    ))?;
    std::fs::write(
        output.join("imported-standard.xlsx"),
        contrix_export::xlsx::workbook_to_xlsx(&workbook)?,
    )?;
    std::fs::write(
        output.join("imported-standard.pptx"),
        contrix_export::pptx::presentation_to_pptx(&presentation)?,
    )?;
    println!("{}", output.display());
    Ok(())
}
