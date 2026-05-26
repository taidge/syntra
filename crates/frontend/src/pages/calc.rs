use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::collab_client::{
    CollabHandle, CollabMessage, ConnectionState, RemoteCursor, connect_with_reconnect,
};
use crate::components::find_dialog::FindDialog;
use crate::components::navbar::Navbar;
use crate::state::{AuthState, api_get, api_put};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct WorkbookLite {
    #[serde(default)]
    sheets: Vec<SheetLite>,
}

#[derive(Deserialize)]
struct SheetLite {
    #[serde(default)]
    name: String,
    #[serde(default)]
    cells: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HeaderMenuKind {
    Row,
    Col,
}

fn parse_cell_ref(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().to_uppercase();
    let mut col = 0u32;
    let mut row_start = 0usize;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_alphabetic() {
            col = col * 26 + (ch as u32 - 'A' as u32 + 1);
        } else {
            row_start = i;
            break;
        }
    }
    if row_start == 0 { return None; }
    let row: u32 = s[row_start..].parse().ok()?;
    if row == 0 || col == 0 { return None; }
    Some((row - 1, col - 1))
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CellStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    number_format: String,
}

#[derive(Clone, PartialEq)]
struct CalcSnapshot {
    cells: HashMap<(usize, u32, u32), String>,
    styles: HashMap<(usize, u32, u32), CellStyle>,
    sheets: Vec<String>,
}

fn current_selection_cells(
    active: (u32, u32),
    sel_start: Option<(u32, u32)>,
    sel_end: Option<(u32, u32)>,
    extra: &[(u32, u32)],
) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let (Some(a), Some(b)) = (sel_start, sel_end) {
        let (r1, r2) = (a.0.min(b.0), a.0.max(b.0));
        let (c1, c2) = (a.1.min(b.1), a.1.max(b.1));
        for r in r1..=r2 {
            for c in c1..=c2 {
                out.push((r, c));
            }
        }
    } else {
        out.push(active);
    }
    for &c in extra {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

fn format_number(value: &str, fmt: &str) -> String {
    match fmt {
        "currency" => value
            .parse::<f64>()
            .map(|n| format!("${:.2}", n))
            .unwrap_or_else(|_| value.to_string()),
        "percent" => value
            .parse::<f64>()
            .map(|n| format!("{:.0}%", n * 100.0))
            .unwrap_or_else(|_| value.to_string()),
        "decimal0" => value
            .parse::<f64>()
            .map(|n| format!("{:.0}", n))
            .unwrap_or_else(|_| value.to_string()),
        "decimal2" => value
            .parse::<f64>()
            .map(|n| format!("{:.2}", n))
            .unwrap_or_else(|_| value.to_string()),
        _ => value.to_string(),
    }
}

fn style_to_json(style: &CellStyle) -> Option<serde_json::Value> {
    if style == &CellStyle::default() {
        return None;
    }
    let mut obj = serde_json::Map::new();
    if style.bold {
        obj.insert("bold".to_string(), serde_json::Value::Bool(true));
    }
    if style.italic {
        obj.insert("italic".to_string(), serde_json::Value::Bool(true));
    }
    if style.underline {
        obj.insert("underline".to_string(), serde_json::Value::Bool(true));
    }
    if !style.number_format.is_empty() && style.number_format != "general" {
        obj.insert(
            "number_format".to_string(),
            serde_json::Value::String(style.number_format.clone()),
        );
    }
    Some(serde_json::Value::Object(obj))
}

fn cell_value_to_string(v: &serde_json::Value) -> String {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let inner = v.get("value");
    match ty {
        "Empty" => String::new(),
        "Text" => inner.and_then(|x| x.as_str()).unwrap_or("").to_string(),
        "Number" => inner
            .and_then(|x| x.as_f64())
            .map(|n| n.to_string())
            .unwrap_or_default(),
        "Boolean" => inner
            .and_then(|x| x.as_bool())
            .map(|b| if b { "TRUE" } else { "FALSE" }.to_string())
            .unwrap_or_default(),
        "Date" => inner.and_then(|x| x.as_str()).unwrap_or("").to_string(),
        "Error" => format!(
            "#ERR: {}",
            inner.and_then(|x| x.as_str()).unwrap_or("")
        ),
        _ => String::new(),
    }
}

fn style_from_json(v: &serde_json::Value) -> CellStyle {
    CellStyle {
        bold: v.get("bold").and_then(|b| b.as_bool()).unwrap_or(false),
        italic: v.get("italic").and_then(|b| b.as_bool()).unwrap_or(false),
        underline: v.get("underline").and_then(|b| b.as_bool()).unwrap_or(false),
        number_format: v
            .get("number_format")
            .and_then(|s| s.as_str())
            .unwrap_or("general")
            .to_string(),
    }
}

fn hydrate_workbook(
    content: &serde_json::Value,
) -> (
    Vec<String>,
    HashMap<(usize, u32, u32), String>,
    HashMap<(usize, u32, u32), CellStyle>,
) {
    let wb: WorkbookLite = match serde_json::from_value(content.clone()) {
        Ok(w) => w,
        Err(_) => return (Vec::new(), HashMap::new(), HashMap::new()),
    };
    let mut names = Vec::new();
    let mut cells = HashMap::new();
    let mut styles = HashMap::new();
    for (sheet_idx, sheet) in wb.sheets.iter().enumerate() {
        names.push(sheet.name.clone());
        // `cells` may be either a HashMap object {"row,col": Cell} OR
        // a JSON object whose keys are JSON-encoded CellRef. Serde JSON
        // serializes HashMap<CellRef, Cell> as an array of [CellRef, Cell]
        // pairs because CellRef is not a string. Handle both shapes.
        if let Some(arr) = sheet.cells.as_array() {
            for pair in arr {
                let Some(pair_arr) = pair.as_array() else { continue };
                if pair_arr.len() != 2 { continue; }
                let cref = &pair_arr[0];
                let cell = &pair_arr[1];
                let row = cref.get("row").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let col = cref.get("col").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let value = cell.get("value");
                let formula = cell
                    .get("formula")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
                let display = if let Some(f) = formula {
                    f
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
                if let Some(style_json) = cell.get("style") {
                    let style = style_from_json(style_json);
                    if style != CellStyle::default() {
                        styles.insert((sheet_idx, row, col), style);
                    }
                }
            }
        } else if let Some(obj) = sheet.cells.as_object() {
            for (k, cell) in obj {
                let (row, col) = if let Ok(cref) = serde_json::from_str::<serde_json::Value>(k) {
                    (
                        cref.get("row").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        cref.get("col").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    )
                } else {
                    continue;
                };
                let value = cell.get("value");
                let formula = cell
                    .get("formula")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
                let display = if let Some(f) = formula {
                    f
                } else if let Some(v) = value {
                    cell_value_to_string(v)
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    cells.insert((sheet_idx, row, col), display);
                }
                if let Some(style_json) = cell.get("style") {
                    let style = style_from_json(style_json);
                    if style != CellStyle::default() {
                        styles.insert((sheet_idx, row, col), style);
                    }
                }
            }
        }
    }
    (names, cells, styles)
}

const FORMULA_INPUT_ID: &str = "calc-formula-input";

fn focus_formula_input() {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id(FORMULA_INPUT_ID) {
            if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
                let _ = input.focus();
                let len = input.value().len() as u32;
                let _ = input.set_selection_range(len, len);
            }
        }
    }
}

/// Total number of columns in the spreadsheet grid.
const NUM_COLS: u32 = 26;
/// Total number of rows in the spreadsheet grid.
const NUM_ROWS: u32 = 50;

/// Convert a 0-based column index to a letter (A-Z).
fn col_letter(col: u32) -> char {
    (b'A' + col as u8) as char
}

/// Human-readable cell reference (e.g. "A1", "C12").
fn cell_ref(row: u32, col: u32) -> String {
    format!("{}{}", col_letter(col), row + 1)
}

/// Return the display value for a cell.  Formulas (starting with `=`) are
/// shown verbatim for now — evaluation will happen server-side on save.
fn display_value(raw: &str) -> &str {
    raw
}

fn format_number_default(n: f64) -> String {
    if n.is_nan() {
        "#ERR".to_string()
    } else if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

fn evaluate_cell_display(
    raw: &str,
    sheet_idx: usize,
    cells: &HashMap<(usize, u32, u32), String>,
) -> String {
    if let Some(expr) = raw.strip_prefix('=') {
        match eval::evaluate(expr, sheet_idx, cells) {
            Ok(n) => format_number_default(n),
            Err(_) => "#ERR".to_string(),
        }
    } else {
        raw.to_string()
    }
}

fn evaluated_numeric(
    raw: &str,
    sheet_idx: usize,
    cells: &HashMap<(usize, u32, u32), String>,
) -> Option<f64> {
    if let Some(expr) = raw.strip_prefix('=') {
        eval::evaluate(expr, sheet_idx, cells).ok()
    } else {
        raw.parse::<f64>().ok()
    }
}

mod eval {
    use std::collections::{HashMap, HashSet};

    #[derive(Debug, Clone, PartialEq)]
    pub enum EvalError {
        Parse(String),
        BadRef,
        DivByZero,
        Cycle,
        UnknownFn(String),
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Number(f64),
        Ident(String),
        CellRef(u32, u32),
        Plus,
        Minus,
        Star,
        Slash,
        LParen,
        RParen,
        Comma,
        Colon,
    }

    fn col_from_letters(s: &str) -> Option<u32> {
        let mut v: u32 = 0;
        for ch in s.chars() {
            if !ch.is_ascii_alphabetic() {
                return None;
            }
            v = v * 26 + (ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
        }
        if v == 0 { None } else { Some(v - 1) }
    }

    fn lex(input: &str) -> Result<Vec<Token>, EvalError> {
        let bytes = input.as_bytes();
        let mut i = 0;
        let mut out = Vec::new();
        while i < bytes.len() {
            let b = bytes[i];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                i += 1;
                continue;
            }
            match b {
                b'+' => { out.push(Token::Plus); i += 1; continue; }
                b'-' => { out.push(Token::Minus); i += 1; continue; }
                b'*' => { out.push(Token::Star); i += 1; continue; }
                b'/' => { out.push(Token::Slash); i += 1; continue; }
                b'(' => { out.push(Token::LParen); i += 1; continue; }
                b')' => { out.push(Token::RParen); i += 1; continue; }
                b',' => { out.push(Token::Comma); i += 1; continue; }
                b':' => { out.push(Token::Colon); i += 1; continue; }
                _ => {}
            }
            if b.is_ascii_digit() || b == b'.' {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                let s = &input[start..i];
                let n: f64 = s
                    .parse()
                    .map_err(|_| EvalError::Parse(format!("bad number: {s}")))?;
                out.push(Token::Number(n));
                continue;
            }
            if b == b'$' || b.is_ascii_alphabetic() {
                if b == b'$' { i += 1; }
                let letters_start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let letters = &input[letters_start..i];
                if letters.is_empty() {
                    return Err(EvalError::Parse("lone $".into()));
                }
                if i < bytes.len() && bytes[i] == b'$' { i += 1; }
                let digit_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > digit_start {
                    let row_s = &input[digit_start..i];
                    let row_n: u32 = row_s
                        .parse()
                        .map_err(|_| EvalError::Parse(format!("bad row: {row_s}")))?;
                    let col = col_from_letters(letters).ok_or(EvalError::BadRef)?;
                    if row_n == 0 {
                        return Err(EvalError::BadRef);
                    }
                    out.push(Token::CellRef(row_n - 1, col));
                } else {
                    out.push(Token::Ident(letters.to_ascii_uppercase()));
                }
                continue;
            }
            return Err(EvalError::Parse(format!("unexpected char: {}", b as char)));
        }
        Ok(out)
    }

    #[derive(Debug, Clone)]
    pub enum Expr {
        Number(f64),
        Cell(u32, u32),
        Range(u32, u32, u32, u32),
        Binary(BinOp, Box<Expr>, Box<Expr>),
        Unary(UnOp, Box<Expr>),
        Func(String, Vec<Expr>),
    }

    #[derive(Debug, Clone, Copy)]
    pub enum BinOp { Add, Sub, Mul, Div }
    #[derive(Debug, Clone, Copy)]
    pub enum UnOp { Neg, Pos }

    struct Parser {
        tokens: Vec<Token>,
        pos: usize,
    }

    impl Parser {
        fn peek(&self) -> Option<&Token> { self.tokens.get(self.pos) }
        fn bump(&mut self) -> Option<Token> {
            let t = self.tokens.get(self.pos).cloned();
            if t.is_some() { self.pos += 1; }
            t
        }

        fn parse_expr(&mut self) -> Result<Expr, EvalError> {
            self.parse_addsub()
        }

        fn parse_addsub(&mut self) -> Result<Expr, EvalError> {
            let mut left = self.parse_muldiv()?;
            loop {
                match self.peek() {
                    Some(Token::Plus) => { self.bump(); let r = self.parse_muldiv()?; left = Expr::Binary(BinOp::Add, Box::new(left), Box::new(r)); }
                    Some(Token::Minus) => { self.bump(); let r = self.parse_muldiv()?; left = Expr::Binary(BinOp::Sub, Box::new(left), Box::new(r)); }
                    _ => break,
                }
            }
            Ok(left)
        }

        fn parse_muldiv(&mut self) -> Result<Expr, EvalError> {
            let mut left = self.parse_unary()?;
            loop {
                match self.peek() {
                    Some(Token::Star) => { self.bump(); let r = self.parse_unary()?; left = Expr::Binary(BinOp::Mul, Box::new(left), Box::new(r)); }
                    Some(Token::Slash) => { self.bump(); let r = self.parse_unary()?; left = Expr::Binary(BinOp::Div, Box::new(left), Box::new(r)); }
                    _ => break,
                }
            }
            Ok(left)
        }

        fn parse_unary(&mut self) -> Result<Expr, EvalError> {
            match self.peek() {
                Some(Token::Minus) => { self.bump(); let e = self.parse_unary()?; Ok(Expr::Unary(UnOp::Neg, Box::new(e))) }
                Some(Token::Plus) => { self.bump(); let e = self.parse_unary()?; Ok(Expr::Unary(UnOp::Pos, Box::new(e))) }
                _ => self.parse_primary(),
            }
        }

        fn parse_primary(&mut self) -> Result<Expr, EvalError> {
            let tok = self.bump().ok_or_else(|| EvalError::Parse("unexpected end".into()))?;
            match tok {
                Token::Number(n) => Ok(Expr::Number(n)),
                Token::LParen => {
                    let e = self.parse_expr()?;
                    match self.bump() {
                        Some(Token::RParen) => Ok(e),
                        _ => Err(EvalError::Parse("expected )".into())),
                    }
                }
                Token::CellRef(r, c) => {
                    if matches!(self.peek(), Some(Token::Colon)) {
                        self.bump();
                        match self.bump() {
                            Some(Token::CellRef(r2, c2)) => Ok(Expr::Range(r, c, r2, c2)),
                            _ => Err(EvalError::Parse("expected cell after :".into())),
                        }
                    } else {
                        Ok(Expr::Cell(r, c))
                    }
                }
                Token::Ident(name) => {
                    if !matches!(self.peek(), Some(Token::LParen)) {
                        return Err(EvalError::Parse(format!("expected ( after {name}")));
                    }
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Token::RParen)) {
                        loop {
                            args.push(self.parse_expr()?);
                            match self.peek() {
                                Some(Token::Comma) => { self.bump(); }
                                _ => break,
                            }
                        }
                    }
                    match self.bump() {
                        Some(Token::RParen) => Ok(Expr::Func(name, args)),
                        _ => Err(EvalError::Parse("expected ) in func call".into())),
                    }
                }
                t => Err(EvalError::Parse(format!("unexpected token: {t:?}"))),
            }
        }
    }

    pub fn evaluate(
        formula: &str,
        sheet_idx: usize,
        cells: &HashMap<(usize, u32, u32), String>,
    ) -> Result<f64, EvalError> {
        let tokens = lex(formula)?;
        let mut parser = Parser { tokens, pos: 0 };
        let expr = parser.parse_expr()?;
        if parser.pos != parser.tokens.len() {
            return Err(EvalError::Parse("trailing tokens".into()));
        }
        let mut visit: HashSet<(usize, u32, u32)> = HashSet::new();
        eval_expr(&expr, sheet_idx, cells, &mut visit)
    }

    fn cell_numeric(
        sheet_idx: usize,
        r: u32,
        c: u32,
        cells: &HashMap<(usize, u32, u32), String>,
        visit: &mut HashSet<(usize, u32, u32)>,
    ) -> Result<f64, EvalError> {
        let key = (sheet_idx, r, c);
        if visit.contains(&key) {
            return Err(EvalError::Cycle);
        }
        let Some(raw) = cells.get(&key) else { return Ok(0.0); };
        if raw.is_empty() {
            return Ok(0.0);
        }
        if let Some(expr_src) = raw.strip_prefix('=') {
            visit.insert(key);
            let tokens = lex(expr_src)?;
            let mut parser = Parser { tokens, pos: 0 };
            let expr = parser.parse_expr()?;
            if parser.pos != parser.tokens.len() {
                visit.remove(&key);
                return Err(EvalError::Parse("trailing tokens".into()));
            }
            let res = eval_expr(&expr, sheet_idx, cells, visit);
            visit.remove(&key);
            res
        } else {
            Ok(raw.parse::<f64>().unwrap_or(0.0))
        }
    }

    fn expand_args(
        args: &[Expr],
        sheet_idx: usize,
        cells: &HashMap<(usize, u32, u32), String>,
        visit: &mut HashSet<(usize, u32, u32)>,
    ) -> Result<Vec<(Option<(u32, u32)>, f64)>, EvalError> {
        let mut out = Vec::new();
        for a in args {
            match a {
                Expr::Range(r1, c1, r2, c2) => {
                    let rmin = (*r1).min(*r2);
                    let rmax = (*r1).max(*r2);
                    let cmin = (*c1).min(*c2);
                    let cmax = (*c1).max(*c2);
                    for r in rmin..=rmax {
                        for c in cmin..=cmax {
                            let v = cell_numeric(sheet_idx, r, c, cells, visit)?;
                            out.push((Some((r, c)), v));
                        }
                    }
                }
                Expr::Cell(r, c) => {
                    let v = cell_numeric(sheet_idx, *r, *c, cells, visit)?;
                    out.push((Some((*r, *c)), v));
                }
                other => {
                    let v = eval_expr(other, sheet_idx, cells, visit)?;
                    out.push((None, v));
                }
            }
        }
        Ok(out)
    }

    fn is_cell_numeric(
        sheet_idx: usize,
        r: u32,
        c: u32,
        cells: &HashMap<(usize, u32, u32), String>,
    ) -> bool {
        let key = (sheet_idx, r, c);
        let Some(raw) = cells.get(&key) else { return false; };
        if raw.is_empty() { return false; }
        if let Some(expr_src) = raw.strip_prefix('=') {
            let mut v: HashSet<(usize, u32, u32)> = HashSet::new();
            v.insert(key);
            let Ok(tokens) = lex(expr_src) else { return false; };
            let mut parser = Parser { tokens, pos: 0 };
            let Ok(expr) = parser.parse_expr() else { return false; };
            if parser.pos != parser.tokens.len() { return false; }
            eval_expr(&expr, sheet_idx, cells, &mut v).is_ok()
        } else {
            raw.parse::<f64>().is_ok()
        }
    }

    fn is_cell_non_empty(
        sheet_idx: usize,
        r: u32,
        c: u32,
        cells: &HashMap<(usize, u32, u32), String>,
    ) -> bool {
        cells
            .get(&(sheet_idx, r, c))
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    fn eval_expr(
        e: &Expr,
        sheet_idx: usize,
        cells: &HashMap<(usize, u32, u32), String>,
        visit: &mut HashSet<(usize, u32, u32)>,
    ) -> Result<f64, EvalError> {
        match e {
            Expr::Number(n) => Ok(*n),
            Expr::Cell(r, c) => cell_numeric(sheet_idx, *r, *c, cells, visit),
            Expr::Range(_, _, _, _) => Err(EvalError::Parse("range not allowed here".into())),
            Expr::Unary(op, x) => {
                let v = eval_expr(x, sheet_idx, cells, visit)?;
                Ok(match op { UnOp::Neg => -v, UnOp::Pos => v })
            }
            Expr::Binary(op, l, r) => {
                let lv = eval_expr(l, sheet_idx, cells, visit)?;
                let rv = eval_expr(r, sheet_idx, cells, visit)?;
                Ok(match op {
                    BinOp::Add => lv + rv,
                    BinOp::Sub => lv - rv,
                    BinOp::Mul => lv * rv,
                    BinOp::Div => {
                        if rv == 0.0 { return Err(EvalError::DivByZero); }
                        lv / rv
                    }
                })
            }
            Expr::Func(name, args) => {
                let up = name.to_ascii_uppercase();
                match up.as_str() {
                    "SUM" => {
                        let vals = expand_args(args, sheet_idx, cells, visit)?;
                        Ok(vals.iter().map(|(_, v)| *v).sum())
                    }
                    "AVG" | "AVERAGE" => {
                        let mut sum = 0.0;
                        let mut count = 0u32;
                        for arg in args {
                            match arg {
                                Expr::Range(r1, c1, r2, c2) => {
                                    let rmin = (*r1).min(*r2);
                                    let rmax = (*r1).max(*r2);
                                    let cmin = (*c1).min(*c2);
                                    let cmax = (*c1).max(*c2);
                                    for r in rmin..=rmax {
                                        for c in cmin..=cmax {
                                            if is_cell_non_empty(sheet_idx, r, c, cells) {
                                                sum += cell_numeric(sheet_idx, r, c, cells, visit)?;
                                                count += 1;
                                            }
                                        }
                                    }
                                }
                                other => {
                                    sum += eval_expr(other, sheet_idx, cells, visit)?;
                                    count += 1;
                                }
                            }
                        }
                        if count == 0 { Err(EvalError::DivByZero) } else { Ok(sum / count as f64) }
                    }
                    "MIN" => {
                        let vals = expand_args(args, sheet_idx, cells, visit)?;
                        let mut m = f64::INFINITY;
                        for (_, v) in &vals { if *v < m { m = *v; } }
                        if m.is_infinite() { Ok(0.0) } else { Ok(m) }
                    }
                    "MAX" => {
                        let vals = expand_args(args, sheet_idx, cells, visit)?;
                        let mut m = f64::NEG_INFINITY;
                        for (_, v) in &vals { if *v > m { m = *v; } }
                        if m.is_infinite() { Ok(0.0) } else { Ok(m) }
                    }
                    "COUNT" => {
                        let mut count = 0u32;
                        for arg in args {
                            match arg {
                                Expr::Range(r1, c1, r2, c2) => {
                                    let rmin = (*r1).min(*r2);
                                    let rmax = (*r1).max(*r2);
                                    let cmin = (*c1).min(*c2);
                                    let cmax = (*c1).max(*c2);
                                    for r in rmin..=rmax {
                                        for c in cmin..=cmax {
                                            if is_cell_numeric(sheet_idx, r, c, cells) {
                                                count += 1;
                                            }
                                        }
                                    }
                                }
                                Expr::Cell(r, c) => {
                                    if is_cell_numeric(sheet_idx, *r, *c, cells) {
                                        count += 1;
                                    }
                                }
                                other => {
                                    if eval_expr(other, sheet_idx, cells, visit).is_ok() {
                                        count += 1;
                                    }
                                }
                            }
                        }
                        Ok(count as f64)
                    }
                    "IF" => {
                        if args.len() != 3 {
                            return Err(EvalError::Parse("IF expects 3 args".into()));
                        }
                        let cond = eval_expr(&args[0], sheet_idx, cells, visit)?;
                        if cond != 0.0 {
                            eval_expr(&args[1], sheet_idx, cells, visit)
                        } else {
                            eval_expr(&args[2], sheet_idx, cells, visit)
                        }
                    }
                    other => Err(EvalError::UnknownFn(other.to_string())),
                }
            }
        }
    }
}

/// Return `true` when `cell` is inside the rectangle defined by corners `a`
/// and `b` (inclusive, order-independent).
fn in_selection(cell: (u32, u32), a: (u32, u32), b: (u32, u32)) -> bool {
    let min_row = a.0.min(b.0);
    let max_row = a.0.max(b.0);
    let min_col = a.1.min(b.1);
    let max_col = a.1.max(b.1);
    cell.0 >= min_row && cell.0 <= max_row && cell.1 >= min_col && cell.1 <= max_col
}

/// Tracks an in-progress drag-to-fill gesture.
#[derive(Clone, Copy, PartialEq)]
struct FillState {
    origin_start: (u32, u32),
    origin_end: (u32, u32),
    target: (u32, u32),
}

/// Compute the union rectangle covered by the fill preview.
fn fill_rect(state: &FillState) -> ((u32, u32), (u32, u32)) {
    let (sr, sc) = state.origin_start;
    let (er, ec) = state.origin_end;
    let (tr, tc) = state.target;
    let min_row = sr.min(er).min(tr);
    let max_row = sr.max(er).max(tr);
    let min_col = sc.min(ec).min(tc);
    let max_col = sc.max(ec).max(tc);
    ((min_row, min_col), (max_row, max_col))
}

/// Spreadsheet editor with formula bar, cell grid, and sheet tabs.
#[component]
pub fn Calc(id: String) -> Element {
    // ── Cell data ────────────────────────────────────────────────────────
    // Key: (sheet_index, row, col)  Value: raw text / formula
    let mut cells = use_signal(|| HashMap::<(usize, u32, u32), String>::new());
    let mut cell_styles = use_signal(|| HashMap::<(usize, u32, u32), CellStyle>::new());

    // ── Active cell & editing ────────────────────────────────────────────
    let mut active_cell = use_signal(|| (0u32, 0u32));
    let mut formula_bar = use_signal(|| String::new());
    let mut is_editing = use_signal(|| false);

    // ── Multi-cell selection ─────────────────────────────────────────────
    let mut selection_start = use_signal(|| None::<(u32, u32)>);
    let mut selection_end = use_signal(|| None::<(u32, u32)>);
    let mut is_selecting = use_signal(|| false);
    let mut extra_selection = use_signal(|| Vec::<(u32, u32)>::new());

    // ── Drag-to-fill state ───────────────────────────────────────────────
    let mut fill_mode = use_signal(|| None::<FillState>);

    // ── Sheet tabs ───────────────────────────────────────────────────────
    let mut sheets = use_signal(|| {
        vec![
            "Sheet1".to_string(),
            "Sheet2".to_string(),
            "Sheet3".to_string(),
        ]
    });
    let mut active_sheet = use_signal(|| 0usize);

    // ── Document chrome ──────────────────────────────────────────────────
    let mut doc_title = use_signal(|| "Untitled Spreadsheet".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut loaded = use_signal(|| false);

    let mut undo_stack = use_signal(|| Vec::<CalcSnapshot>::new());
    let mut redo_stack = use_signal(|| Vec::<CalcSnapshot>::new());

    let mut find_open = use_signal(|| false);
    let mut find_matches = use_signal(|| Vec::<(usize, u32, u32)>::new());
    let mut find_index = use_signal(|| 0usize);
    let mut find_info = use_signal(String::new);

    let presence = use_signal(Vec::<RemoteCursor>::new);
    let mut ws_handle = use_signal(|| None::<CollabHandle>);
    let mut conn_state = use_signal(|| ConnectionState::Connecting);

    let mut header_menu = use_signal(|| None::<(HeaderMenuKind, u32, f64, f64)>);
    let mut sheet_menu = use_signal(|| None::<(usize, f64, f64)>);
    let mut renaming_sheet = use_signal(|| None::<usize>);
    let mut rename_buffer = use_signal(String::new);
    let mut address_input = use_signal(|| cell_ref(0, 0));
    let mut address_error = use_signal(|| false);

    let mut push_snapshot = move || {
        let snap = CalcSnapshot {
            cells: cells.read().clone(),
            styles: cell_styles.read().clone(),
            sheets: sheets.read().clone(),
        };
        {
            let mut s = undo_stack.write();
            s.push(snap);
            if s.len() > 100 {
                s.remove(0);
            }
        }
        redo_stack.write().clear();
    };

    let auth = use_context::<Signal<AuthState>>();
    let id_for_resource = id.clone();
    let doc_resource = use_resource(move || {
        let id = id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            api_get::<DocumentResponse>(&format!("/documents/{id}"), &token).await
        }
    });

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        if let Some(Ok(doc)) = doc_resource.read().as_ref() {
            doc_title.set(doc.name.clone());
            let (names, hydrated, styles) = hydrate_workbook(&doc.content);
            if !names.is_empty() {
                sheets.set(names);
            }
            cells.set(hydrated);
            cell_styles.set(styles);
            active_sheet.set(0);
            loaded.set(true);
        }
    });

    let doc_id_for_ws = id.clone();
    use_effect(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        let doc_id = doc_id_for_ws.clone();
        let mut presence_for_msg = presence;
        let on_message = move |msg: CollabMessage| match msg {
            CollabMessage::UserJoined { user_id, username, color } => {
                let mut list = presence_for_msg.write();
                if !list.iter().any(|c| c.user_id == user_id) {
                    list.push(RemoteCursor {
                        user_id,
                        username,
                        color,
                        is_active: true,
                        position: None,
                    });
                }
            }
            CollabMessage::UserLeft { user_id } => {
                let mut list = presence_for_msg.write();
                list.retain(|c| c.user_id != user_id);
            }
            _ => {}
        };
        let on_state = move |s: ConnectionState| {
            conn_state.set(s);
        };
        let handle = connect_with_reconnect(doc_id, token, on_message, on_state);
        ws_handle.set(Some(handle));
    });

    use_drop(move || {
        if let Some(handle) = ws_handle.write().take() {
            if let Some(ws) = handle.current_ws() {
                let _ = ws.close();
            }
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_workbook = move || -> serde_json::Value {
        let names = sheets.read().clone();
        let cells_ref = cells.read();
        let styles_ref = cell_styles.read();
        let mut per_sheet: Vec<Vec<serde_json::Value>> = names.iter().map(|_| Vec::new()).collect();
        let mut emitted: std::collections::HashSet<(usize, u32, u32)> =
            std::collections::HashSet::new();
        for ((si, r, c), value) in cells_ref.iter() {
            if *si >= per_sheet.len() {
                continue;
            }
            let cref = serde_json::json!({ "row": r, "col": c });
            let is_formula = value.starts_with('=');
            let mut cell = if is_formula {
                serde_json::json!({
                    "formula": value,
                    "value": { "type": "Empty" }
                })
            } else if let Ok(n) = value.parse::<f64>() {
                serde_json::json!({
                    "formula": serde_json::Value::Null,
                    "value": { "type": "Number", "value": n }
                })
            } else {
                serde_json::json!({
                    "formula": serde_json::Value::Null,
                    "value": { "type": "Text", "value": value }
                })
            };
            if let Some(style) = styles_ref.get(&(*si, *r, *c)) {
                if let Some(style_json) = style_to_json(style) {
                    if let Some(obj) = cell.as_object_mut() {
                        obj.insert("style".to_string(), style_json);
                    }
                }
            }
            per_sheet[*si].push(serde_json::json!([cref, cell]));
            emitted.insert((*si, *r, *c));
        }
        for ((si, r, c), style) in styles_ref.iter() {
            if *si >= per_sheet.len() {
                continue;
            }
            if emitted.contains(&(*si, *r, *c)) {
                continue;
            }
            let Some(style_json) = style_to_json(style) else { continue };
            let cref = serde_json::json!({ "row": r, "col": c });
            let cell = serde_json::json!({
                "formula": serde_json::Value::Null,
                "value": { "type": "Empty" },
                "style": style_json,
            });
            per_sheet[*si].push(serde_json::json!([cref, cell]));
        }
        let sheets_json: Vec<serde_json::Value> = names
            .iter()
            .enumerate()
            .map(|(i, name)| serde_json::json!({ "name": name, "cells": per_sheet[i] }))
            .collect();
        serde_json::json!({ "sheets": sheets_json })
    };

    let perform_save = {
        let id = id.clone();
        move |body: serde_json::Value, mut status: Signal<String>, token: String| {
            let id = id.clone();
            status.set("Saving…".to_string());
            wasm_bindgen_futures::spawn_local(async move {
                let req = serde_json::json!({ "content": body });
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token).await {
                    Ok(_) => status.set("All changes saved".to_string()),
                    Err(e) => status.set(format!("Save failed: {e}")),
                }
            });
        }
    };

    let schedule_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_workbook = serialize_workbook.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set("Unsaved changes".to_string());
            let body = serialize_workbook();
            let token = auth.read().token.clone().unwrap_or_default();
            let status = saved_status;
            let perform_save = perform_save.clone();
            let new_timer = Timeout::new(1500, move || {
                perform_save(body, status, token);
            });
            *save_timer.borrow_mut() = Some(new_timer);
        }
    };

    let flush_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_workbook = serialize_workbook.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            save_timer.borrow_mut().take();
            let body = serialize_workbook();
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    use_effect(move || {
        let (r, c) = *active_cell.read();
        address_input.set(cell_ref(r, c));
        address_error.set(false);
    });

    // ── Derived values ───────────────────────────────────────────────────
    let (active_row, active_col) = *active_cell.read();
    let sheet_idx = *active_sheet.read();

    // Helper: commit the current formula bar contents into the cell map.
    let commit_formula_bar = {
        let mut schedule_save = schedule_save.clone();
        let mut push_snapshot = push_snapshot.clone();
        move || {
            let value = formula_bar.read().clone();
            let (r, c) = *active_cell.read();
            let si = *active_sheet.read();
            let prev = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
            if prev == value {
                is_editing.set(false);
                return;
            }
            if *loaded.read() {
                push_snapshot();
            }
            if value.is_empty() {
                cells.write().remove(&(si, r, c));
            } else {
                cells.write().insert((si, r, c), value);
            }
            is_editing.set(false);
            schedule_save();
        }
    };

    // ── Status bar aggregates (sum / average / count) ────────────────────
    let (stat_sum, stat_avg, stat_count) = {
        let sel_start = *selection_start.read();
        let sel_end = *selection_end.read();
        match (sel_start, sel_end) {
            (Some(a), Some(b)) => {
                let cells_ref = cells.read();
                let min_row = a.0.min(b.0);
                let max_row = a.0.max(b.0);
                let min_col = a.1.min(b.1);
                let max_col = a.1.max(b.1);
                let mut sum: f64 = 0.0;
                let mut numeric_count: u32 = 0;
                let mut non_empty: u32 = 0;
                for r in min_row..=max_row {
                    for c in min_col..=max_col {
                        if let Some(val) = cells_ref.get(&(sheet_idx, r, c)) {
                            if !val.is_empty() {
                                non_empty += 1;
                                if let Some(n) = evaluated_numeric(val, sheet_idx, &cells_ref) {
                                    sum += n;
                                    numeric_count += 1;
                                }
                            }
                        }
                    }
                }
                let avg = if numeric_count > 0 {
                    sum / numeric_count as f64
                } else {
                    0.0
                };
                (sum, avg, non_empty)
            }
            _ => (0.0, 0.0, 0),
        }
    };

    rsx! {
        Navbar {}
        {
            let state = *conn_state.read();
            match state {
                ConnectionState::Reconnecting(n) => rsx! {
                    div { class: "ws-banner ws-reconnecting",
                        "Connection lost — reconnecting (attempt {n})…"
                    }
                },
                ConnectionState::Failed => rsx! {
                    div { class: "ws-banner ws-failed",
                        "Could not reconnect. Refresh the page."
                    }
                },
                _ => rsx! {},
            }
        }
        div {
            class: "editor-container",
            tabindex: "0",

            // ── Keyboard navigation ──────────────────────────────────────
            onkeydown: {
                let mut schedule_save = schedule_save.clone();
                let mut flush_save = flush_save.clone();
                let mut commit_formula_bar = commit_formula_bar.clone();
                let mut push_snapshot = push_snapshot.clone();
                move |evt: KeyboardEvent| {
                let key = evt.key();
                let modifiers = evt.modifiers();
                let (mut r, mut c) = *active_cell.read();

                match key {
                    // Arrow keys — move active cell (only when not editing)
                    Key::ArrowUp if !*is_editing.read() => {
                        evt.prevent_default();
                        if r > 0 { r -= 1; }
                    }
                    Key::ArrowDown if !*is_editing.read() => {
                        evt.prevent_default();
                        if r + 1 < NUM_ROWS { r += 1; }
                    }
                    Key::ArrowLeft if !*is_editing.read() => {
                        evt.prevent_default();
                        if c > 0 { c -= 1; }
                    }
                    Key::ArrowRight if !*is_editing.read() => {
                        evt.prevent_default();
                        if c + 1 < NUM_COLS { c += 1; }
                    }

                    // Tab / Shift+Tab
                    Key::Tab => {
                        evt.prevent_default();
                        if *is_editing.read() {
                            commit_formula_bar();
                        }
                        if modifiers.contains(Modifiers::SHIFT) {
                            if c > 0 { c -= 1; }
                        } else if c + 1 < NUM_COLS {
                            c += 1;
                        }
                    }

                    // Enter — confirm edit, move down
                    Key::Enter if *is_editing.read() => {
                        evt.prevent_default();
                        commit_formula_bar();
                        if r + 1 < NUM_ROWS { r += 1; }
                    }

                    // Escape — cancel edit
                    Key::Escape => {
                        // Restore original value
                        let si = *active_sheet.read();
                        let orig = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                        formula_bar.set(orig);
                        is_editing.set(false);
                        fill_mode.set(None);
                    }

                    // Delete / Backspace — clear cell
                    Key::Delete | Key::Backspace if !*is_editing.read() => {
                        evt.prevent_default();
                        let si = *active_sheet.read();
                        if cells.read().contains_key(&(si, r, c)) {
                            if *loaded.read() {
                                push_snapshot();
                            }
                            cells.write().remove(&(si, r, c));
                            formula_bar.set(String::new());
                            schedule_save();
                        } else {
                            formula_bar.set(String::new());
                        }
                    }

                    // Ctrl+S — force-flush save
                    Key::Character(ref ch) if ch == "s" && modifiers.contains(Modifiers::CONTROL) => {
                        evt.prevent_default();
                        flush_save();
                    }

                    // Ctrl+F — open find dialog
                    Key::Character(ref ch) if ch == "f" && modifiers.contains(Modifiers::CONTROL) => {
                        evt.prevent_default();
                        find_open.set(true);
                    }

                    // Ctrl+Z — undo
                    Key::Character(ref ch)
                        if ch == "z"
                            && modifiers.contains(Modifiers::CONTROL)
                            && !modifiers.contains(Modifiers::SHIFT) =>
                    {
                        evt.prevent_default();
                        let snap_opt = undo_stack.write().pop();
                        if let Some(snap) = snap_opt {
                            let current = CalcSnapshot {
                                cells: cells.read().clone(),
                                styles: cell_styles.read().clone(),
                                sheets: sheets.read().clone(),
                            };
                            redo_stack.write().push(current);
                            cells.set(snap.cells);
                            cell_styles.set(snap.styles);
                            sheets.set(snap.sheets);
                            saved_status.set("Unsaved changes".to_string());
                            schedule_save();
                        }
                    }

                    // Ctrl+Y / Ctrl+Shift+Z — redo
                    Key::Character(ref ch)
                        if (ch == "y" && modifiers.contains(Modifiers::CONTROL))
                            || (ch == "z"
                                && modifiers.contains(Modifiers::CONTROL)
                                && modifiers.contains(Modifiers::SHIFT)) =>
                    {
                        evt.prevent_default();
                        let snap_opt = redo_stack.write().pop();
                        if let Some(snap) = snap_opt {
                            let current = CalcSnapshot {
                                cells: cells.read().clone(),
                                styles: cell_styles.read().clone(),
                                sheets: sheets.read().clone(),
                            };
                            undo_stack.write().push(current);
                            cells.set(snap.cells);
                            cell_styles.set(snap.styles);
                            sheets.set(snap.sheets);
                            saved_status.set("Unsaved changes".to_string());
                            schedule_save();
                        }
                    }

                    // Any printable character starts an edit by seeding the formula bar
                    Key::Character(ref ch)
                        if !*is_editing.read()
                            && !modifiers.contains(Modifiers::CONTROL)
                            && !modifiers.contains(Modifiers::META)
                            && !modifiers.contains(Modifiers::ALT) =>
                    {
                        evt.prevent_default();
                        formula_bar.set(ch.clone());
                        is_editing.set(true);
                        saved_status.set("Unsaved changes".to_string());
                        focus_formula_input();
                    }

                    // F2 enters edit mode preserving the existing value
                    Key::F2 if !*is_editing.read() => {
                        evt.prevent_default();
                        is_editing.set(true);
                        focus_formula_input();
                    }

                    _ => {}
                }

                // Update active cell if it changed
                if (r, c) != *active_cell.read() {
                    // Commit any pending edit before moving
                    if *is_editing.read() {
                        commit_formula_bar();
                    }
                    active_cell.set((r, c));
                    let si = *active_sheet.read();
                    let val = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                    formula_bar.set(val);
                    // Reset selection to single cell
                    selection_start.set(Some((r, c)));
                    selection_end.set(Some((r, c)));
                }
                }
            },

            onpaste: {
                let mut schedule_save = schedule_save.clone();
                let mut push_snapshot = push_snapshot.clone();
                move |evt: Event<ClipboardData>| {
                    use dioxus::web::WebEventExt;
                    let Some(web_evt) = evt.try_as_web_event() else { return; };
                    let Ok(clip_evt) = web_evt.dyn_into::<web_sys::ClipboardEvent>() else { return; };
                    let Some(dt) = clip_evt.clipboard_data() else { return; };
                    let Ok(data) = dt.get_data("text/plain") else { return; };
                    if data.is_empty() { return; }
                    clip_evt.prevent_default();
                    let (start_r, start_c) = *active_cell.read();
                    let si = *active_sheet.read();
                    if *loaded.read() { push_snapshot(); }
                    {
                        let mut writer = cells.write();
                        for (i, line) in data.lines().enumerate() {
                            let r = start_r + i as u32;
                            if r >= NUM_ROWS { break; }
                            for (j, field) in line.split('\t').enumerate() {
                                let c = start_c + j as u32;
                                if c >= NUM_COLS { break; }
                                if field.is_empty() {
                                    writer.remove(&(si, r, c));
                                } else {
                                    writer.insert((si, r, c), field.to_string());
                                }
                            }
                        }
                    }
                    saved_status.set("Unsaved changes".to_string());
                    schedule_save();
                }
            },

            // Global mouseup to stop drag-select even outside cells
            onmouseup: {
                let mut schedule_save = schedule_save.clone();
                let mut push_snapshot = push_snapshot.clone();
                move |_| {
                is_selecting.set(false);
                let current_fill = *fill_mode.read();
                if let Some(state) = current_fill {
                    let ((min_r, min_c), (max_r, max_c)) = fill_rect(&state);
                    let (osr, osc) = state.origin_start;
                    let (oer, oec) = state.origin_end;
                    let o_min_r = osr.min(oer);
                    let o_max_r = osr.max(oer);
                    let o_min_c = osc.min(oec);
                    let o_max_c = osc.max(oec);
                    let si = *active_sheet.read();
                    let src: Vec<Vec<String>> = (o_min_r..=o_max_r)
                        .map(|r| {
                            (o_min_c..=o_max_c)
                                .map(|c| cells.read().get(&(si, r, c)).cloned().unwrap_or_default())
                                .collect()
                        })
                        .collect();
                    let src_rows = (o_max_r - o_min_r + 1) as usize;
                    let src_cols = (o_max_c - o_min_c + 1) as usize;
                    if *loaded.read() {
                        push_snapshot();
                    }
                    let mut writer = cells.write();
                    for r in min_r..=max_r {
                        for c in min_c..=max_c {
                            if r >= o_min_r && r <= o_max_r && c >= o_min_c && c <= o_max_c {
                                continue;
                            }
                            let sr_i = ((r - o_min_r) as usize) % src_rows;
                            let sc_i = ((c - o_min_c) as usize) % src_cols;
                            let mut val = src[sr_i][sc_i].clone();
                            if src_rows == 1 && src_cols == 1 {
                                if let Ok(n) = val.parse::<f64>() {
                                    let offset = if r != o_min_r {
                                        (r as i64) - (o_min_r as i64)
                                    } else {
                                        (c as i64) - (o_min_c as i64)
                                    };
                                    val = format!("{}", n + offset as f64);
                                }
                            }
                            if val.is_empty() {
                                writer.remove(&(si, r, c));
                            } else {
                                writer.insert((si, r, c), val);
                            }
                        }
                    }
                    drop(writer);
                    selection_start.set(Some((min_r, min_c)));
                    selection_end.set(Some((max_r, max_c)));
                    schedule_save();
                    fill_mode.set(None);
                }
                }
            },

            // ── Title bar ────────────────────────────────────────────────
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: {
                        let mut schedule_save = schedule_save.clone();
                        move |e: FormEvent| {
                            doc_title.set(e.value());
                            schedule_save();
                        }
                    },
                    placeholder: "Untitled Spreadsheet",
                }
                span { class: "save-status", "{saved_status}" }
                if !presence.read().is_empty() {
                    div {
                        class: "presence-list",
                        style: "display: inline-flex; gap: 4px; margin-left: 12px; align-items: center;",
                        for cursor in presence.read().iter() {
                            span {
                                key: "{cursor.user_id}",
                                class: "presence-pill",
                                title: "{cursor.username}",
                                style: "background: {cursor.color}; color: #fff; border-radius: 999px; padding: 2px 8px; font-size: 12px; font-weight: 600; line-height: 1;",
                                {cursor.username.chars().take(2).collect::<String>().to_uppercase()}
                            }
                        }
                    }
                }
            }

            // ── Toolbar ──────────────────────────────────────────────────
            {
                let mut schedule_save_bold = schedule_save.clone();
                let mut schedule_save_italic = schedule_save.clone();
                let mut schedule_save_under = schedule_save.clone();
                let mut schedule_save_curr = schedule_save.clone();
                let mut schedule_save_pct = schedule_save.clone();
                let mut schedule_save_d0 = schedule_save.clone();
                let mut schedule_save_d2 = schedule_save.clone();
                let mut push_snapshot_tg = push_snapshot.clone();
                let apply_toggle = move |which: u8| {
                    if *loaded.read() {
                        push_snapshot_tg();
                    }
                    let active = *active_cell.read();
                    let sel_s = *selection_start.read();
                    let sel_e = *selection_end.read();
                    let extra = extra_selection.read().clone();
                    let targets = current_selection_cells(active, sel_s, sel_e, &extra);
                    let si = *active_sheet.read();
                    let all_on = targets.iter().all(|(r, c)| {
                        cell_styles
                            .read()
                            .get(&(si, *r, *c))
                            .map(|s| match which {
                                0 => s.bold,
                                1 => s.italic,
                                _ => s.underline,
                            })
                            .unwrap_or(false)
                    });
                    let mut w = cell_styles.write();
                    for (r, c) in targets {
                        let entry = w.entry((si, r, c)).or_default();
                        match which {
                            0 => entry.bold = !all_on,
                            1 => entry.italic = !all_on,
                            _ => entry.underline = !all_on,
                        }
                        if entry == &CellStyle::default() {
                            w.remove(&(si, r, c));
                        }
                    }
                };
                let mut push_snapshot_fmt = push_snapshot.clone();
                let apply_format = move |fmt: &'static str| {
                    if *loaded.read() {
                        push_snapshot_fmt();
                    }
                    let active = *active_cell.read();
                    let sel_s = *selection_start.read();
                    let sel_e = *selection_end.read();
                    let extra = extra_selection.read().clone();
                    let targets = current_selection_cells(active, sel_s, sel_e, &extra);
                    let si = *active_sheet.read();
                    let mut w = cell_styles.write();
                    for (r, c) in targets {
                        let entry = w.entry((si, r, c)).or_default();
                        entry.number_format = fmt.to_string();
                        if entry == &CellStyle::default() {
                            w.remove(&(si, r, c));
                        }
                    }
                };
                let apply_toggle_b = apply_toggle.clone();
                let apply_toggle_i = apply_toggle.clone();
                let apply_toggle_u = apply_toggle.clone();
                let apply_format_c = apply_format.clone();
                let apply_format_p = apply_format.clone();
                let apply_format_d0 = apply_format.clone();
                let apply_format_d2 = apply_format.clone();
                rsx! {
                    div { class: "toolbar",
                        div { class: "toolbar-group",
                            select { class: "toolbar-select font-family",
                                option { "Arial" }
                                option { "Times New Roman" }
                                option { "Courier New" }
                            }
                            select { class: "toolbar-select font-size",
                                option { value: "10", "10" }
                                option { value: "11", selected: true, "11" }
                                option { value: "12", "12" }
                                option { value: "14", "14" }
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn",
                                title: "Bold",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_b.clone();
                                    f(0);
                                    schedule_save_bold();
                                },
                                "B"
                            }
                            button {
                                class: "toolbar-btn italic",
                                title: "Italic",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_i.clone();
                                    f(1);
                                    schedule_save_italic();
                                },
                                "I"
                            }
                            button {
                                class: "toolbar-btn underline",
                                title: "Underline",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_toggle_u.clone();
                                    f(2);
                                    schedule_save_under();
                                },
                                "U"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn",
                                title: "Currency",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_c.clone();
                                    f("currency");
                                    schedule_save_curr();
                                },
                                "$"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Percent",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_p.clone();
                                    f("percent");
                                    schedule_save_pct();
                                },
                                "%"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Decrease Decimal",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_d0.clone();
                                    f("decimal0");
                                    schedule_save_d0();
                                },
                                ".0"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "Increase Decimal",
                                onmousedown: move |evt: MouseEvent| evt.prevent_default(),
                                onclick: move |_| {
                                    let mut f = apply_format_d2.clone();
                                    f("decimal2");
                                    schedule_save_d2();
                                },
                                ".00"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Merge Cells", "Merge" }
                            button { class: "toolbar-btn", title: "Cell Borders", "Borders" }
                            input { r#type: "color", class: "toolbar-color", title: "Fill Color", value: "#ffffff" }
                            input { r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000" }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Align Left", "L" }
                            button { class: "toolbar-btn", title: "Align Center", "C" }
                            button { class: "toolbar-btn", title: "Align Right", "R" }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button { class: "toolbar-btn", title: "Sort A-Z", "Sort" }
                            button { class: "toolbar-btn", title: "Filter", "Filter" }
                            button { class: "toolbar-btn", title: "Insert Function", "f(x)" }
                            button { class: "toolbar-btn", title: "Insert Chart", "Chart" }
                        }
                    }
                }
            }

            // ── Formula bar ──────────────────────────────────────────────
            div { class: "formula-bar",
                input {
                    class: if *address_error.read() { "cell-ref-input cell-ref-error" } else { "cell-ref-input" },
                    value: "{address_input}",
                    oninput: move |e: FormEvent| {
                        address_input.set(e.value());
                        address_error.set(false);
                    },
                    onkeydown: move |evt: KeyboardEvent| {
                        if evt.key() == Key::Enter {
                            evt.prevent_default();
                            let s = address_input.read().clone();
                            match parse_cell_ref(&s) {
                                Some((r, c)) if r < NUM_ROWS && c < NUM_COLS => {
                                    active_cell.set((r, c));
                                    selection_start.set(Some((r, c)));
                                    selection_end.set(Some((r, c)));
                                    let si = *active_sheet.read();
                                    let val = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                                    formula_bar.set(val);
                                    address_input.set(cell_ref(r, c));
                                    address_error.set(false);
                                    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                                        if let Some(el) = doc.query_selector(".cell.active").ok().flatten() {
                                            if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
                                                html_el.scroll_into_view_with_bool(false);
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    address_error.set(true);
                                    let (r, c) = *active_cell.read();
                                    let buf = cell_ref(r, c);
                                    let mut input_handle = address_input;
                                    let mut err_handle = address_error;
                                    Timeout::new(900, move || {
                                        input_handle.set(buf);
                                        err_handle.set(false);
                                    }).forget();
                                }
                            }
                        } else if evt.key() == Key::Escape {
                            let (r, c) = *active_cell.read();
                            address_input.set(cell_ref(r, c));
                            address_error.set(false);
                        }
                    },
                }
                button {
                    class: "formula-icon",
                    r#type: "button",
                    title: "Insert formula",
                    onclick: move |_| {
                        let current = formula_bar.read().clone();
                        if !current.starts_with('=') {
                            formula_bar.set(format!("={current}"));
                        }
                        is_editing.set(true);
                        focus_formula_input();
                    },
                    "f(x)"
                }
                input {
                    class: "formula-input",
                    id: FORMULA_INPUT_ID,
                    value: "{formula_bar}",
                    placeholder: "Enter value or formula...",
                    oninput: move |e| {
                        formula_bar.set(e.value());
                        is_editing.set(true);
                    },
                    onkeydown: {
                        let mut commit_formula_bar = commit_formula_bar.clone();
                        move |evt: KeyboardEvent| {
                            if evt.key() == Key::Enter {
                                evt.prevent_default();
                                commit_formula_bar();
                                // Move active cell down
                                let (r, c) = *active_cell.read();
                                if r + 1 < NUM_ROWS {
                                    active_cell.set((r + 1, c));
                                    let si = *active_sheet.read();
                                    let val = cells.read().get(&(si, r + 1, c)).cloned().unwrap_or_default();
                                    formula_bar.set(val);
                                }
                            } else if evt.key() == Key::Escape {
                                let (r, c) = *active_cell.read();
                                let si = *active_sheet.read();
                                let orig = cells.read().get(&(si, r, c)).cloned().unwrap_or_default();
                                formula_bar.set(orig);
                                is_editing.set(false);
                            }
                        }
                    },
                }
            }

            // ── Loading / error overlay ─────────────────────────────────
            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(_)) => rsx! {},
            }

            // ── Spreadsheet grid ─────────────────────────────────────────
            div { class: "spreadsheet",
                table { class: "sheet-grid",
                    thead {
                        tr {
                            th { class: "row-header corner-cell" }
                            for col in 0..NUM_COLS {
                                th {
                                    class: if active_col == col { "col-header active-header" } else { "col-header" },
                                    oncontextmenu: move |evt: MouseEvent| {
                                        evt.prevent_default();
                                        let coords = evt.client_coordinates();
                                        header_menu.set(Some((HeaderMenuKind::Col, col, coords.x, coords.y)));
                                    },
                                    "{col_letter(col)}"
                                }
                            }
                        }
                    }
                    tbody {
                        for row in 0..NUM_ROWS {
                            tr {
                                th {
                                    class: if active_row == row { "row-header active-header" } else { "row-header" },
                                    oncontextmenu: move |evt: MouseEvent| {
                                        evt.prevent_default();
                                        let coords = evt.client_coordinates();
                                        header_menu.set(Some((HeaderMenuKind::Row, row, coords.x, coords.y)));
                                    },
                                    "{row + 1}"
                                }
                                for col in 0..NUM_COLS {
                                    {
                                        let is_active = (row, col) == (active_row, active_col);
                                        let sel_s = *selection_start.read();
                                        let sel_e = *selection_end.read();
                                        let is_selected = match (sel_s, sel_e) {
                                            (Some(a), Some(b)) => in_selection((row, col), a, b),
                                            (Some(a), None) => (row, col) == a,
                                            _ => false,
                                        } || extra_selection.read().contains(&(row, col));
                                        let fill_preview = match *fill_mode.read() {
                                            Some(state) => {
                                                let (a, b) = fill_rect(&state);
                                                in_selection((row, col), a, b)
                                            }
                                            None => false,
                                        };

                                        let editing = is_active && *is_editing.read();
                                        let base_class = match (is_active, is_selected, fill_preview, editing) {
                                            (_, _, true, _) => "cell fill-preview",
                                            (_, _, _, true) => "cell active editing",
                                            (true, _, _, _) => "cell active",
                                            (false, true, _, _) => "cell selected",
                                            _ => "cell",
                                        };

                                        let si = sheet_idx;
                                        let cell_value = cells.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                        let style = cell_styles.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                        let mut style_classes = String::new();
                                        if style.bold { style_classes.push_str(" cell-bold"); }
                                        if style.italic { style_classes.push_str(" cell-italic"); }
                                        if style.underline { style_classes.push_str(" cell-underline"); }
                                        let class_str = format!("{base_class}{style_classes}");
                                        let evaluated = evaluate_cell_display(&cell_value, si, &cells.read());
                                        let display = if style.number_format.is_empty() || style.number_format == "general" {
                                            evaluated
                                        } else {
                                            format_number(&evaluated, &style.number_format)
                                        };
                                        let mut commit_formula_bar = commit_formula_bar.clone();

                                        rsx! {
                                            td {
                                                class: "{class_str}",

                                                onmousedown: move |evt: MouseEvent| {
                                                    if *is_editing.read() {
                                                        commit_formula_bar();
                                                    }
                                                    let modifiers = evt.modifiers();
                                                    let ctrl_like = modifiers.contains(Modifiers::CONTROL) || modifiers.contains(Modifiers::META);
                                                    let shift = modifiers.contains(Modifiers::SHIFT);

                                                    if shift {
                                                        if selection_start.read().is_none() {
                                                            selection_start.set(Some((row, col)));
                                                        }
                                                        selection_end.set(Some((row, col)));
                                                        active_cell.set((row, col));
                                                        is_selecting.set(false);
                                                    } else if ctrl_like {
                                                        let cell = (row, col);
                                                        let mut extra = extra_selection.write();
                                                        if extra.contains(&cell) {
                                                            extra.retain(|&c| c != cell);
                                                        } else {
                                                            extra.push(cell);
                                                        }
                                                        active_cell.set(cell);
                                                        is_selecting.set(false);
                                                    } else {
                                                        active_cell.set((row, col));
                                                        selection_start.set(Some((row, col)));
                                                        selection_end.set(Some((row, col)));
                                                        extra_selection.write().clear();
                                                        is_selecting.set(true);
                                                    }

                                                    let si = *active_sheet.read();
                                                    let val = cells.read().get(&(si, row, col)).cloned().unwrap_or_default();
                                                    formula_bar.set(val);
                                                },

                                                onmouseover: move |_| {
                                                    if *is_selecting.read() {
                                                        selection_end.set(Some((row, col)));
                                                    } else {
                                                        let current = *fill_mode.read();
                                                        if let Some(mut state) = current {
                                                            state.target = (row, col);
                                                            fill_mode.set(Some(state));
                                                        }
                                                    }
                                                },

                                                onmouseup: move |_| {
                                                    is_selecting.set(false);
                                                },

                                                ondoubleclick: move |_| {
                                                    is_editing.set(true);
                                                    focus_formula_input();
                                                },

                                                // Cell display value
                                                "{display}"

                                                if is_active {
                                                    div {
                                                        class: "autofill-handle",
                                                        onmousedown: move |evt: MouseEvent| {
                                                            evt.stop_propagation();
                                                            let sel_s = selection_start.read().unwrap_or((row, col));
                                                            let sel_e = selection_end.read().unwrap_or(sel_s);
                                                            fill_mode.set(Some(FillState {
                                                                origin_start: sel_s,
                                                                origin_end: sel_e,
                                                                target: (row, col),
                                                            }));
                                                            is_selecting.set(false);
                                                        },
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Sheet tabs ───────────────────────────────────────────────
            div { class: "sheet-tabs",
                for (idx, name) in sheets.read().iter().enumerate() {
                    {
                        let is_active_tab = idx == *active_sheet.read();
                        let is_renaming = *renaming_sheet.read() == Some(idx);
                        let tab_class = match (is_active_tab, is_renaming) {
                            (true, true) => "sheet-tab active rename-mode",
                            (true, false) => "sheet-tab active",
                            (false, true) => "sheet-tab rename-mode",
                            (false, false) => "sheet-tab",
                        };
                        let tab_name = name.clone();
                        let mut commit_formula_bar = commit_formula_bar.clone();
                        let mut schedule_save_tab = schedule_save.clone();
                        let mut push_snapshot_tab = push_snapshot.clone();
                        rsx! {
                            div {
                                class: "{tab_class}",
                                oncontextmenu: move |evt: MouseEvent| {
                                    evt.prevent_default();
                                    let coords = evt.client_coordinates();
                                    sheet_menu.set(Some((idx, coords.x, coords.y)));
                                },
                                ondoubleclick: {
                                    let nm = tab_name.clone();
                                    move |_| {
                                        rename_buffer.set(nm.clone());
                                        renaming_sheet.set(Some(idx));
                                    }
                                },
                                onclick: move |_| {
                                    if *renaming_sheet.read() == Some(idx) { return; }
                                    if *is_editing.read() {
                                        commit_formula_bar();
                                    }
                                    active_sheet.set(idx);
                                    let (r, c) = *active_cell.read();
                                    let val = cells.read().get(&(idx, r, c)).cloned().unwrap_or_default();
                                    formula_bar.set(val);
                                },
                                if is_renaming {
                                    input {
                                        value: "{rename_buffer}",
                                        autofocus: true,
                                        oninput: move |e: FormEvent| rename_buffer.set(e.value()),
                                        onkeydown: move |evt: KeyboardEvent| {
                                            if evt.key() == Key::Enter {
                                                evt.prevent_default();
                                                let new_name = rename_buffer.read().trim().to_string();
                                                if !new_name.is_empty() {
                                                    if *loaded.read() { push_snapshot_tab(); }
                                                    sheets.write()[idx] = new_name;
                                                    schedule_save_tab();
                                                }
                                                renaming_sheet.set(None);
                                            } else if evt.key() == Key::Escape {
                                                renaming_sheet.set(None);
                                            }
                                        },
                                        onblur: move |_| {
                                            renaming_sheet.set(None);
                                        },
                                    }
                                } else {
                                    "{tab_name}"
                                }
                            }
                        }
                    }
                }
                button {
                    class: "sheet-tab-add",
                    title: "Add sheet",
                    onclick: move |_| {
                        let count = sheets.read().len() + 1;
                        sheets.write().push(format!("Sheet{count}"));
                        let new_idx = sheets.read().len() - 1;
                        active_sheet.set(new_idx);
                        let (r, c) = *active_cell.read();
                        let val = cells.read().get(&(new_idx, r, c)).cloned().unwrap_or_default();
                        formula_bar.set(val);
                    },
                    "+"
                }
            }

            // ── Status bar ───────────────────────────────────────────────
            div { class: "status-bar",
                span { "Cell: {cell_ref(active_row, active_col)}" }
                span { "Sum: {stat_sum:.4}" }
                span { "Average: {stat_avg:.4}" }
                span { "Count: {stat_count}" }
            }

            if let Some((kind, idx, mx, my)) = *header_menu.read() {
                {
                    let mut schedule_save_hm = schedule_save.clone();
                    let mut push_snapshot_hm = push_snapshot.clone();
                    let si = *active_sheet.read();
                    rsx! {
                        div {
                            class: "header-context-menu-backdrop",
                            style: "position: fixed; inset: 0; z-index: 1999;",
                            onclick: move |_| header_menu.set(None),
                            oncontextmenu: move |evt: MouseEvent| {
                                evt.prevent_default();
                                header_menu.set(None);
                            },
                            div {
                                class: "header-context-menu",
                                style: "left: {mx}px; top: {my}px;",
                                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                                match kind {
                                    HeaderMenuKind::Row => rsx! {
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si && *r >= idx {
                                                            if r + 1 < NUM_ROWS {
                                                                new_cells.insert((*s_i, *r + 1, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si && *r >= idx {
                                                            if r + 1 < NUM_ROWS {
                                                                new_styles.insert((*s_i, *r + 1, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Insert row above"
                                        }
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let target = idx + 1;
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si && *r >= target {
                                                            if r + 1 < NUM_ROWS {
                                                                new_cells.insert((*s_i, *r + 1, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si && *r >= target {
                                                            if r + 1 < NUM_ROWS {
                                                                new_styles.insert((*s_i, *r + 1, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Insert row below"
                                        }
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si {
                                                            if *r == idx { continue; }
                                                            if *r > idx {
                                                                new_cells.insert((*s_i, *r - 1, *c), v.clone());
                                                            } else {
                                                                new_cells.insert((*s_i, *r, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si {
                                                            if *r == idx { continue; }
                                                            if *r > idx {
                                                                new_styles.insert((*s_i, *r - 1, *c), v.clone());
                                                            } else {
                                                                new_styles.insert((*s_i, *r, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Delete row"
                                        }
                                    },
                                    HeaderMenuKind::Col => rsx! {
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si && *c >= idx {
                                                            if c + 1 < NUM_COLS {
                                                                new_cells.insert((*s_i, *r, *c + 1), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si && *c >= idx {
                                                            if c + 1 < NUM_COLS {
                                                                new_styles.insert((*s_i, *r, *c + 1), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Insert column left"
                                        }
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let target = idx + 1;
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si && *c >= target {
                                                            if c + 1 < NUM_COLS {
                                                                new_cells.insert((*s_i, *r, *c + 1), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si && *c >= target {
                                                            if c + 1 < NUM_COLS {
                                                                new_styles.insert((*s_i, *r, *c + 1), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Insert column right"
                                        }
                                        button {
                                            onclick: {
                                                let mut s = schedule_save_hm.clone();
                                                let mut p = push_snapshot_hm.clone();
                                                move |_| {
                                                    if *loaded.read() { p(); }
                                                    let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                                    for ((s_i, r, c), v) in cells.read().iter() {
                                                        if *s_i == si {
                                                            if *c == idx { continue; }
                                                            if *c > idx {
                                                                new_cells.insert((*s_i, *r, *c - 1), v.clone());
                                                            } else {
                                                                new_cells.insert((*s_i, *r, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_cells.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cells.set(new_cells);
                                                    let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                                    for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                        if *s_i == si {
                                                            if *c == idx { continue; }
                                                            if *c > idx {
                                                                new_styles.insert((*s_i, *r, *c - 1), v.clone());
                                                            } else {
                                                                new_styles.insert((*s_i, *r, *c), v.clone());
                                                            }
                                                        } else {
                                                            new_styles.insert((*s_i, *r, *c), v.clone());
                                                        }
                                                    }
                                                    cell_styles.set(new_styles);
                                                    header_menu.set(None);
                                                    s();
                                                }
                                            },
                                            "Delete column"
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }

            if let Some((sidx, mx, my)) = *sheet_menu.read() {
                {
                    let mut schedule_save_sm = schedule_save.clone();
                    let mut push_snapshot_sm = push_snapshot.clone();
                    let cur_name = sheets.read().get(sidx).cloned().unwrap_or_default();
                    rsx! {
                        div {
                            class: "header-context-menu-backdrop",
                            style: "position: fixed; inset: 0; z-index: 1999;",
                            onclick: move |_| sheet_menu.set(None),
                            oncontextmenu: move |evt: MouseEvent| {
                                evt.prevent_default();
                                sheet_menu.set(None);
                            },
                            div {
                                class: "header-context-menu",
                                style: "left: {mx}px; top: {my}px;",
                                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                                button {
                                    onclick: {
                                        let nm = cur_name.clone();
                                        move |_| {
                                            rename_buffer.set(nm.clone());
                                            renaming_sheet.set(Some(sidx));
                                            sheet_menu.set(None);
                                        }
                                    },
                                    "Rename"
                                }
                                button {
                                    onclick: {
                                        let mut s = schedule_save_sm.clone();
                                        let mut p = push_snapshot_sm.clone();
                                        move |_| {
                                            if sheets.read().len() <= 1 {
                                                sheet_menu.set(None);
                                                return;
                                            }
                                            if *loaded.read() { p(); }
                                            sheets.write().remove(sidx);
                                            let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                            for ((s_i, r, c), v) in cells.read().iter() {
                                                if *s_i == sidx { continue; }
                                                let ns = if *s_i > sidx { *s_i - 1 } else { *s_i };
                                                new_cells.insert((ns, *r, *c), v.clone());
                                            }
                                            cells.set(new_cells);
                                            let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                            for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                if *s_i == sidx { continue; }
                                                let ns = if *s_i > sidx { *s_i - 1 } else { *s_i };
                                                new_styles.insert((ns, *r, *c), v.clone());
                                            }
                                            cell_styles.set(new_styles);
                                            let cur = *active_sheet.read();
                                            let new_active = if cur >= sheets.read().len() {
                                                sheets.read().len() - 1
                                            } else if cur > sidx {
                                                cur - 1
                                            } else {
                                                cur
                                            };
                                            active_sheet.set(new_active);
                                            sheet_menu.set(None);
                                            s();
                                        }
                                    },
                                    "Delete"
                                }
                                button {
                                    onclick: {
                                        let mut s = schedule_save_sm.clone();
                                        let mut p = push_snapshot_sm.clone();
                                        move |_| {
                                            if sidx == 0 {
                                                sheet_menu.set(None);
                                                return;
                                            }
                                            if *loaded.read() { p(); }
                                            sheets.write().swap(sidx, sidx - 1);
                                            let a = sidx;
                                            let b = sidx - 1;
                                            let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                            for ((s_i, r, c), v) in cells.read().iter() {
                                                let ns = if *s_i == a { b } else if *s_i == b { a } else { *s_i };
                                                new_cells.insert((ns, *r, *c), v.clone());
                                            }
                                            cells.set(new_cells);
                                            let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                            for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                let ns = if *s_i == a { b } else if *s_i == b { a } else { *s_i };
                                                new_styles.insert((ns, *r, *c), v.clone());
                                            }
                                            cell_styles.set(new_styles);
                                            let cur = *active_sheet.read();
                                            if cur == a { active_sheet.set(b); }
                                            else if cur == b { active_sheet.set(a); }
                                            sheet_menu.set(None);
                                            s();
                                        }
                                    },
                                    "Move left"
                                }
                                button {
                                    onclick: {
                                        let mut s = schedule_save_sm.clone();
                                        let mut p = push_snapshot_sm.clone();
                                        move |_| {
                                            if sidx + 1 >= sheets.read().len() {
                                                sheet_menu.set(None);
                                                return;
                                            }
                                            if *loaded.read() { p(); }
                                            sheets.write().swap(sidx, sidx + 1);
                                            let a = sidx;
                                            let b = sidx + 1;
                                            let mut new_cells: HashMap<(usize, u32, u32), String> = HashMap::new();
                                            for ((s_i, r, c), v) in cells.read().iter() {
                                                let ns = if *s_i == a { b } else if *s_i == b { a } else { *s_i };
                                                new_cells.insert((ns, *r, *c), v.clone());
                                            }
                                            cells.set(new_cells);
                                            let mut new_styles: HashMap<(usize, u32, u32), CellStyle> = HashMap::new();
                                            for ((s_i, r, c), v) in cell_styles.read().iter() {
                                                let ns = if *s_i == a { b } else if *s_i == b { a } else { *s_i };
                                                new_styles.insert((ns, *r, *c), v.clone());
                                            }
                                            cell_styles.set(new_styles);
                                            let cur = *active_sheet.read();
                                            if cur == a { active_sheet.set(b); }
                                            else if cur == b { active_sheet.set(a); }
                                            sheet_menu.set(None);
                                            s();
                                        }
                                    },
                                    "Move right"
                                }
                            }
                        }
                    }
                }
            }

            FindDialog {
                open: find_open,
                match_info: find_info.read().clone(),
                on_search: move |q: String| {
                    if q.is_empty() {
                        find_matches.set(Vec::new());
                        find_index.set(0);
                        find_info.set(String::new());
                        return;
                    }
                    let needle = q.to_lowercase();
                    let mut matches: Vec<(usize, u32, u32)> = cells
                        .read()
                        .iter()
                        .filter(|(_, v)| v.to_lowercase().contains(&needle))
                        .map(|(k, _)| *k)
                        .collect();
                    matches.sort_by_key(|(s, r, c)| (*s, *r, *c));
                    let n = matches.len();
                    if n == 0 {
                        find_matches.set(Vec::new());
                        find_index.set(0);
                        find_info.set("No matches".to_string());
                    } else {
                        let (si, r, c) = matches[0];
                        find_matches.set(matches);
                        find_index.set(0);
                        find_info.set(format!("1 of {n}"));
                        active_sheet.set(si);
                        active_cell.set((r, c));
                        formula_bar.set(cells.read().get(&(si, r, c)).cloned().unwrap_or_default());
                        selection_start.set(Some((r, c)));
                        selection_end.set(Some((r, c)));
                    }
                },
                on_next: move |_| {
                    let matches = find_matches.read().clone();
                    let n = matches.len();
                    if n == 0 { return; }
                    let next = (*find_index.read() + 1) % n;
                    find_index.set(next);
                    let (si, r, c) = matches[next];
                    active_sheet.set(si);
                    active_cell.set((r, c));
                    formula_bar.set(cells.read().get(&(si, r, c)).cloned().unwrap_or_default());
                    selection_start.set(Some((r, c)));
                    selection_end.set(Some((r, c)));
                    find_info.set(format!("{} of {}", next + 1, n));
                },
                on_prev: move |_| {
                    let matches = find_matches.read().clone();
                    let n = matches.len();
                    if n == 0 { return; }
                    let cur = *find_index.read();
                    let prev = if cur == 0 { n - 1 } else { cur - 1 };
                    find_index.set(prev);
                    let (si, r, c) = matches[prev];
                    active_sheet.set(si);
                    active_cell.set((r, c));
                    formula_bar.set(cells.read().get(&(si, r, c)).cloned().unwrap_or_default());
                    selection_start.set(Some((r, c)));
                    selection_end.set(Some((r, c)));
                    find_info.set(format!("{} of {}", prev + 1, n));
                },
            }
        }
    }
}

#[cfg(test)]
mod eval_tests {
    use super::eval::{evaluate, EvalError};
    use std::collections::HashMap;

    fn make(items: &[((usize, u32, u32), &str)]) -> HashMap<(usize, u32, u32), String> {
        items.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn plain_arithmetic() {
        let cells = HashMap::new();
        assert_eq!(evaluate("1+2", 0, &cells).unwrap(), 3.0);
    }

    #[test]
    fn precedence_and_parens() {
        let cells = HashMap::new();
        assert_eq!(evaluate("1+2*3", 0, &cells).unwrap(), 7.0);
        assert_eq!(evaluate("(1+2)*3", 0, &cells).unwrap(), 9.0);
    }

    #[test]
    fn cell_ref_addition() {
        let cells = make(&[((0, 0, 0), "5"), ((0, 0, 1), "7")]);
        assert_eq!(evaluate("A1+B1", 0, &cells).unwrap(), 12.0);
    }

    #[test]
    fn sum_range() {
        let cells = make(&[((0, 0, 0), "1"), ((0, 1, 0), "2"), ((0, 2, 0), "3")]);
        assert_eq!(evaluate("SUM(A1:A3)", 0, &cells).unwrap(), 6.0);
    }

    #[test]
    fn avg_range() {
        let cells = make(&[((0, 0, 0), "10"), ((0, 1, 0), "20")]);
        assert_eq!(evaluate("AVG(A1:A2)", 0, &cells).unwrap(), 15.0);
    }

    #[test]
    fn if_truthy_and_falsy() {
        let cells_t = make(&[((0, 0, 0), "1")]);
        assert_eq!(evaluate("IF(A1, 100, 200)", 0, &cells_t).unwrap(), 100.0);
        let cells_f = make(&[((0, 0, 0), "0")]);
        assert_eq!(evaluate("IF(A1, 100, 200)", 0, &cells_f).unwrap(), 200.0);
    }

    #[test]
    fn cycle_detection() {
        let cells = make(&[((0, 0, 0), "=B1"), ((0, 0, 1), "=A1")]);
        let res = evaluate("A1", 0, &cells);
        assert!(matches!(res, Err(EvalError::Cycle)));
    }

    #[test]
    fn min_max_count() {
        let cells = make(&[
            ((0, 0, 0), "3"),
            ((0, 1, 0), "1"),
            ((0, 2, 0), "9"),
            ((0, 3, 0), "hello"),
        ]);
        assert_eq!(evaluate("MIN(A1:A4)", 0, &cells).unwrap(), 0.0);
        assert_eq!(evaluate("MAX(A1:A4)", 0, &cells).unwrap(), 9.0);
        assert_eq!(evaluate("COUNT(A1:A4)", 0, &cells).unwrap(), 3.0);
    }

    #[test]
    fn unary_and_div_by_zero() {
        let cells = HashMap::new();
        assert_eq!(evaluate("-2+5", 0, &cells).unwrap(), 3.0);
        assert!(matches!(evaluate("1/0", 0, &cells), Err(EvalError::DivByZero)));
    }

    #[test]
    fn dollar_absolute_refs_ignored() {
        let cells = make(&[((0, 0, 0), "5")]);
        assert_eq!(evaluate("$A$1+1", 0, &cells).unwrap(), 6.0);
    }
}
