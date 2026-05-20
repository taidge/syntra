# Contrix Office — Parallel Task List (Round 4)

> Generated: 2026-05-20
> Builds on completed Rounds 1+2+3 (48 tasks done)
> Focus: Office format **import** (upload → auto-parse → editable)
> Status: **COMPLETE**

---

## Batch P — Workspace prep (sequential, ran first)

- [x] P0. Added `quick-xml = "0.36"` and `calamine = "0.26"` to workspace `Cargo.toml`; referenced from `crates/export/Cargo.toml`. `uuid` promoted to regular deps (needed at runtime by importer Block IDs)

## Batch Q — Importers + missing exporter (parallel)

- [x] Q1. **DOCX import** — `pub fn docx_to_writer(bytes) -> Result<WriterDocument, String>` in `crates/export/src/docx.rs`. Unzips `word/document.xml`, parses via quick-xml, handles `<w:p>`/`<w:r>`/`<w:t>`/`<w:b>`/`<w:i>`/`<w:u>`/`<w:strike>`/`<w:pStyle>`/`<w:tbl>`/`<w:tr>`/`<w:tc>` with table recursion. Title from `docProps/core.xml` `<dc:title>`. 3 new tests pass
- [x] Q2. **XLSX import** — `pub fn xlsx_to_workbook(bytes) -> Result<Workbook, String>` in `crates/export/src/xlsx.rs` via `calamine::Xlsx::new`. Iterates `range.cells()` (calamine 0.26's 3-tuple `(usize, usize, &Data)`), maps `Data::String/Float/Int/Bool/DateTime/Error` → `CellValue`. 9 tests pass
- [x] Q3. **PPTX import + export** — new file `crates/export/src/pptx.rs` (404 lines). `presentation_to_pptx` emits `[Content_Types].xml`, `_rels/.rels`, `ppt/presentation.xml`, slide files with `<p:sp>`/`<p:txBody>`/`<a:t>` for TextBoxes (bold/italic/underline/font props via `<a:rPr>`). `pptx_to_presentation` unzips, sorts slides by numeric suffix, walks XML events with sp_depth/in_txbody state. 5 tests pass
- [x] Q4. **ExportFormat::from_mime** — added to `crates/export/src/format.rs` alongside `from_extension`, strips MIME parameters (`text/html; charset=utf-8`). 3 new tests pass

## Batch R — Server integration (sequential after Q)

- [x] R1+R2. `upload_file` auto-import — after persisting binary, dispatches on extension to the right importer, serializes result to JSON, inserts `NewDocument` row with `file_id` pointing to the same file. `FileResponse.document_id` field added to DTO and threaded through **9 build sites** (files.rs upload + helper, folders.rs ×3, upload.rs, rooms/trash/recent/favorites helpers). Failed imports log a warning, don't fail the upload

## Batch S — Frontend hook-up (parallel with R)

- [x] S1. `pages/file_manager.rs` — both upload paths (drag-drop `ondrop` and `<input>` `onchange`) now read `FileResponse.document_id`; if `Some`, navigate via `use_navigator()` to `Route::Writer { id }` / `Route::Calc { id }` / `Route::Slides { id }` based on file extension. `state.rs` `FileResponse` mirror updated with `document_id: Option<Uuid>`

---

## Final Status

| Batch | Done | Total | % |
|-------|------|-------|---|
| P — Prep | 1 | 1 | 100% |
| Q — Importers | 4 | 4 | 100% |
| R — Integration | 2 | 2 | 100% |
| S — Frontend | 1 | 1 | 100% |
| **Total** | **8** | **8** | **100%** |

## Verification

- `cargo check --workspace` — **0 errors**
- `cargo check -p contrix-frontend --target wasm32-unknown-unknown` — **0 errors**
- `cargo test -p contrix-export --lib` — **40 passed, 0 failed** (includes new docx/xlsx/pptx import + format round-trip tests)

## What this unlocks

A user uploading `report.docx` / `q3.xlsx` / `pitch.pptx` from the file manager now sees:
1. Binary saved to storage (original fidelity preserved for re-download)
2. Auto-parsed into the internal model
3. New `documents` row created with parsed JSON content
4. Browser **navigates directly into the Writer / Calc / Slides editor** — fully editable
5. Failed parses (corrupt files) fall back to plain-file behavior with a server-side log warning

Legacy binary `.doc / .xls` formats remain unsupported (out of scope per user direction).

## Cumulative Project Tally (Rounds 1 + 2 + 3 + 4)

- **56 tasks completed** across 4 rounds (33 + 8 + 7 + 8)
- **3 new importers**, **1 new exporter** (PPTX)
- **40 unit tests in contrix-export** alone (was 23 before Round 4)
- **Workspace deps**: + `quick-xml`, `calamine`
