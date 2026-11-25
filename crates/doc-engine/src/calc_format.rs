//! Shared SpreadsheetML display formatting for the editor and print output.
use super::CellValue;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedCell {
    pub text: String,
    pub color: Option<String>,
}

/// Format the calculated value, keeping its numeric/date type in the model.
pub fn format_cell_value(value: &CellValue, pattern: Option<&str>) -> FormattedCell {
    let raw = match value {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => n.to_string(),
        CellValue::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        CellValue::Text(s) | CellValue::Date(s) | CellValue::Error(s) => s.clone(),
    };
    if matches!(
        value,
        CellValue::Text(_) | CellValue::Boolean(_) | CellValue::Error(_) | CellValue::Empty
    ) {
        let sections = split_sections(pattern.unwrap_or("General"));
        let text = sections
            .get(3)
            .map(|section| literal(section).replace('@', &raw))
            .unwrap_or(raw);
        return FormattedCell { text, color: None };
    }
    format_display_value(&raw, pattern.unwrap_or("General"))
}

/// Editor calculation snapshots contain display-independent strings. ISO dates
/// are recognized only for a date format, never for an ordinary text cell.
pub fn format_display_value(raw: &str, pattern: &str) -> FormattedCell {
    let alias = match pattern {
        "currency" => "$#,##0.00",
        "percent" => "0%",
        "decimal0" => "0",
        "decimal2" => "0.00",
        _ => pattern,
    };
    if alias.is_empty() || alias.eq_ignore_ascii_case("general") {
        return FormattedCell {
            text: raw.into(),
            color: None,
        };
    }
    let number = raw.parse::<f64>().ok().filter(|n| n.is_finite());
    let sections = split_sections(alias);
    let selected = number
        .map(|n| select_section(&sections, n))
        .unwrap_or((sections.first().copied().unwrap_or(""), false));
    let (section, unsigned) = selected;
    let color = section_color(section);
    if section.is_empty() {
        return FormattedCell {
            text: String::new(),
            color,
        };
    }
    let mut pattern = remove_brackets(section);
    if date_pattern(&pattern) {
        let date = if let Some(n) = number {
            serial_date(n)
        } else {
            iso_date(raw)
        };
        if let Some(date) = date {
            return FormattedCell {
                text: format_date(date, &pattern),
                color,
            };
        }
    }
    let Some(mut number) = number else {
        return FormattedCell {
            text: raw.into(),
            color,
        };
    };
    if unsigned {
        number = number.abs();
    }
    let tokens = unquoted(&pattern);
    let percent_count = tokens.chars().filter(|c| *c == '%').count();
    number *= 100_f64.powi(percent_count.min(4) as i32);
    let Some(first) = pattern
        .char_indices()
        .find(|(_, c)| matches!(c, '0' | '#' | '?'))
        .map(|(i, _)| i)
    else {
        return FormattedCell {
            text: literal(&pattern).replace('@', raw),
            color,
        };
    };
    let last = pattern
        .char_indices()
        .filter(|(_, c)| matches!(c, '0' | '#' | '?'))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(first);
    let prefix = literal(&pattern[..first]);
    let mut suffix = literal(&pattern[last..]);
    let numeric_pattern = &pattern[first..last];
    // Trailing commas scale the value, whereas commas between placeholders group it.
    let trailing_commas = suffix.chars().take_while(|c| *c == ',').count();
    if trailing_commas > 0 {
        number /= 1000_f64.powi(trailing_commas.min(4) as i32);
        suffix = suffix[trailing_commas..].into();
    }
    let mut formatted =
        if let Some((mantissa, exponent)) = numeric_pattern.to_ascii_uppercase().split_once('E') {
            let decimals = mantissa
                .split_once('.')
                .map(|(_, s)| s.chars().filter(|c| matches!(c, '0' | '#' | '?')).count())
                .unwrap_or(0)
                .min(12);
            let text = format!("{number:.decimals$E}");
            if let Some((left, right)) = text.split_once('E') {
                let exponent_value = right.parse::<i32>().unwrap_or(0);
                let digits = exponent.chars().filter(|c| *c == '0').count().min(8);
                format!(
                    "{left}E{}{value:0digits$}",
                    if exponent_value < 0 {
                        "-"
                    } else if exponent.contains('+') {
                        "+"
                    } else {
                        ""
                    },
                    value = exponent_value.unsigned_abs()
                )
            } else {
                text
            }
        } else {
            let decimal_pattern = numeric_pattern
                .split_once('.')
                .map(|(_, s)| s)
                .unwrap_or("");
            let decimals = decimal_pattern
                .chars()
                .filter(|c| matches!(c, '0' | '#' | '?'))
                .count()
                .min(12);
            let required = decimal_pattern
                .chars()
                .filter(|c| *c == '0')
                .count()
                .min(decimals);
            let mut text = format!("{number:.decimals$}");
            while decimals > required
                && text.contains('.')
                && text.ends_with('0')
                && text.rsplit('.').next().unwrap_or("").len() > required
            {
                text.pop();
            }
            if text.ends_with('.') {
                text.pop();
            }
            let (integer, fraction) = text
                .split_once('.')
                .map(|(a, b)| (a.to_owned(), format!(".{b}")))
                .unwrap_or((text.clone(), String::new()));
            let negative = integer.starts_with('-');
            let mut integer = integer.trim_start_matches('-').to_owned();
            let integer_pattern = numeric_pattern.split('.').next().unwrap_or("");
            let required_digits = integer_pattern
                .chars()
                .filter(|c| *c == '0')
                .count()
                .min(32);
            if integer.len() < required_digits {
                integer = format!("{integer:0>required_digits$}");
            }
            if integer_pattern.contains(',') {
                let mut grouped = String::new();
                for (i, c) in integer.chars().enumerate() {
                    if i > 0 && (integer.len() - i) % 3 == 0 {
                        grouped.push(',');
                    }
                    grouped.push(c);
                }
                integer = grouped;
            }
            format!("{}{integer}{fraction}", if negative { "-" } else { "" })
        };
    formatted = format!("{prefix}{formatted}{suffix}");
    // Formatting characters are bounded by the document content budget.
    pattern.clear();
    FormattedCell {
        text: formatted,
        color,
    }
}

fn split_sections(pattern: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quote = false;
    let mut escape = false;
    for (i, c) in pattern.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if c == '"' {
            quote = !quote
        } else if c == ';' && !quote {
            out.push(&pattern[start..i]);
            start = i + 1;
        }
    }
    out.push(&pattern[start..]);
    out
}
fn select_section<'a>(sections: &[&'a str], number: f64) -> (&'a str, bool) {
    let conditional = sections.iter().take(3).any(|s| condition(s).is_some());
    if conditional {
        for section in sections.iter().take(3) {
            if condition(section)
                .map(|(op, n)| match op {
                    ">=" => number >= n,
                    "<=" => number <= n,
                    "<>" => number != n,
                    ">" => number > n,
                    "<" => number < n,
                    _ => number == n,
                })
                .unwrap_or(true)
            {
                return (section, false);
            }
        }
        return ("", false);
    }
    if number < 0. && sections.len() > 1 {
        (sections[1], true)
    } else if number == 0. && sections.len() > 2 {
        (sections[2], false)
    } else {
        (sections.first().copied().unwrap_or(""), false)
    }
}
fn condition(section: &str) -> Option<(&str, f64)> {
    for chunk in section.split('[').skip(1) {
        let chunk = chunk.split(']').next()?;
        for op in [">=", "<=", "<>", ">", "<", "="] {
            if let Some(n) = chunk.strip_prefix(op).and_then(|n| n.parse().ok()) {
                return Some((op, n));
            }
        }
    }
    None
}
fn section_color(section: &str) -> Option<String> {
    for (name, color) in [
        ("black", "#000000"),
        ("blue", "#0000FF"),
        ("cyan", "#00FFFF"),
        ("green", "#008000"),
        ("magenta", "#FF00FF"),
        ("red", "#FF0000"),
        ("white", "#FFFFFF"),
        ("yellow", "#FFFF00"),
    ] {
        if section.to_ascii_lowercase().contains(&format!("[{name}]")) {
            return Some(color.into());
        }
    }
    None
}
fn remove_brackets(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '[' {
            let chunk = chars.by_ref().take_while(|c| *c != ']').collect::<String>();
            if let Some(currency) = chunk.strip_prefix('$') {
                out.push_str(currency.split('-').next().unwrap_or(""));
            } else if matches!(chunk.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
                out.push_str(&chunk)
            }
        } else {
            out.push(c)
        }
    }
    out
}
fn literal(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {}
            '\\' => {
                if let Some(c) = chars.next() {
                    out.push(c)
                }
            }
            '_' => {
                chars.next();
                out.push(' ');
            }
            '*' => {
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}
fn unquoted(pattern: &str) -> String {
    let mut out = String::new();
    let mut quote = false;
    let mut escaped = false;
    for c in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quote = !quote;
            continue;
        }
        if !quote {
            out.push(c)
        }
    }
    out
}
fn date_pattern(pattern: &str) -> bool {
    let p = unquoted(pattern).to_ascii_lowercase();
    p.contains('y') || p.contains('d') || p.contains('h') || p.contains('s') || p.contains("mmm")
}
fn iso_date(value: &str) -> Option<NaiveDateTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.naive_local())
        .or_else(|| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").ok())
        .or_else(|| {
            NaiveDate::parse_from_str(value.get(..10).unwrap_or(value), "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })
}
fn serial_date(value: f64) -> Option<NaiveDateTime> {
    if !(0. ..3_000_000.).contains(&value) {
        return None;
    }
    NaiveDate::from_ymd_opt(1899, 12, 30)?
        .and_hms_opt(0, 0, 0)?
        .checked_add_signed(chrono::Duration::milliseconds(
            (value * 86_400_000.).round() as i64,
        ))
}
fn format_date(value: NaiveDateTime, pattern: &str) -> String {
    let chars = pattern.chars().collect::<Vec<_>>();
    let lower = pattern.to_ascii_lowercase();
    let twelve = lower.contains("am/pm");
    let mut out = String::new();
    let mut i = 0;
    let mut quote = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quote = !quote;
            i += 1;
            continue;
        }
        if c == '\\' {
            i += 1;
            if i < chars.len() {
                out.push(chars[i]);
                i += 1
            }
            continue;
        }
        if quote {
            out.push(c);
            i += 1;
            continue;
        }
        if c == '_' || c == '*' {
            i += 2;
            continue;
        }
        if chars[i..]
            .iter()
            .take(5)
            .collect::<String>()
            .eq_ignore_ascii_case("AM/PM")
        {
            out.push_str(if value.hour() < 12 { "AM" } else { "PM" });
            i += 5;
            continue;
        }
        let key = c.to_ascii_lowercase();
        if !matches!(key, 'y' | 'm' | 'd' | 'h' | 's') {
            out.push(c);
            i += 1;
            continue;
        }
        let mut end = i + 1;
        while end < chars.len() && chars[end].to_ascii_lowercase() == key {
            end += 1
        }
        let count = end - i;
        let padded = |n: u32| {
            if count >= 2 {
                format!("{n:02}")
            } else {
                n.to_string()
            }
        };
        out.push_str(&match key {
            'y' => {
                if count == 2 {
                    format!("{:02}", value.year().rem_euclid(100))
                } else {
                    format!("{:04}", value.year())
                }
            }
            'd' => match count {
                3 => value.format("%a").to_string(),
                4.. => value.format("%A").to_string(),
                _ => padded(value.day()),
            },
            'h' => padded(if twelve {
                let h = value.hour() % 12;
                if h == 0 { 12 } else { h }
            } else {
                value.hour()
            }),
            's' => padded(value.second()),
            'm' => {
                let before = chars[..i].iter().collect::<String>().to_ascii_lowercase();
                let after = chars[end..].iter().collect::<String>().to_ascii_lowercase();
                if before.ends_with("h:") || before.ends_with("hh:") || after.starts_with(":s") {
                    padded(value.minute())
                } else {
                    match count {
                        3 => value.format("%b").to_string(),
                        4 => value.format("%B").to_string(),
                        5 => value.format("%B").to_string().chars().take(1).collect(),
                        _ => padded(value.month()),
                    }
                }
            }
            _ => String::new(),
        });
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ooxml_sections_colors_percent_and_grouping() {
        assert_eq!(
            format_display_value("-1234.56", "\"¥\"#,##0.00;[Red](\"¥\"#,##0.00)").text,
            "(¥1,234.56)"
        );
        assert_eq!(
            format_display_value("-1234.56", "0.00;[Red](0.00)")
                .color
                .as_deref(),
            Some("#FF0000")
        );
        assert_eq!(format_display_value("0.05", "0.0%").text, "5.0%");
        assert_eq!(
            format_display_value("10792.5850875", "#,##0.00").text,
            "10,792.59"
        );
        assert_eq!(
            format_display_value("0", "0.00;(0.00);\"zero\"").text,
            "zero"
        );
    }
    #[test]
    fn date_and_literals_are_not_substituted_as_tokens() {
        assert_eq!(
            format_cell_value(
                &CellValue::Date("2026-11-17T00:00:00.000Z".into()),
                Some("yyyy年m月d日")
            )
            .text,
            "2026年11月17日"
        );
        assert_eq!(
            format_display_value("45292", "yyyy-mm-dd").text,
            "2024-01-01"
        );
        assert_eq!(format_display_value("12.5", "0.00\" kg\"").text, "12.50 kg");
    }
    #[test]
    fn scaled_scientific_and_conditional_formats() {
        assert_eq!(format_display_value("1200000", "0.0,,\"M\"").text, "1.2M");
        assert_eq!(format_display_value("1234", "0.00E+00").text, "1.23E+03");
        assert_eq!(format_display_value("5", "[>9]0.0;[Red]0.00").text, "5.00");
    }
}
