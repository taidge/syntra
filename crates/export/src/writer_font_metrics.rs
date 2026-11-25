//! Font facts measured from the named installed font files; no font binaries
//! are included. Values are normalized by unitsPerEm, use hhea unless OS/2
//! USE_TYPO_METRICS selects typo values, and retain the CJK code-page flag.
//! They model Office's line box, not a fixture-specific baseline or coordinate.
use std::collections::BTreeMap;
use syntra_doc_engine::writer::{Block, FontMetrics, ListItem, TextRun};

fn canonical(family: &str) -> String {
    match family
        .trim()
        .trim_matches(['\'', '"'])
        .to_lowercase()
        .as_str()
    {
        "宋体" => "simsun".into(),
        "新宋体" => "nsimsun".into(),
        "黑体" => "simhei".into(),
        "微软雅黑" => "microsoft yahei".into(),
        "微软雅黑 ui" => "microsoft yahei ui".into(),
        "仿宋" | "仿宋_gb2312" => "fangsong".into(),
        "楷体" | "楷体_gb2312" => "kaiti".into(),
        "ｍｓ ゴシック" => "ms gothic".into(),
        "ｍｓ ｐゴシック" => "ms pgothic".into(),
        "ｍｓ ｕｉ ゴシック" => "ms ui gothic".into(),
        normalized => normalized.to_owned(),
    }
}

pub fn used_metrics(blocks: &[Block]) -> BTreeMap<String, FontMetrics> {
    static FACTS: std::sync::LazyLock<BTreeMap<String, FontMetrics>> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str::<BTreeMap<String, FontMetrics>>(include_str!(
                "office_font_metrics.json"
            ))
            .expect("checked-in measured font facts")
            .into_iter()
            .map(|(family, facts)| (canonical(&family), facts))
            .collect()
        });
    fn runs(runs: &[TextRun], output: &mut BTreeMap<String, FontMetrics>) {
        for run in runs {
            if let Some(family) = &run.font_family {
                if let Some(facts) = FACTS.get(&canonical(family)) {
                    output.insert(
                        family.trim().trim_matches(['\'', '"']).to_lowercase(),
                        *facts,
                    );
                }
            }
        }
    }
    fn items(list: &[ListItem], output: &mut BTreeMap<String, FontMetrics>) {
        for item in list {
            runs(&item.runs, output);
            if let Some(family) = item
                .layout
                .as_ref()
                .and_then(|layout| layout.label_font_family.as_ref())
            {
                if let Some(facts) = FACTS.get(&canonical(family)) {
                    output.insert(family.to_lowercase(), *facts);
                }
            }
            items(&item.children, output);
        }
    }
    fn visit(blocks: &[Block], output: &mut BTreeMap<String, FontMetrics>) {
        for block in blocks {
            match block {
                Block::Paragraph(paragraph) => runs(&paragraph.runs, output),
                Block::Heading(heading) => runs(&heading.runs, output),
                Block::List(list) => items(&list.items, output),
                Block::Table(table) => {
                    for row in &table.rows {
                        for cell in &row.cells {
                            visit(&cell.blocks, output);
                        }
                    }
                }
                Block::Blockquote(quote) => visit(&quote.blocks, output),
                _ => {}
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(blocks, &mut output);
    output
}
