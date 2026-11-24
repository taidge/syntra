//! Generate standard Office files for independent-reader smoke verification.
//! Usage: cargo run -p syntra-export --example office_fixture_smoke -- <output-dir>
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("Pass an explicit output directory")?;
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output)?;
    let workbook = syntra_export::xlsx::xlsx_to_workbook(include_bytes!(
        "../tests/fixtures/external-rich.xlsx"
    ))?;
    let presentation = syntra_export::pptx::pptx_to_presentation(include_bytes!(
        "../tests/fixtures/external-rich.pptx"
    ))?;
    std::fs::write(
        output.join("imported-standard.xlsx"),
        syntra_export::xlsx::workbook_to_xlsx(&workbook)?,
    )?;
    std::fs::write(
        output.join("imported-standard.pptx"),
        syntra_export::pptx::presentation_to_pptx(&presentation)?,
    )?;
    println!("{}", output.display());
    Ok(())
}
