//! Reproducible opt-in measurements. These establish evidence, not machine-independent SLOs.
//! cargo test --release -p contrix-server --test capacity_benchmarks -- --ignored --nocapture --test-threads=1
use contrix_doc_engine::calc::{CellValue, Sheet};
use std::time::Instant;

fn distribution(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let percentile =
        |p: f64| samples[((samples.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    serde_json::json!({"samples":samples.len(),"p50_ms":percentile(0.50),
        "p95_ms":percentile(0.95),"p99_ms":percentile(0.99),"max_ms":samples.last()})
}

#[test]
#[ignore = "explicit capacity measurement; run release profile, one test thread, with nocapture"]
fn formula_diamond_dependencies_and_sparse_export_baseline() {
    // Independent four-cell diamonds prevent recursion depth from obscuring
    // the shared-dependency/cache workload. Every result must be exactly six.
    for diamonds in [100, 1_000, 5_000] {
        let mut sheet = Sheet::new("Diamonds".into());
        for row in 0..diamonds {
            sheet.set_cell(row, 0, CellValue::Number(1.0));
            sheet.set_formula(row, 1, format!("=A{}+1", row + 1));
            sheet.set_formula(row, 2, format!("=A{}+3", row + 1));
            sheet.set_formula(row, 3, format!("=B{}+C{}", row + 1, row + 1));
        }
        sheet.recalculate(); // warm process/allocator separately from timed samples
        let mut samples = Vec::new();
        let mut stats = None;
        for _ in 0..30 {
            let start = Instant::now();
            stats = Some(sheet.recalculate_with_stats());
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
            for row in 0..diamonds {
                assert!(
                    matches!(sheet.get_cell(row, 3).map(|cell| &cell.value), Some(CellValue::Number(value)) if *value == 6.0)
                );
            }
        }
        let stats = stats.unwrap();
        println!(
            "{}",
            serde_json::json!({"scenario":"formula_diamonds","diamonds":diamonds,
            "populated_cells":diamonds*4,"formula_cells":diamonds*3,"timing":distribution(samples),
            "calculation_stats":{"evaluated_cells":stats.evaluated_cells,
                "expression_steps":stats.expression_steps,"parsed_formulas":stats.parsed_formulas,
                "budget_exceeded":stats.budget_exceeded},"profile":if cfg!(debug_assertions){"debug"}else{"release"}})
        );
    }
    // Identical nonempty-cell counts, radically different bounding rectangles.
    for (last_row, last_col) in [(999, 9), (1_048_575, 16_383)] {
        let mut workbook = contrix_doc_engine::Workbook::new("Sparse export".into());
        for index in 0..1_000 {
            workbook.sheets[0].set_cell(index, 0, CellValue::Number(f64::from(index)));
        }
        workbook.sheets[0].set_cell(last_row, last_col, CellValue::Text("edge".into()));
        let mut samples = Vec::new();
        let mut output_bytes = 0;
        for _ in 0..30 {
            let start = Instant::now();
            let bytes = contrix_export::xlsx::workbook_to_xlsx(&workbook).expect("sparse export");
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
            output_bytes = bytes.len();
            let restored = contrix_export::xlsx::xlsx_to_workbook(&bytes).expect("reimport");
            assert!(
                matches!(restored.sheets[0].get_cell(last_row,last_col).map(|cell| &cell.value), Some(CellValue::Text(value)) if value=="edge")
            );
        }
        println!(
            "{}",
            serde_json::json!({"scenario":"sparse_xlsx","populated_cells":1001,
            "last_row":last_row,"last_col":last_col,"output_bytes":output_bytes,"timing":distribution(samples),
            "profile":if cfg!(debug_assertions){"debug"}else{"release"}})
        );
    }
}
