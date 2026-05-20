use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Workbook
// ---------------------------------------------------------------------------

/// A Calc workbook — spreadsheet similar to Excel / Google Sheets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workbook {
    pub id: Uuid,
    pub title: String,
    pub sheets: Vec<Sheet>,
    pub metadata: WorkbookMetadata,
}

/// Workbook-level metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbookMetadata {
    pub author: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

impl Default for WorkbookMetadata {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            author: None,
            created_at: now,
            modified_at: now,
        }
    }
}

impl Workbook {
    /// Create a new workbook with a single default sheet named "Sheet1".
    pub fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            sheets: vec![Sheet::new("Sheet1".to_owned())],
            metadata: WorkbookMetadata::default(),
        }
    }

    /// Add a new sheet and return a mutable reference to it.
    pub fn add_sheet(&mut self, name: String) -> &mut Sheet {
        self.sheets.push(Sheet::new(name));
        self.sheets.last_mut().expect("just pushed")
    }
}

// ---------------------------------------------------------------------------
// Sheet
// ---------------------------------------------------------------------------

/// A single sheet inside a workbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub id: Uuid,
    pub name: String,
    pub cells: HashMap<CellRef, Cell>,
    pub column_widths: HashMap<u32, f32>,
    pub row_heights: HashMap<u32, f32>,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    pub merged_cells: Vec<MergedRange>,
    pub conditional_formats: Vec<ConditionalFormat>,
}

impl Sheet {
    /// Create a new, empty sheet with the given name.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            cells: HashMap::new(),
            column_widths: HashMap::new(),
            row_heights: HashMap::new(),
            frozen_rows: 0,
            frozen_cols: 0,
            merged_cells: Vec::new(),
            conditional_formats: Vec::new(),
        }
    }

    /// Set a cell value at `(row, col)` (both 0-based).
    ///
    /// If the cell already exists its value is replaced; formatting is
    /// preserved.  If the cell does not exist a new one with default
    /// formatting is created.
    pub fn set_cell(&mut self, row: u32, col: u32, value: CellValue) {
        let cell_ref = CellRef::new(row, col);
        let cell = self.cells.entry(cell_ref).or_insert_with(|| Cell {
            value: CellValue::Empty,
            formula: None,
            format: CellFormat::default(),
            comment: None,
        });
        cell.value = value;
    }

    /// Return a reference to the cell at `(row, col)`, if present.
    pub fn get_cell(&self, row: u32, col: u32) -> Option<&Cell> {
        self.cells.get(&CellRef::new(row, col))
    }

    /// Set a formula on the cell at `(row, col)`.
    ///
    /// Creates the cell with [`CellValue::Empty`] if it does not exist yet.
    pub fn set_formula(&mut self, row: u32, col: u32, formula: String) {
        let cell_ref = CellRef::new(row, col);
        let cell = self.cells.entry(cell_ref).or_insert_with(|| Cell {
            value: CellValue::Empty,
            formula: None,
            format: CellFormat::default(),
            comment: None,
        });
        cell.formula = Some(formula);
    }
}

// ---------------------------------------------------------------------------
// Cell reference
// ---------------------------------------------------------------------------

/// A zero-based (row, col) reference to a cell.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// Parse A1-style notation (e.g. `"A1"`, `"B3"`, `"AA12"`).
    ///
    /// Column letters are case-insensitive.  Returns `None` if the string is
    /// not valid A1 notation.
    pub fn from_a1(notation: &str) -> Option<Self> {
        let notation = notation.trim();
        if notation.is_empty() {
            return None;
        }

        // Split into letter prefix and digit suffix.
        let col_end = notation
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(notation.len());

        let col_part = &notation[..col_end];
        let row_part = &notation[col_end..];

        if col_part.is_empty() || row_part.is_empty() {
            return None;
        }

        // Ensure all column chars are ASCII letters.
        if !col_part.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }

        // Column: A=0, B=1, ..., Z=25, AA=26, ...
        let col: u32 = col_part.chars().fold(0u32, |acc, c| {
            acc * 26 + (c.to_ascii_uppercase() as u32 - b'A' as u32 + 1)
        });
        let col = col.checked_sub(1)?; // make 0-based

        // Row: 1-based in A1 notation -> 0-based internally.
        let row: u32 = row_part.parse::<u32>().ok()?.checked_sub(1)?;

        Some(Self { row, col })
    }

    /// Convert to A1-style notation (e.g. `(0, 0) -> "A1"`).
    pub fn to_a1(&self) -> String {
        let mut col_str = String::new();
        let mut c = self.col + 1; // 1-based for conversion
        while c > 0 {
            c -= 1;
            col_str.push((b'A' + (c % 26) as u8) as char);
            c /= 26;
        }
        // The letters were pushed in reverse order.
        let col_str: String = col_str.chars().rev().collect();
        format!("{}{}", col_str, self.row + 1)
    }
}

// ---------------------------------------------------------------------------
// Cell value & formatting
// ---------------------------------------------------------------------------

/// The raw value stored in a cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum CellValue {
    Empty,
    Text(String),
    Number(f64),
    Boolean(bool),
    /// ISO-8601 date string.
    Date(String),
    Error(String),
}

/// A single cell in a sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub value: CellValue,
    pub formula: Option<String>,
    pub format: CellFormat,
    pub comment: Option<String>,
}

/// Visual formatting applied to a cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellFormat {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    pub text_color: Option<String>,
    pub bg_color: Option<String>,
    pub horizontal_align: HAlign,
    pub vertical_align: VAlign,
    pub number_format: Option<String>,
    pub borders: CellBorders,
    pub wrap_text: bool,
}

impl Default for CellFormat {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: None,
            bold: false,
            italic: false,
            text_color: None,
            bg_color: None,
            horizontal_align: HAlign::Left,
            vertical_align: VAlign::Bottom,
            number_format: None,
            borders: CellBorders::default(),
            wrap_text: false,
        }
    }
}

/// Horizontal alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// Vertical alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

// ---------------------------------------------------------------------------
// Borders
// ---------------------------------------------------------------------------

/// The four borders of a cell.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CellBorders {
    pub top: Option<Border>,
    pub right: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
}

/// A single border edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Border {
    pub style: BorderStyle,
    pub color: String,
}

/// Visual style of a border line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BorderStyle {
    Thin,
    Medium,
    Thick,
    Dashed,
    Dotted,
    Double,
}

// ---------------------------------------------------------------------------
// Merged cells & conditional formatting
// ---------------------------------------------------------------------------

/// A rectangular range of merged cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedRange {
    pub start: CellRef,
    pub end: CellRef,
}

/// A conditional formatting rule applied to a range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalFormat {
    pub range_start: CellRef,
    pub range_end: CellRef,
    pub rule: ConditionalRule,
    pub format: CellFormat,
}

/// The predicate for a conditional format rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionalRule {
    GreaterThan(f64),
    LessThan(f64),
    Between(f64, f64),
    EqualTo(String),
    ContainsText(String),
    IsEmpty,
    IsNotEmpty,
}

// ===========================================================================
// Selection
// ===========================================================================

/// A rectangular selection of cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub start: CellRef,
    pub end: CellRef,
}

// ===========================================================================
// Formula engine – tokeniser, parser, evaluator
// ===========================================================================

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;

/// Tokens produced by the formula lexer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    StringLit(String),
    Bool(bool),
    Ident(String),
    CellReference(String),
    /// Operators / punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Ampersand,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    LParen,
    RParen,
    Comma,
    Colon,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Number(n) => write!(f, "{n}"),
            Token::StringLit(s) => write!(f, "\"{s}\""),
            Token::Bool(b) => write!(f, "{}", if *b { "TRUE" } else { "FALSE" }),
            Token::Ident(s) => write!(f, "{s}"),
            Token::CellReference(s) => write!(f, "{s}"),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Caret => write!(f, "^"),
            Token::Ampersand => write!(f, "&"),
            Token::Eq => write!(f, "="),
            Token::Neq => write!(f, "<>"),
            Token::Lt => write!(f, "<"),
            Token::Gt => write!(f, ">"),
            Token::Le => write!(f, "<="),
            Token::Ge => write!(f, ">="),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

/// Tokenise a formula string (the leading `=` should already be stripped).
fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < len {
        let ch = chars[i];
        match ch {
            // whitespace – skip
            ' ' | '\t' | '\r' | '\n' => {
                i += 1;
            }

            // string literal
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < len && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                if i >= len {
                    return Err("Unterminated string literal".into());
                }
                i += 1; // closing quote
                tokens.push(Token::StringLit(s));
            }

            // number – starts with a digit or a decimal point followed by digit
            '0'..='9' | '.'
                if {
                    // only treat '.' as number start if next char is digit
                    ch != '.' || (i + 1 < len && chars[i + 1].is_ascii_digit())
                } =>
            {
                let start = i;
                while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                // handle scientific notation
                if i < len && (chars[i] == 'e' || chars[i] == 'E') {
                    i += 1;
                    if i < len && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    while i < len && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let num_str: String = chars[start..i].iter().collect();
                match num_str.parse::<f64>() {
                    Ok(n) => tokens.push(Token::Number(n)),
                    Err(_) => return Err(format!("Invalid number: {num_str}")),
                }
            }

            // operators and punctuation
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '&' => {
                tokens.push(Token::Ampersand);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
            }
            '<' => {
                if i + 1 < len && chars[i + 1] == '>' {
                    tokens.push(Token::Neq);
                    i += 2;
                } else if i + 1 < len && chars[i + 1] == '=' {
                    tokens.push(Token::Le);
                    i += 2;
                } else {
                    tokens.push(Token::Lt);
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < len && chars[i + 1] == '=' {
                    tokens.push(Token::Ge);
                    i += 2;
                } else {
                    tokens.push(Token::Gt);
                    i += 1;
                }
            }

            // identifier / cell-reference / bool
            _ if ch.is_ascii_alphabetic() || ch == '_' || ch == '$' => {
                let start = i;
                // Consume letters, digits, underscore, dollar sign
                while i < len
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let upper = word.to_ascii_uppercase();

                if upper == "TRUE" {
                    tokens.push(Token::Bool(true));
                } else if upper == "FALSE" {
                    tokens.push(Token::Bool(false));
                } else if is_cell_ref_str(&word) {
                    tokens.push(Token::CellReference(upper));
                } else {
                    tokens.push(Token::Ident(upper));
                }
            }

            _ => {
                return Err(format!("Unexpected character: '{ch}'"));
            }
        }
    }

    Ok(tokens)
}

/// Check if a string looks like a cell reference (e.g. A1, $B$3, AA12).
/// Strips optional `$` signs for absolute references before checking.
fn is_cell_ref_str(s: &str) -> bool {
    let stripped: String = s.chars().filter(|c| *c != '$').collect();
    if stripped.is_empty() {
        return false;
    }
    let first_digit = stripped.find(|c: char| c.is_ascii_digit());
    match first_digit {
        None => false,
        Some(0) => false,
        Some(col_end) => {
            let col_part = &stripped[..col_end];
            let row_part = &stripped[col_end..];
            col_part.chars().all(|c| c.is_ascii_alphabetic())
                && !row_part.is_empty()
                && row_part.chars().all(|c| c.is_ascii_digit())
        }
    }
}

/// Strip `$` signs from cell reference string so `$A$1` becomes `A1`.
fn strip_dollars(s: &str) -> String {
    s.chars().filter(|c| *c != '$').collect()
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Expr {
    Number(f64),
    StringLit(String),
    Bool(bool),
    CellRef(CellRef),
    Range(CellRef, CellRef),
    UnaryMinus(Box<Expr>),
    BinaryOp(Box<Expr>, BinOp, Box<Expr>),
    FunctionCall(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        match self.advance() {
            Some(ref tok) if tok == expected => Ok(()),
            Some(tok) => Err(format!("Expected {expected}, got {tok}")),
            None => Err(format!("Expected {expected}, got end of input")),
        }
    }

    /// Top-level: parse a full expression.
    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_compare()
    }

    /// compare = concat (("=" | "<>" | "<" | ">" | "<=" | ">=") concat)*
    fn parse_compare(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_concat()?;
        loop {
            let op = match self.peek() {
                Some(Token::Eq) => BinOp::Eq,
                Some(Token::Neq) => BinOp::Neq,
                Some(Token::Lt) => BinOp::Lt,
                Some(Token::Gt) => BinOp::Gt,
                Some(Token::Le) => BinOp::Le,
                Some(Token::Ge) => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_concat()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// concat = add (("&") add)*
    fn parse_concat(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        while let Some(Token::Ampersand) = self.peek() {
            self.advance();
            let right = self.parse_add()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::Concat, Box::new(right));
        }
        Ok(left)
    }

    /// add = mul (("+" | "-") mul)*
    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// mul = power (("*" | "/") power)*
    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_power()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.advance();
            let right = self.parse_power()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// power = unary ("^" unary)*
    fn parse_power(&mut self) -> Result<Expr, String> {
        let mut base = self.parse_unary()?;
        while let Some(Token::Caret) = self.peek() {
            self.advance();
            let exp = self.parse_unary()?;
            base = Expr::BinaryOp(Box::new(base), BinOp::Pow, Box::new(exp));
        }
        Ok(base)
    }

    /// unary = "-" unary | primary
    fn parse_unary(&mut self) -> Result<Expr, String> {
        if let Some(Token::Minus) = self.peek() {
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(Expr::UnaryMinus(Box::new(inner)));
        }
        // skip unary +
        if let Some(Token::Plus) = self.peek() {
            self.advance();
            return self.parse_unary();
        }
        self.parse_primary()
    }

    /// primary = number | string | bool | function_call | cell_range | "(" expr ")"
    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.advance();
                Ok(Expr::Number(n))
            }
            Some(Token::StringLit(s)) => {
                self.advance();
                Ok(Expr::StringLit(s))
            }
            Some(Token::Bool(b)) => {
                self.advance();
                Ok(Expr::Bool(b))
            }
            Some(Token::Ident(name)) => {
                // function call: IDENT "(" ... ")"
                self.advance();
                self.expect(&Token::LParen)?;
                let mut args = Vec::new();
                if self.peek() != Some(&Token::RParen) {
                    args.push(self.parse_expr()?);
                    while let Some(Token::Comma) = self.peek() {
                        self.advance();
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect(&Token::RParen)?;
                Ok(Expr::FunctionCall(name, args))
            }
            Some(Token::CellReference(ref_str)) => {
                self.advance();
                let clean = strip_dollars(&ref_str);
                let cell = CellRef::from_a1(&clean)
                    .ok_or_else(|| format!("Invalid cell reference: {ref_str}"))?;
                // Check for range "A1:B3"
                if let Some(Token::Colon) = self.peek() {
                    self.advance();
                    match self.advance() {
                        Some(Token::CellReference(end_str)) => {
                            let end_clean = strip_dollars(&end_str);
                            let end = CellRef::from_a1(&end_clean)
                                .ok_or_else(|| format!("Invalid cell reference: {end_str}"))?;
                            Ok(Expr::Range(cell, end))
                        }
                        other => Err(format!(
                            "Expected cell reference after ':', got {:?}",
                            other
                        )),
                    }
                } else {
                    Ok(Expr::CellRef(cell))
                }
            }
            Some(Token::LParen) => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Some(tok) => Err(format!("Unexpected token: {tok}")),
            None => Err("Unexpected end of expression".into()),
        }
    }
}

/// Parse a formula string into an AST.
fn parse_formula(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err("Empty formula".into());
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!(
            "Unexpected token after expression: {}",
            parser.tokens[parser.pos]
        ));
    }
    Ok(expr)
}

// ---------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------

// Thread-local set used for circular-reference detection during a single
// evaluation pass.
thread_local! {
    static EVAL_STACK: RefCell<HashSet<(u32, u32)>> = RefCell::new(HashSet::new());
}

impl CellValue {
    /// Coerce to f64 for arithmetic.  Text that looks numeric is converted.
    fn as_number(&self) -> Option<f64> {
        match self {
            CellValue::Number(n) => Some(*n),
            CellValue::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            CellValue::Text(s) => s.trim().parse::<f64>().ok(),
            CellValue::Empty => Some(0.0),
            _ => None,
        }
    }

    /// Coerce to a string representation.
    fn as_text(&self) -> String {
        match self {
            CellValue::Number(n) => {
                if *n == (*n as i64 as f64) && n.is_finite() {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            CellValue::Text(s) => s.clone(),
            CellValue::Boolean(b) => {
                if *b {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
            CellValue::Empty => String::new(),
            CellValue::Date(d) => d.clone(),
            CellValue::Error(e) => format!("#ERR: {e}"),
        }
    }

    /// Is this value truthy?
    fn is_truthy(&self) -> bool {
        match self {
            CellValue::Boolean(b) => *b,
            CellValue::Number(n) => *n != 0.0,
            CellValue::Text(s) => !s.is_empty(),
            CellValue::Empty => false,
            _ => false,
        }
    }
}

impl Sheet {
    // -----------------------------------------------------------------------
    // Public formula API
    // -----------------------------------------------------------------------

    /// Evaluate a formula string and return the computed [`CellValue`].
    ///
    /// The formula may optionally start with `=`.  Circular references are
    /// detected and result in `CellValue::Error`.
    pub fn evaluate_formula(&self, formula: &str) -> CellValue {
        let formula = formula.trim();
        let body = if let Some(stripped) = formula.strip_prefix('=') {
            stripped
        } else {
            formula
        };

        if body.is_empty() {
            return CellValue::Empty;
        }

        match parse_formula(body) {
            Ok(expr) => self.eval_expr(&expr),
            Err(e) => CellValue::Error(e),
        }
    }

    /// Recalculate all cells that have formulas, updating their cached values.
    ///
    /// Performs a topological sort based on cell dependencies so that cells
    /// are evaluated in the correct order.
    pub fn recalculate(&mut self) {
        // Collect all cells that have formulas.
        let formula_cells: Vec<(CellRef, String)> = self
            .cells
            .iter()
            .filter_map(|(cr, cell)| cell.formula.as_ref().map(|f| (cr.clone(), f.clone())))
            .collect();

        // Build dependency graph: for each formula cell, determine which other
        // cells it depends on.
        let mut deps: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        let mut all_formula_keys: HashSet<(u32, u32)> = HashSet::new();
        for (cr, formula) in &formula_cells {
            let key = (cr.row, cr.col);
            all_formula_keys.insert(key);
            let body = formula.strip_prefix('=').unwrap_or(formula);
            if let Ok(expr) = parse_formula(body) {
                let mut references = Vec::new();
                collect_cell_refs(&expr, &mut references);
                deps.insert(key, references);
            } else {
                deps.insert(key, Vec::new());
            }
        }

        // Topological sort (Kahn's algorithm).
        let order = topo_sort(&all_formula_keys, &deps);

        // Evaluate in topological order, updating cells.
        for (row, col) in &order {
            let cr = CellRef::new(*row, *col);
            if let Some(cell) = self.cells.get(&cr) {
                if let Some(f) = cell.formula.clone() {
                    let val = self.evaluate_formula(&f);
                    if let Some(cell_mut) = self.cells.get_mut(&cr) {
                        cell_mut.value = val;
                    }
                }
            }
        }

        // Also evaluate formula cells that were not in the topo order
        // (e.g. part of a cycle – mark them as errors).
        let ordered_set: HashSet<(u32, u32)> = order.into_iter().collect();
        for (cr, _formula) in &formula_cells {
            let key = (cr.row, cr.col);
            if !ordered_set.contains(&key) {
                if let Some(cell_mut) = self.cells.get_mut(cr) {
                    cell_mut.value = CellValue::Error("Circular reference detected".into());
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Range helpers
    // -----------------------------------------------------------------------

    /// Expand a range like `A1:C3` into a flat list of [`CellRef`]s (row-major).
    pub fn expand_range(start: &CellRef, end: &CellRef) -> Vec<CellRef> {
        let r_min = start.row.min(end.row);
        let r_max = start.row.max(end.row);
        let c_min = start.col.min(end.col);
        let c_max = start.col.max(end.col);
        let mut refs = Vec::with_capacity(((r_max - r_min + 1) * (c_max - c_min + 1)) as usize);
        for r in r_min..=r_max {
            for c in c_min..=c_max {
                refs.push(CellRef::new(r, c));
            }
        }
        refs
    }

    /// Collect all numeric values in a rectangular range.
    pub fn range_values(&self, start: &CellRef, end: &CellRef) -> Vec<f64> {
        let refs = Self::expand_range(start, end);
        let mut values = Vec::new();
        for cr in &refs {
            if let Some(cell) = self.cells.get(cr) {
                if let Some(n) = cell.value.as_number() {
                    // Skip truly empty cells for aggregation purposes (but
                    // 0.0 from empty is acceptable for SUM, not AVERAGE).
                    if !matches!(cell.value, CellValue::Empty) {
                        values.push(n);
                    }
                }
            }
        }
        values
    }

    /// Collect all [`CellValue`]s in a rectangular range (including Empty).
    fn range_cell_values(&self, start: &CellRef, end: &CellRef) -> Vec<CellValue> {
        let refs = Self::expand_range(start, end);
        refs.iter()
            .map(|cr| {
                self.cells
                    .get(cr)
                    .map(|c| c.value.clone())
                    .unwrap_or(CellValue::Empty)
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Auto-fill
    // -----------------------------------------------------------------------

    /// Auto-fill cells from a source range to a target range.
    ///
    /// Detects patterns:
    /// - **Numbers**: linear increment (delta between successive source values).
    /// - **Formulas**: adjusts relative cell references by the fill offset.
    /// - **Other values**: copied as-is (repeating cyclically).
    ///
    /// `fill_end` indicates the last cell of the fill target.  The fill
    /// direction is inferred: if `fill_end.row > source_end.row` we fill
    /// downward; if `fill_end.col > source_end.col` we fill rightward.
    pub fn auto_fill(&mut self, source_start: CellRef, source_end: CellRef, fill_end: CellRef) {
        let sr_min = source_start.row.min(source_end.row);
        let sr_max = source_start.row.max(source_end.row);
        let sc_min = source_start.col.min(source_end.col);
        let sc_max = source_start.col.max(source_end.col);

        let fill_down = fill_end.row > sr_max;
        let fill_right = fill_end.col > sc_max;

        if fill_down {
            let src_rows = sr_max - sr_min + 1;

            // For each column, precompute the numeric delta from the source
            // range (if the column has a consistent linear pattern).
            let mut col_deltas: HashMap<u32, Option<f64>> = HashMap::new();
            for c in sc_min..=sc_max {
                if src_rows >= 2 {
                    // Collect numbers from source rows for this column.
                    let nums: Vec<Option<f64>> = (sr_min..=sr_max)
                        .map(|r| {
                            self.cells.get(&CellRef::new(r, c)).and_then(|cell| {
                                if let CellValue::Number(n) = cell.value {
                                    Some(n)
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();
                    // Check if all are numbers and have consistent delta.
                    if nums.iter().all(|n| n.is_some()) {
                        let ns: Vec<f64> = nums.into_iter().flatten().collect();
                        let delta = ns[1] - ns[0];
                        let consistent = ns.windows(2).all(|w| (w[1] - w[0] - delta).abs() < 1e-10);
                        if consistent {
                            col_deltas.insert(c, Some(delta));
                        } else {
                            col_deltas.insert(c, None);
                        }
                    } else {
                        col_deltas.insert(c, None);
                    }
                }
            }

            for r in (sr_max + 1)..=fill_end.row {
                let steps = r - sr_min; // how many rows from the start of source
                let src_row = sr_min + ((r - sr_min) % src_rows);
                // For pattern detection, the "cycle" is which full repetition
                // beyond the source we are in.
                for c in sc_min..=sc_max {
                    let src_cr = CellRef::new(src_row, c);
                    if let Some(src_cell) = self.cells.get(&src_cr).cloned() {
                        let mut new_cell = src_cell.clone();
                        if let Some(ref f) = src_cell.formula {
                            let row_delta = r as i64 - src_row as i64;
                            new_cell.formula = Some(adjust_formula_refs(f, row_delta, 0));
                        } else if matches!(src_cell.value, CellValue::Number(_)) {
                            if let Some(Some(delta)) = col_deltas.get(&c) {
                                // Linear pattern: value at source_start + delta * steps
                                let base = self
                                    .cells
                                    .get(&CellRef::new(sr_min, c))
                                    .and_then(|cell| {
                                        if let CellValue::Number(n) = cell.value {
                                            Some(n)
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or(0.0);
                                new_cell.value = CellValue::Number(base + *delta * steps as f64);
                            }
                        }
                        self.cells.insert(CellRef::new(r, c), new_cell);
                    }
                }
            }
        } else if fill_right {
            let src_cols = sc_max - sc_min + 1;

            // Precompute deltas per row.
            let mut row_deltas: HashMap<u32, Option<f64>> = HashMap::new();
            for r in sr_min..=sr_max {
                if src_cols >= 2 {
                    let nums: Vec<Option<f64>> = (sc_min..=sc_max)
                        .map(|c| {
                            self.cells.get(&CellRef::new(r, c)).and_then(|cell| {
                                if let CellValue::Number(n) = cell.value {
                                    Some(n)
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();
                    if nums.iter().all(|n| n.is_some()) {
                        let ns: Vec<f64> = nums.into_iter().flatten().collect();
                        let delta = ns[1] - ns[0];
                        let consistent = ns.windows(2).all(|w| (w[1] - w[0] - delta).abs() < 1e-10);
                        if consistent {
                            row_deltas.insert(r, Some(delta));
                        } else {
                            row_deltas.insert(r, None);
                        }
                    } else {
                        row_deltas.insert(r, None);
                    }
                }
            }

            for c in (sc_max + 1)..=fill_end.col {
                let steps = c - sc_min;
                let src_col = sc_min + ((c - sc_min) % src_cols);
                for r in sr_min..=sr_max {
                    let src_cr = CellRef::new(r, src_col);
                    if let Some(src_cell) = self.cells.get(&src_cr).cloned() {
                        let mut new_cell = src_cell.clone();
                        if let Some(ref f) = src_cell.formula {
                            let col_delta = c as i64 - src_col as i64;
                            new_cell.formula = Some(adjust_formula_refs(f, 0, col_delta));
                        } else if matches!(src_cell.value, CellValue::Number(_)) {
                            if let Some(Some(delta)) = row_deltas.get(&r) {
                                let base = self
                                    .cells
                                    .get(&CellRef::new(r, sc_min))
                                    .and_then(|cell| {
                                        if let CellValue::Number(n) = cell.value {
                                            Some(n)
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or(0.0);
                                new_cell.value = CellValue::Number(base + *delta * steps as f64);
                            }
                        }
                        self.cells.insert(CellRef::new(r, c), new_cell);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Selection & row/col operations
    // -----------------------------------------------------------------------

    /// Sort rows in the given range by the specified column index (0-based).
    pub fn sort_range(
        &mut self,
        range_start: &CellRef,
        range_end: &CellRef,
        sort_col: u32,
        ascending: bool,
    ) {
        let r_min = range_start.row.min(range_end.row);
        let r_max = range_start.row.max(range_end.row);
        let c_min = range_start.col.min(range_end.col);
        let c_max = range_start.col.max(range_end.col);

        // Extract rows as vectors of Option<Cell>.
        let mut rows: Vec<Vec<Option<Cell>>> = Vec::new();
        for r in r_min..=r_max {
            let mut row = Vec::new();
            for c in c_min..=c_max {
                row.push(self.cells.remove(&CellRef::new(r, c)));
            }
            rows.push(row);
        }

        let sort_idx = if sort_col >= c_min && sort_col <= c_max {
            (sort_col - c_min) as usize
        } else {
            0
        };

        rows.sort_by(|a, b| {
            let va = a
                .get(sort_idx)
                .and_then(|c| c.as_ref())
                .map(|c| &c.value)
                .unwrap_or(&CellValue::Empty);
            let vb = b
                .get(sort_idx)
                .and_then(|c| c.as_ref())
                .map(|c| &c.value)
                .unwrap_or(&CellValue::Empty);
            let cmp = compare_cell_values(va, vb);
            if ascending { cmp } else { cmp.reverse() }
        });

        // Write rows back.
        for (ri, row) in rows.into_iter().enumerate() {
            let r = r_min + ri as u32;
            for (ci, cell_opt) in row.into_iter().enumerate() {
                let c = c_min + ci as u32;
                if let Some(cell) = cell_opt {
                    self.cells.insert(CellRef::new(r, c), cell);
                }
            }
        }
    }

    /// Copy cells from source rectangle to destination, adjusting relative
    /// references in formulas by the delta.
    pub fn copy_cells(&mut self, src_start: &CellRef, src_end: &CellRef, dest_start: &CellRef) {
        let sr_min = src_start.row.min(src_end.row);
        let sr_max = src_start.row.max(src_end.row);
        let sc_min = src_start.col.min(src_end.col);
        let sc_max = src_start.col.max(src_end.col);
        let row_delta = dest_start.row as i64 - sr_min as i64;
        let col_delta = dest_start.col as i64 - sc_min as i64;

        // Collect source cells first to avoid borrow conflicts.
        let mut copies: Vec<(CellRef, Cell)> = Vec::new();
        for r in sr_min..=sr_max {
            for c in sc_min..=sc_max {
                let src_cr = CellRef::new(r, c);
                if let Some(cell) = self.cells.get(&src_cr) {
                    let dest_r = (r as i64 + row_delta) as u32;
                    let dest_c = (c as i64 + col_delta) as u32;
                    let mut new_cell = cell.clone();
                    if let Some(ref f) = cell.formula {
                        new_cell.formula = Some(adjust_formula_refs(f, row_delta, col_delta));
                    }
                    copies.push((CellRef::new(dest_r, dest_c), new_cell));
                }
            }
        }
        for (cr, cell) in copies {
            self.cells.insert(cr, cell);
        }
    }

    /// Insert `count` rows at `at_row`, shifting existing rows down.
    pub fn insert_rows(&mut self, at_row: u32, count: u32) {
        // Collect affected cells, sorted by row descending to avoid conflicts.
        let mut to_move: Vec<(CellRef, Cell)> = self
            .cells
            .iter()
            .filter(|(cr, _)| cr.row >= at_row)
            .map(|(cr, cell)| (cr.clone(), cell.clone()))
            .collect();
        to_move.sort_by(|a, b| b.0.row.cmp(&a.0.row));

        for (cr, _) in &to_move {
            self.cells.remove(cr);
        }
        for (cr, cell) in to_move {
            self.cells
                .insert(CellRef::new(cr.row + count, cr.col), cell);
        }
    }

    /// Insert `count` columns at `at_col`, shifting existing columns right.
    pub fn insert_cols(&mut self, at_col: u32, count: u32) {
        let mut to_move: Vec<(CellRef, Cell)> = self
            .cells
            .iter()
            .filter(|(cr, _)| cr.col >= at_col)
            .map(|(cr, cell)| (cr.clone(), cell.clone()))
            .collect();
        to_move.sort_by(|a, b| b.0.col.cmp(&a.0.col));

        for (cr, _) in &to_move {
            self.cells.remove(cr);
        }
        for (cr, cell) in to_move {
            self.cells
                .insert(CellRef::new(cr.row, cr.col + count), cell);
        }
    }

    /// Delete `count` rows starting at `at_row`, shifting remaining rows up.
    pub fn delete_rows(&mut self, at_row: u32, count: u32) {
        // Remove cells in the deleted rows.
        let delete_end = at_row + count;
        self.cells
            .retain(|cr, _| cr.row < at_row || cr.row >= delete_end);

        // Shift rows above down.
        let mut to_move: Vec<(CellRef, Cell)> = self
            .cells
            .iter()
            .filter(|(cr, _)| cr.row >= delete_end)
            .map(|(cr, cell)| (cr.clone(), cell.clone()))
            .collect();
        to_move.sort_by(|a, b| a.0.row.cmp(&b.0.row));

        for (cr, _) in &to_move {
            self.cells.remove(cr);
        }
        for (cr, cell) in to_move {
            self.cells
                .insert(CellRef::new(cr.row - count, cr.col), cell);
        }
    }

    /// Delete `count` columns starting at `at_col`, shifting remaining
    /// columns left.
    pub fn delete_cols(&mut self, at_col: u32, count: u32) {
        let delete_end = at_col + count;
        self.cells
            .retain(|cr, _| cr.col < at_col || cr.col >= delete_end);

        let mut to_move: Vec<(CellRef, Cell)> = self
            .cells
            .iter()
            .filter(|(cr, _)| cr.col >= delete_end)
            .map(|(cr, cell)| (cr.clone(), cell.clone()))
            .collect();
        to_move.sort_by(|a, b| a.0.col.cmp(&b.0.col));

        for (cr, _) in &to_move {
            self.cells.remove(cr);
        }
        for (cr, cell) in to_move {
            self.cells
                .insert(CellRef::new(cr.row, cr.col - count), cell);
        }
    }

    // -----------------------------------------------------------------------
    // Internal evaluation
    // -----------------------------------------------------------------------

    /// Evaluate an AST [`Expr`] using this sheet's cell data.
    fn eval_expr(&self, expr: &Expr) -> CellValue {
        match expr {
            Expr::Number(n) => CellValue::Number(*n),
            Expr::StringLit(s) => CellValue::Text(s.clone()),
            Expr::Bool(b) => CellValue::Boolean(*b),
            Expr::CellRef(cr) => self.resolve_cell(cr),
            Expr::Range(_, _) => CellValue::Error("Range used outside of function".into()),
            Expr::UnaryMinus(inner) => {
                let val = self.eval_expr(inner);
                match val.as_number() {
                    Some(n) => CellValue::Number(-n),
                    None => CellValue::Error("Cannot negate non-number".into()),
                }
            }
            Expr::BinaryOp(lhs, op, rhs) => self.eval_binop(lhs, *op, rhs),
            Expr::FunctionCall(name, args) => self.eval_function(name, args),
        }
    }

    /// Resolve a cell reference, evaluating its formula if needed.
    /// Detects circular references.
    fn resolve_cell(&self, cr: &CellRef) -> CellValue {
        let key = (cr.row, cr.col);

        // Circular reference check.
        let is_circular = EVAL_STACK.with(|stack| {
            let st = stack.borrow();
            st.contains(&key)
        });
        if is_circular {
            return CellValue::Error("Circular reference".into());
        }

        match self.cells.get(cr) {
            None => CellValue::Empty,
            Some(cell) => {
                if let Some(ref formula) = cell.formula {
                    // Push onto eval stack
                    EVAL_STACK.with(|stack| stack.borrow_mut().insert(key));
                    let result = self.evaluate_formula(formula);
                    EVAL_STACK.with(|stack| stack.borrow_mut().remove(&key));
                    result
                } else {
                    cell.value.clone()
                }
            }
        }
    }

    /// Evaluate a binary operation.
    fn eval_binop(&self, lhs: &Expr, op: BinOp, rhs: &Expr) -> CellValue {
        // Concatenation
        if matches!(op, BinOp::Concat) {
            let l = self.eval_expr(lhs);
            let r = self.eval_expr(rhs);
            return CellValue::Text(format!("{}{}", l.as_text(), r.as_text()));
        }

        let l = self.eval_expr(lhs);
        let r = self.eval_expr(rhs);

        // Comparison operators work on numbers or strings
        match op {
            BinOp::Eq => return CellValue::Boolean(cell_values_equal(&l, &r)),
            BinOp::Neq => return CellValue::Boolean(!cell_values_equal(&l, &r)),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                let ord = compare_cell_values(&l, &r);
                let result = match op {
                    BinOp::Lt => ord == std::cmp::Ordering::Less,
                    BinOp::Gt => ord == std::cmp::Ordering::Greater,
                    BinOp::Le => ord != std::cmp::Ordering::Greater,
                    BinOp::Ge => ord != std::cmp::Ordering::Less,
                    _ => unreachable!(),
                };
                return CellValue::Boolean(result);
            }
            _ => {}
        }

        // Arithmetic operators need numeric operands
        let ln = match l.as_number() {
            Some(n) => n,
            None => {
                return CellValue::Error(format!("Left operand is not a number: {}", l.as_text()));
            }
        };
        let rn = match r.as_number() {
            Some(n) => n,
            None => {
                return CellValue::Error(format!("Right operand is not a number: {}", r.as_text()));
            }
        };

        match op {
            BinOp::Add => CellValue::Number(ln + rn),
            BinOp::Sub => CellValue::Number(ln - rn),
            BinOp::Mul => CellValue::Number(ln * rn),
            BinOp::Div => {
                if rn == 0.0 {
                    CellValue::Error("#DIV/0!".into())
                } else {
                    CellValue::Number(ln / rn)
                }
            }
            BinOp::Pow => CellValue::Number(ln.powf(rn)),
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------------
    // Built-in functions
    // -----------------------------------------------------------------------

    fn eval_function(&self, name: &str, args: &[Expr]) -> CellValue {
        match name {
            "SUM" => self.fn_sum(args),
            "AVERAGE" => self.fn_average(args),
            "COUNT" => self.fn_count(args),
            "COUNTA" => self.fn_counta(args),
            "MIN" => self.fn_min(args),
            "MAX" => self.fn_max(args),
            "IF" => self.fn_if(args),
            "ABS" => self.fn_abs(args),
            "ROUND" => self.fn_round(args),
            "CONCATENATE" => self.fn_concatenate(args),
            "LEN" => self.fn_len(args),
            "LEFT" => self.fn_left(args),
            "RIGHT" => self.fn_right(args),
            "MID" => self.fn_mid(args),
            "UPPER" => self.fn_upper(args),
            "LOWER" => self.fn_lower(args),
            "TRIM" => self.fn_trim(args),
            "NOW" => self.fn_now(args),
            "TODAY" => self.fn_today(args),
            "VLOOKUP" => self.fn_vlookup(args),
            "COUNTIF" => self.fn_countif(args),
            "SUMIF" => self.fn_sumif(args),
            _ => CellValue::Error(format!("Unknown function: {name}")),
        }
    }

    /// Collect numeric values from a mixed list of expressions that may
    /// contain scalars or ranges.
    fn collect_numbers(&self, args: &[Expr]) -> Vec<f64> {
        let mut vals = Vec::new();
        for arg in args {
            match arg {
                Expr::Range(start, end) => {
                    vals.extend(self.range_values(start, end));
                }
                _ => {
                    let v = self.eval_expr(arg);
                    if let Some(n) = v.as_number() {
                        if !matches!(v, CellValue::Empty) {
                            vals.push(n);
                        }
                    }
                }
            }
        }
        vals
    }

    /// Collect all CellValues from a mixed list (ranges are expanded).
    fn collect_values(&self, args: &[Expr]) -> Vec<CellValue> {
        let mut vals = Vec::new();
        for arg in args {
            match arg {
                Expr::Range(start, end) => {
                    vals.extend(self.range_cell_values(start, end));
                }
                _ => {
                    vals.push(self.eval_expr(arg));
                }
            }
        }
        vals
    }

    fn fn_sum(&self, args: &[Expr]) -> CellValue {
        let nums = self.collect_numbers(args);
        CellValue::Number(nums.iter().sum())
    }

    fn fn_average(&self, args: &[Expr]) -> CellValue {
        let nums = self.collect_numbers(args);
        if nums.is_empty() {
            CellValue::Error("#DIV/0!".into())
        } else {
            let sum: f64 = nums.iter().sum();
            CellValue::Number(sum / nums.len() as f64)
        }
    }

    fn fn_count(&self, args: &[Expr]) -> CellValue {
        let nums = self.collect_numbers(args);
        CellValue::Number(nums.len() as f64)
    }

    fn fn_counta(&self, args: &[Expr]) -> CellValue {
        let vals = self.collect_values(args);
        let count = vals
            .iter()
            .filter(|v| !matches!(v, CellValue::Empty))
            .count();
        CellValue::Number(count as f64)
    }

    fn fn_min(&self, args: &[Expr]) -> CellValue {
        let nums = self.collect_numbers(args);
        match nums.iter().cloned().reduce(f64::min) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Number(0.0),
        }
    }

    fn fn_max(&self, args: &[Expr]) -> CellValue {
        let nums = self.collect_numbers(args);
        match nums.iter().cloned().reduce(f64::max) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Number(0.0),
        }
    }

    fn fn_if(&self, args: &[Expr]) -> CellValue {
        if args.is_empty() || args.len() > 3 {
            return CellValue::Error("IF requires 1 to 3 arguments".into());
        }
        let cond = self.eval_expr(&args[0]);
        if cond.is_truthy() {
            if args.len() >= 2 {
                self.eval_expr(&args[1])
            } else {
                CellValue::Boolean(true)
            }
        } else if args.len() >= 3 {
            self.eval_expr(&args[2])
        } else {
            CellValue::Boolean(false)
        }
    }

    fn fn_abs(&self, args: &[Expr]) -> CellValue {
        if args.len() != 1 {
            return CellValue::Error("ABS requires 1 argument".into());
        }
        match self.eval_expr(&args[0]).as_number() {
            Some(n) => CellValue::Number(n.abs()),
            None => CellValue::Error("#VALUE!".into()),
        }
    }

    fn fn_round(&self, args: &[Expr]) -> CellValue {
        if args.is_empty() || args.len() > 2 {
            return CellValue::Error("ROUND requires 1 or 2 arguments".into());
        }
        let n = match self.eval_expr(&args[0]).as_number() {
            Some(v) => v,
            None => return CellValue::Error("#VALUE!".into()),
        };
        let digits = if args.len() == 2 {
            match self.eval_expr(&args[1]).as_number() {
                Some(d) => d as i32,
                None => return CellValue::Error("#VALUE!".into()),
            }
        } else {
            0
        };
        let factor = 10f64.powi(digits);
        CellValue::Number((n * factor).round() / factor)
    }

    fn fn_concatenate(&self, args: &[Expr]) -> CellValue {
        let mut result = String::new();
        for arg in args {
            let v = self.eval_expr(arg);
            result.push_str(&v.as_text());
        }
        CellValue::Text(result)
    }

    fn fn_len(&self, args: &[Expr]) -> CellValue {
        if args.len() != 1 {
            return CellValue::Error("LEN requires 1 argument".into());
        }
        let s = self.eval_expr(&args[0]).as_text();
        CellValue::Number(s.len() as f64)
    }

    fn fn_left(&self, args: &[Expr]) -> CellValue {
        if args.is_empty() || args.len() > 2 {
            return CellValue::Error("LEFT requires 1 or 2 arguments".into());
        }
        let s = self.eval_expr(&args[0]).as_text();
        let n = if args.len() == 2 {
            match self.eval_expr(&args[1]).as_number() {
                Some(v) => v.max(0.0) as usize,
                None => return CellValue::Error("#VALUE!".into()),
            }
        } else {
            1
        };
        let result: String = s.chars().take(n).collect();
        CellValue::Text(result)
    }

    fn fn_right(&self, args: &[Expr]) -> CellValue {
        if args.is_empty() || args.len() > 2 {
            return CellValue::Error("RIGHT requires 1 or 2 arguments".into());
        }
        let s = self.eval_expr(&args[0]).as_text();
        let n = if args.len() == 2 {
            match self.eval_expr(&args[1]).as_number() {
                Some(v) => v.max(0.0) as usize,
                None => return CellValue::Error("#VALUE!".into()),
            }
        } else {
            1
        };
        let char_count = s.chars().count();
        let skip = char_count.saturating_sub(n);
        let result: String = s.chars().skip(skip).collect();
        CellValue::Text(result)
    }

    fn fn_mid(&self, args: &[Expr]) -> CellValue {
        if args.len() != 3 {
            return CellValue::Error("MID requires 3 arguments".into());
        }
        let s = self.eval_expr(&args[0]).as_text();
        let start = match self.eval_expr(&args[1]).as_number() {
            Some(v) => (v.max(1.0) as usize).saturating_sub(1), // 1-based
            None => return CellValue::Error("#VALUE!".into()),
        };
        let n = match self.eval_expr(&args[2]).as_number() {
            Some(v) => v.max(0.0) as usize,
            None => return CellValue::Error("#VALUE!".into()),
        };
        let result: String = s.chars().skip(start).take(n).collect();
        CellValue::Text(result)
    }

    fn fn_upper(&self, args: &[Expr]) -> CellValue {
        if args.len() != 1 {
            return CellValue::Error("UPPER requires 1 argument".into());
        }
        CellValue::Text(self.eval_expr(&args[0]).as_text().to_uppercase())
    }

    fn fn_lower(&self, args: &[Expr]) -> CellValue {
        if args.len() != 1 {
            return CellValue::Error("LOWER requires 1 argument".into());
        }
        CellValue::Text(self.eval_expr(&args[0]).as_text().to_lowercase())
    }

    fn fn_trim(&self, args: &[Expr]) -> CellValue {
        if args.len() != 1 {
            return CellValue::Error("TRIM requires 1 argument".into());
        }
        // Excel TRIM removes leading/trailing spaces and collapses internal
        // runs of spaces to a single space.
        let s = self.eval_expr(&args[0]).as_text();
        let trimmed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        CellValue::Text(trimmed)
    }

    fn fn_now(&self, args: &[Expr]) -> CellValue {
        if !args.is_empty() {
            return CellValue::Error("NOW takes no arguments".into());
        }
        let now = chrono::Utc::now();
        CellValue::Date(now.to_rfc3339())
    }

    fn fn_today(&self, args: &[Expr]) -> CellValue {
        if !args.is_empty() {
            return CellValue::Error("TODAY takes no arguments".into());
        }
        let today = chrono::Utc::now().date_naive();
        CellValue::Date(today.to_string())
    }

    fn fn_vlookup(&self, args: &[Expr]) -> CellValue {
        // VLOOKUP(search_key, range, col_index, [is_sorted])
        if args.len() < 3 || args.len() > 4 {
            return CellValue::Error("VLOOKUP requires 3 or 4 arguments".into());
        }
        let search = self.eval_expr(&args[0]);
        let (range_start, range_end) = match &args[1] {
            Expr::Range(s, e) => (s.clone(), e.clone()),
            _ => return CellValue::Error("VLOOKUP 2nd argument must be a range".into()),
        };
        let col_idx = match self.eval_expr(&args[2]).as_number() {
            Some(n) => {
                let idx = n as u32;
                if idx < 1 {
                    return CellValue::Error("VLOOKUP column index must be >= 1".into());
                }
                idx - 1 // 0-based offset from range start col
            }
            None => return CellValue::Error("#VALUE!".into()),
        };
        // is_sorted defaults to true, but we always do exact match when false.
        let _is_sorted = if args.len() == 4 {
            self.eval_expr(&args[3]).is_truthy()
        } else {
            true
        };

        let r_min = range_start.row.min(range_end.row);
        let r_max = range_start.row.max(range_end.row);
        let c_min = range_start.col.min(range_end.col);

        let target_col = c_min + col_idx;

        for r in r_min..=r_max {
            let first_val = self
                .cells
                .get(&CellRef::new(r, c_min))
                .map(|c| &c.value)
                .unwrap_or(&CellValue::Empty);
            if cell_values_equal(first_val, &search) {
                return self
                    .cells
                    .get(&CellRef::new(r, target_col))
                    .map(|c| c.value.clone())
                    .unwrap_or(CellValue::Empty);
            }
        }

        CellValue::Error("#N/A".into())
    }

    fn fn_countif(&self, args: &[Expr]) -> CellValue {
        // COUNTIF(range, criteria)
        if args.len() != 2 {
            return CellValue::Error("COUNTIF requires 2 arguments".into());
        }
        let (start, end) = match &args[0] {
            Expr::Range(s, e) => (s.clone(), e.clone()),
            _ => return CellValue::Error("COUNTIF 1st argument must be a range".into()),
        };
        let criteria = self.eval_expr(&args[1]);
        let values = self.range_cell_values(&start, &end);

        let count = values
            .iter()
            .filter(|v| match_criteria(v, &criteria))
            .count();
        CellValue::Number(count as f64)
    }

    fn fn_sumif(&self, args: &[Expr]) -> CellValue {
        // SUMIF(range, criteria, [sum_range])
        if args.is_empty() || args.len() > 3 {
            return CellValue::Error("SUMIF requires 2 or 3 arguments".into());
        }
        let (start, end) = match &args[0] {
            Expr::Range(s, e) => (s.clone(), e.clone()),
            _ => return CellValue::Error("SUMIF 1st argument must be a range".into()),
        };
        let criteria = self.eval_expr(&args[1]);

        let (sum_start, sum_end) = if args.len() == 3 {
            match &args[2] {
                Expr::Range(s, e) => (s.clone(), e.clone()),
                _ => return CellValue::Error("SUMIF 3rd argument must be a range".into()),
            }
        } else {
            (start.clone(), end.clone())
        };

        let crit_refs = Self::expand_range(&start, &end);
        let sum_refs = Self::expand_range(&sum_start, &sum_end);

        let mut total = 0.0;
        for (i, cr) in crit_refs.iter().enumerate() {
            let val = self
                .cells
                .get(cr)
                .map(|c| c.value.clone())
                .unwrap_or(CellValue::Empty);
            if match_criteria(&val, &criteria) {
                if let Some(scr) = sum_refs.get(i) {
                    if let Some(cell) = self.cells.get(scr) {
                        if let Some(n) = cell.value.as_number() {
                            total += n;
                        }
                    }
                }
            }
        }
        CellValue::Number(total)
    }
}

// ===========================================================================
// Helper functions
// ===========================================================================

/// Simple equality check between two CellValues.
fn cell_values_equal(a: &CellValue, b: &CellValue) -> bool {
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => (x - y).abs() < f64::EPSILON,
        (CellValue::Text(x), CellValue::Text(y)) => {
            x.to_ascii_uppercase() == y.to_ascii_uppercase()
        }
        (CellValue::Boolean(x), CellValue::Boolean(y)) => x == y,
        (CellValue::Empty, CellValue::Empty) => true,
        // Cross-type: number vs text that looks like number
        (CellValue::Number(n), CellValue::Text(s)) | (CellValue::Text(s), CellValue::Number(n)) => {
            s.trim()
                .parse::<f64>()
                .map_or(false, |sn| (sn - n).abs() < f64::EPSILON)
        }
        _ => false,
    }
}

/// Compare two CellValues for sorting purposes.
fn compare_cell_values(a: &CellValue, b: &CellValue) -> std::cmp::Ordering {
    // Empty < Number < Text < Boolean < Error
    fn type_rank(v: &CellValue) -> u8 {
        match v {
            CellValue::Empty => 0,
            CellValue::Number(_) => 1,
            CellValue::Date(_) => 2,
            CellValue::Text(_) => 3,
            CellValue::Boolean(_) => 4,
            CellValue::Error(_) => 5,
        }
    }
    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => {
            x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (CellValue::Text(x), CellValue::Text(y)) => x.cmp(y),
        (CellValue::Boolean(x), CellValue::Boolean(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Match a cell value against a criteria value.
///
/// Criteria can be:
/// - A plain value (exact match)
/// - A string starting with an operator like `">5"`, `"<>hello"`
fn match_criteria(value: &CellValue, criteria: &CellValue) -> bool {
    if let CellValue::Text(s) = criteria {
        let s = s.trim();
        // Check for operator prefix
        if let Some(rest) = s.strip_prefix(">=") {
            if let (Some(vn), Ok(cn)) = (value.as_number(), rest.trim().parse::<f64>()) {
                return vn >= cn;
            }
        } else if let Some(rest) = s.strip_prefix("<=") {
            if let (Some(vn), Ok(cn)) = (value.as_number(), rest.trim().parse::<f64>()) {
                return vn <= cn;
            }
        } else if let Some(rest) = s.strip_prefix("<>") {
            let crit_val = CellValue::Text(rest.trim().to_string());
            return !cell_values_equal(value, &crit_val);
        } else if let Some(rest) = s.strip_prefix('>') {
            if let (Some(vn), Ok(cn)) = (value.as_number(), rest.trim().parse::<f64>()) {
                return vn > cn;
            }
        } else if let Some(rest) = s.strip_prefix('<') {
            if let (Some(vn), Ok(cn)) = (value.as_number(), rest.trim().parse::<f64>()) {
                return vn < cn;
            }
        } else if let Some(rest) = s.strip_prefix('=') {
            let crit_val = if let Ok(n) = rest.trim().parse::<f64>() {
                CellValue::Number(n)
            } else {
                CellValue::Text(rest.trim().to_string())
            };
            return cell_values_equal(value, &crit_val);
        }
    }
    // Default: exact match
    cell_values_equal(value, criteria)
}

/// Collect all cell reference coordinates from an expression tree.
fn collect_cell_refs(expr: &Expr, out: &mut Vec<(u32, u32)>) {
    match expr {
        Expr::CellRef(cr) => out.push((cr.row, cr.col)),
        Expr::Range(start, end) => {
            let refs = Sheet::expand_range(start, end);
            for cr in refs {
                out.push((cr.row, cr.col));
            }
        }
        Expr::UnaryMinus(inner) => collect_cell_refs(inner, out),
        Expr::BinaryOp(l, _, r) => {
            collect_cell_refs(l, out);
            collect_cell_refs(r, out);
        }
        Expr::FunctionCall(_, args) => {
            for a in args {
                collect_cell_refs(a, out);
            }
        }
        _ => {}
    }
}

/// Topological sort of nodes identified by (row, col).
fn topo_sort(
    nodes: &HashSet<(u32, u32)>,
    deps: &HashMap<(u32, u32), Vec<(u32, u32)>>,
) -> Vec<(u32, u32)> {
    // Only consider dependencies that are within our node set.
    let mut in_degree: HashMap<(u32, u32), usize> = HashMap::new();
    let mut dependents: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
    for node in nodes {
        in_degree.entry(*node).or_insert(0);
        if let Some(node_deps) = deps.get(node) {
            for dep in node_deps {
                if nodes.contains(dep) {
                    *in_degree.entry(*node).or_insert(0) += 1;
                    dependents.entry(*dep).or_default().push(*node);
                }
            }
        }
    }

    let mut queue: Vec<(u32, u32)> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(&node, _)| node)
        .collect();
    queue.sort(); // deterministic order

    let mut order = Vec::new();
    while let Some(node) = queue.pop() {
        order.push(node);
        if let Some(deps_of) = dependents.get(&node) {
            for dep in deps_of {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push(*dep);
                    }
                }
            }
        }
    }

    order
}

/// Adjust cell references in a formula string by the given row/col deltas.
/// This is used for auto-fill and copy operations.  Absolute references
/// (`$A$1`) are left unchanged.
fn adjust_formula_refs(formula: &str, row_delta: i64, col_delta: i64) -> String {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let had_eq = formula.starts_with('=');

    let chars: Vec<char> = body.chars().collect();
    let len = chars.len();
    let mut result = String::new();
    let mut i = 0;

    while i < len {
        // Check for string literal – skip it.
        if chars[i] == '"' {
            result.push('"');
            i += 1;
            while i < len && chars[i] != '"' {
                result.push(chars[i]);
                i += 1;
            }
            if i < len {
                result.push('"');
                i += 1;
            }
            continue;
        }

        // Look for a cell reference pattern: optional $ + letters + optional $ + digits
        if chars[i] == '$' || chars[i].is_ascii_alphabetic() {
            let start = i;
            let mut col_absolute = false;
            let mut row_absolute = false;

            if i < len && chars[i] == '$' {
                col_absolute = true;
                i += 1;
            }
            let col_start = i;
            while i < len && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let col_end = i;
            if col_end == col_start {
                // No letters found – not a cell ref.
                result.push_str(&chars[start..i].iter().collect::<String>());
                continue;
            }

            if i < len && chars[i] == '$' {
                row_absolute = true;
                i += 1;
            }
            let row_start = i;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            let row_end = i;
            if row_end == row_start {
                // No digits – not a cell ref. Could be a function name.
                // Check if next char is a letter/digit/underscore which would
                // mean this is part of a longer identifier.
                while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                result.push_str(&chars[start..i].iter().collect::<String>());
                continue;
            }

            // We have a cell ref.
            let col_str: String = chars[col_start..col_end].iter().collect();
            let row_str: String = chars[row_start..row_end].iter().collect();

            let col_num: u32 = col_str
                .chars()
                .fold(0u32, |acc, c| {
                    acc * 26 + (c.to_ascii_uppercase() as u32 - b'A' as u32 + 1)
                })
                .saturating_sub(1);
            let row_num: u32 = row_str.parse::<u32>().unwrap_or(1).saturating_sub(1);

            let new_col = if col_absolute {
                col_num
            } else {
                (col_num as i64 + col_delta).max(0) as u32
            };
            let new_row = if row_absolute {
                row_num
            } else {
                (row_num as i64 + row_delta).max(0) as u32
            };

            let new_ref = CellRef::new(new_row, new_col);
            if col_absolute {
                result.push('$');
            }
            // Build column letters from new_col.
            let a1 = new_ref.to_a1();
            let a1_col_end = a1.find(|c: char| c.is_ascii_digit()).unwrap_or(a1.len());
            result.push_str(&a1[..a1_col_end]);
            if row_absolute {
                result.push('$');
            }
            result.push_str(&a1[a1_col_end..]);
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    if had_eq { format!("={result}") } else { result }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a sheet and set some cells for testing.
    fn make_test_sheet() -> Sheet {
        let mut sheet = Sheet::new("Test".into());
        // A1=1, A2=2, A3=3
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(1, 0, CellValue::Number(2.0));
        sheet.set_cell(2, 0, CellValue::Number(3.0));
        // B1=10, B2=20, B3=30
        sheet.set_cell(0, 1, CellValue::Number(10.0));
        sheet.set_cell(1, 1, CellValue::Number(20.0));
        sheet.set_cell(2, 1, CellValue::Number(30.0));
        // C1="hello"
        sheet.set_cell(0, 2, CellValue::Text("hello".into()));
        sheet
    }

    // -----------------------------------------------------------------------
    // Cell reference parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_cell_ref_a1() {
        let cr = CellRef::from_a1("A1").unwrap();
        assert_eq!(cr.row, 0);
        assert_eq!(cr.col, 0);

        let cr2 = CellRef::from_a1("B3").unwrap();
        assert_eq!(cr2.row, 2);
        assert_eq!(cr2.col, 1);

        let cr3 = CellRef::from_a1("AA12").unwrap();
        assert_eq!(cr3.row, 11);
        assert_eq!(cr3.col, 26);
    }

    #[test]
    fn test_cell_ref_roundtrip() {
        for (r, c) in [(0, 0), (0, 25), (0, 26), (5, 100)] {
            let cr = CellRef::new(r, c);
            let a1 = cr.to_a1();
            let cr2 = CellRef::from_a1(&a1).unwrap();
            assert_eq!(cr.row, cr2.row, "row mismatch for {a1}");
            assert_eq!(cr.col, cr2.col, "col mismatch for {a1}");
        }
    }

    // -----------------------------------------------------------------------
    // Tokeniser
    // -----------------------------------------------------------------------

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("1+2*3").unwrap();
        assert_eq!(tokens.len(), 5);
        assert!(matches!(tokens[0], Token::Number(n) if (n - 1.0).abs() < f64::EPSILON));
        assert_eq!(tokens[1], Token::Plus);
    }

    #[test]
    fn test_tokenize_cell_refs() {
        let tokens = tokenize("A1+B2").unwrap();
        assert!(matches!(&tokens[0], Token::CellReference(s) if s == "A1"));
        assert!(matches!(&tokens[2], Token::CellReference(s) if s == "B2"));
    }

    #[test]
    fn test_tokenize_function() {
        let tokens = tokenize("SUM(A1:B3)").unwrap();
        assert!(matches!(&tokens[0], Token::Ident(s) if s == "SUM"));
        assert_eq!(tokens[1], Token::LParen);
    }

    #[test]
    fn test_tokenize_comparison() {
        let tokens = tokenize("A1<>B1").unwrap();
        assert!(matches!(&tokens[0], Token::CellReference(_)));
        assert_eq!(tokens[1], Token::Neq);
    }

    #[test]
    fn test_tokenize_string_literal() {
        let tokens = tokenize("\"hello world\"").unwrap();
        assert!(matches!(&tokens[0], Token::StringLit(s) if s == "hello world"));
    }

    // -----------------------------------------------------------------------
    // Formula evaluation – arithmetic
    // -----------------------------------------------------------------------

    #[test]
    fn test_simple_arithmetic() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=1+2") {
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON),
            other => panic!("Expected Number, got {other:?}"),
        }
    }

    #[test]
    fn test_operator_precedence() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=2+3*4") {
            CellValue::Number(n) => assert!((n - 14.0).abs() < f64::EPSILON),
            other => panic!("Expected 14, got {other:?}"),
        }
    }

    #[test]
    fn test_parentheses() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=(2+3)*4") {
            CellValue::Number(n) => assert!((n - 20.0).abs() < f64::EPSILON),
            other => panic!("Expected 20, got {other:?}"),
        }
    }

    #[test]
    fn test_power() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=2^10") {
            CellValue::Number(n) => assert!((n - 1024.0).abs() < f64::EPSILON),
            other => panic!("Expected 1024, got {other:?}"),
        }
    }

    #[test]
    fn test_unary_minus() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=-5+3") {
            CellValue::Number(n) => assert!((n - (-2.0)).abs() < f64::EPSILON),
            other => panic!("Expected -2, got {other:?}"),
        }
    }

    #[test]
    fn test_division_by_zero() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=1/0") {
            CellValue::Error(e) => assert!(e.contains("DIV/0")),
            other => panic!("Expected error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Concatenation & comparison
    // -----------------------------------------------------------------------

    #[test]
    fn test_concatenation() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=\"hello\" & \" world\"") {
            CellValue::Text(s) => assert_eq!(s, "hello world"),
            other => panic!("Expected text, got {other:?}"),
        }
    }

    #[test]
    fn test_comparison() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=3>2") {
            CellValue::Boolean(b) => assert!(b),
            other => panic!("Expected true, got {other:?}"),
        }
        match sheet.evaluate_formula("=3<2") {
            CellValue::Boolean(b) => assert!(!b),
            other => panic!("Expected false, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Cell references
    // -----------------------------------------------------------------------

    #[test]
    fn test_cell_reference_eval() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=A1+B1") {
            CellValue::Number(n) => assert!((n - 11.0).abs() < f64::EPSILON),
            other => panic!("Expected 11, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // SUM
    // -----------------------------------------------------------------------

    #[test]
    fn test_sum_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=SUM(A1:A3)") {
            CellValue::Number(n) => assert!((n - 6.0).abs() < f64::EPSILON),
            other => panic!("Expected 6, got {other:?}"),
        }
    }

    #[test]
    fn test_sum_multiple_args() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=SUM(A1:A3, B1:B3)") {
            CellValue::Number(n) => assert!((n - 66.0).abs() < f64::EPSILON),
            other => panic!("Expected 66, got {other:?}"),
        }
    }

    #[test]
    fn test_sum_two_dimensional() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=SUM(A1:B3)") {
            CellValue::Number(n) => assert!((n - 66.0).abs() < f64::EPSILON),
            other => panic!("Expected 66, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // AVERAGE
    // -----------------------------------------------------------------------

    #[test]
    fn test_average_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=AVERAGE(A1:A3)") {
            CellValue::Number(n) => assert!((n - 2.0).abs() < f64::EPSILON),
            other => panic!("Expected 2, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // COUNT / COUNTA
    // -----------------------------------------------------------------------

    #[test]
    fn test_count_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=COUNT(A1:B3)") {
            CellValue::Number(n) => assert!((n - 6.0).abs() < f64::EPSILON),
            other => panic!("Expected 6, got {other:?}"),
        }
    }

    #[test]
    fn test_counta_formula() {
        let sheet = make_test_sheet();
        // A1:C1 has 1, 10, "hello" => 3 non-empty
        match sheet.evaluate_formula("=COUNTA(A1:C1)") {
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON),
            other => panic!("Expected 3, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // MIN / MAX
    // -----------------------------------------------------------------------

    #[test]
    fn test_min_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=MIN(A1:B3)") {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
    }

    #[test]
    fn test_max_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=MAX(A1:B3)") {
            CellValue::Number(n) => assert!((n - 30.0).abs() < f64::EPSILON),
            other => panic!("Expected 30, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // IF
    // -----------------------------------------------------------------------

    #[test]
    fn test_if_formula() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=IF(A1>0, \"yes\", \"no\")") {
            CellValue::Text(s) => assert_eq!(s, "yes"),
            other => panic!("Expected 'yes', got {other:?}"),
        }
        match sheet.evaluate_formula("=IF(A1>100, \"big\", \"small\")") {
            CellValue::Text(s) => assert_eq!(s, "small"),
            other => panic!("Expected 'small', got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Nested formulas
    // -----------------------------------------------------------------------

    #[test]
    fn test_nested_formula() {
        let sheet = make_test_sheet();
        // IF(SUM(A1:A3)>5, MAX(B1:B3), MIN(B1:B3))
        match sheet.evaluate_formula("=IF(SUM(A1:A3)>5, MAX(B1:B3), MIN(B1:B3))") {
            CellValue::Number(n) => assert!((n - 30.0).abs() < f64::EPSILON),
            other => panic!("Expected 30, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // String functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_len() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=LEN(\"hello\")") {
            CellValue::Number(n) => assert!((n - 5.0).abs() < f64::EPSILON),
            other => panic!("Expected 5, got {other:?}"),
        }
    }

    #[test]
    fn test_upper_lower() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=UPPER(\"hello\")") {
            CellValue::Text(s) => assert_eq!(s, "HELLO"),
            other => panic!("Expected HELLO, got {other:?}"),
        }
        match sheet.evaluate_formula("=LOWER(\"HELLO\")") {
            CellValue::Text(s) => assert_eq!(s, "hello"),
            other => panic!("Expected hello, got {other:?}"),
        }
    }

    #[test]
    fn test_left_right_mid() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=LEFT(\"hello\", 3)") {
            CellValue::Text(s) => assert_eq!(s, "hel"),
            other => panic!("Expected 'hel', got {other:?}"),
        }
        match sheet.evaluate_formula("=RIGHT(\"hello\", 2)") {
            CellValue::Text(s) => assert_eq!(s, "lo"),
            other => panic!("Expected 'lo', got {other:?}"),
        }
        match sheet.evaluate_formula("=MID(\"hello\", 2, 3)") {
            CellValue::Text(s) => assert_eq!(s, "ell"),
            other => panic!("Expected 'ell', got {other:?}"),
        }
    }

    #[test]
    fn test_trim() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=TRIM(\"  hello   world  \")") {
            CellValue::Text(s) => assert_eq!(s, "hello world"),
            other => panic!("Expected 'hello world', got {other:?}"),
        }
    }

    #[test]
    fn test_concatenate() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=CONCATENATE(\"a\", \"b\", \"c\")") {
            CellValue::Text(s) => assert_eq!(s, "abc"),
            other => panic!("Expected 'abc', got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // ABS / ROUND
    // -----------------------------------------------------------------------

    #[test]
    fn test_abs() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=ABS(-42)") {
            CellValue::Number(n) => assert!((n - 42.0).abs() < f64::EPSILON),
            other => panic!("Expected 42, got {other:?}"),
        }
    }

    #[test]
    fn test_round() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=ROUND(3.14159, 2)") {
            CellValue::Number(n) => assert!((n - 3.14).abs() < 1e-9),
            other => panic!("Expected 3.14, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // VLOOKUP
    // -----------------------------------------------------------------------

    #[test]
    fn test_vlookup() {
        let mut sheet = Sheet::new("T".into());
        // Table: A1=1,B1="one"  A2=2,B2="two"  A3=3,B3="three"
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(0, 1, CellValue::Text("one".into()));
        sheet.set_cell(1, 0, CellValue::Number(2.0));
        sheet.set_cell(1, 1, CellValue::Text("two".into()));
        sheet.set_cell(2, 0, CellValue::Number(3.0));
        sheet.set_cell(2, 1, CellValue::Text("three".into()));

        match sheet.evaluate_formula("=VLOOKUP(2, A1:B3, 2)") {
            CellValue::Text(s) => assert_eq!(s, "two"),
            other => panic!("Expected 'two', got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // COUNTIF / SUMIF
    // -----------------------------------------------------------------------

    #[test]
    fn test_countif() {
        let sheet = make_test_sheet();
        match sheet.evaluate_formula("=COUNTIF(A1:A3, \">1\")") {
            CellValue::Number(n) => assert!((n - 2.0).abs() < f64::EPSILON),
            other => panic!("Expected 2, got {other:?}"),
        }
    }

    #[test]
    fn test_sumif() {
        let sheet = make_test_sheet();
        // Sum B values where A > 1 => B2 + B3 = 20 + 30 = 50
        match sheet.evaluate_formula("=SUMIF(A1:A3, \">1\", B1:B3)") {
            CellValue::Number(n) => assert!((n - 50.0).abs() < f64::EPSILON),
            other => panic!("Expected 50, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Circular reference detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_circular_reference() {
        let mut sheet = Sheet::new("T".into());
        // A1 depends on B1, B1 depends on A1
        sheet.set_formula(0, 0, "=B1+1".into());
        sheet.set_formula(0, 1, "=A1+1".into());
        // Evaluating A1 should detect the cycle.
        match sheet.evaluate_formula("=A1") {
            CellValue::Error(e) => assert!(
                e.contains("ircular"),
                "Expected circular ref error, got: {e}"
            ),
            other => panic!("Expected error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Recalculate
    // -----------------------------------------------------------------------

    #[test]
    fn test_recalculate() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(5.0));
        sheet.set_cell(1, 0, CellValue::Number(10.0));
        sheet.set_formula(2, 0, "=A1+A2".into());

        sheet.recalculate();

        let cell = sheet.get_cell(2, 0).unwrap();
        match &cell.value {
            CellValue::Number(n) => assert!((n - 15.0).abs() < f64::EPSILON),
            other => panic!("Expected 15, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Range expansion
    // -----------------------------------------------------------------------

    #[test]
    fn test_expand_range() {
        let refs = Sheet::expand_range(&CellRef::new(0, 0), &CellRef::new(2, 1));
        assert_eq!(refs.len(), 6); // 3 rows x 2 cols
        assert_eq!(refs[0], CellRef::new(0, 0));
        assert_eq!(refs[5], CellRef::new(2, 1));
    }

    // -----------------------------------------------------------------------
    // Auto-fill
    // -----------------------------------------------------------------------

    #[test]
    fn test_auto_fill_numbers() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(1, 0, CellValue::Number(2.0));
        // Fill A1:A2 down to A5
        sheet.auto_fill(CellRef::new(0, 0), CellRef::new(1, 0), CellRef::new(4, 0));
        // A3 should be 3, A4 should be 4, A5 should be 5
        let a3 = sheet.get_cell(2, 0).unwrap();
        match &a3.value {
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON, "Expected 3, got {n}"),
            other => panic!("Expected number, got {other:?}"),
        }
    }

    #[test]
    fn test_auto_fill_formula() {
        let mut sheet = Sheet::new("T".into());
        // A1 has values, B1 has formula =A1*2
        sheet.set_cell(0, 0, CellValue::Number(5.0));
        sheet.set_cell(1, 0, CellValue::Number(10.0));
        sheet.set_formula(0, 1, "=A1*2".into());
        sheet.auto_fill(CellRef::new(0, 1), CellRef::new(0, 1), CellRef::new(1, 1));
        // B2 should have formula =A2*2
        let b2 = sheet.get_cell(1, 1).unwrap();
        assert_eq!(b2.formula.as_deref(), Some("=A2*2"));
    }

    // -----------------------------------------------------------------------
    // Sort
    // -----------------------------------------------------------------------

    #[test]
    fn test_sort_range() {
        let mut sheet = Sheet::new("T".into());
        // A1=3,B1="c"  A2=1,B2="a"  A3=2,B3="b"
        sheet.set_cell(0, 0, CellValue::Number(3.0));
        sheet.set_cell(0, 1, CellValue::Text("c".into()));
        sheet.set_cell(1, 0, CellValue::Number(1.0));
        sheet.set_cell(1, 1, CellValue::Text("a".into()));
        sheet.set_cell(2, 0, CellValue::Number(2.0));
        sheet.set_cell(2, 1, CellValue::Text("b".into()));

        sheet.sort_range(&CellRef::new(0, 0), &CellRef::new(2, 1), 0, true);

        // After ascending sort by col 0:
        // Row 0: 1, "a"
        // Row 1: 2, "b"
        // Row 2: 3, "c"
        match &sheet.get_cell(0, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
        match &sheet.get_cell(0, 1).unwrap().value {
            CellValue::Text(s) => assert_eq!(s, "a"),
            other => panic!("Expected 'a', got {other:?}"),
        }
        match &sheet.get_cell(2, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON),
            other => panic!("Expected 3, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Copy cells
    // -----------------------------------------------------------------------

    #[test]
    fn test_copy_cells() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(42.0));
        sheet.set_formula(0, 1, "=A1*2".into());

        sheet.copy_cells(
            &CellRef::new(0, 0),
            &CellRef::new(0, 1),
            &CellRef::new(2, 0),
        );

        // C1 (row 2, col 0) should have 42
        match &sheet.get_cell(2, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 42.0).abs() < f64::EPSILON),
            other => panic!("Expected 42, got {other:?}"),
        }
        // C2 (row 2, col 1) should have formula =A3*2 (shifted by 2 rows)
        let c2 = sheet.get_cell(2, 1).unwrap();
        assert_eq!(c2.formula.as_deref(), Some("=A3*2"));
    }

    // -----------------------------------------------------------------------
    // Insert / delete rows & cols
    // -----------------------------------------------------------------------

    #[test]
    fn test_insert_rows() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(1, 0, CellValue::Number(2.0));
        sheet.set_cell(2, 0, CellValue::Number(3.0));

        sheet.insert_rows(1, 2); // Insert 2 rows at row 1

        // Row 0 unchanged
        match &sheet.get_cell(0, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
        // Row 1,2 should be empty (inserted)
        assert!(sheet.get_cell(1, 0).is_none());
        assert!(sheet.get_cell(2, 0).is_none());
        // Original row 1 (value=2) moved to row 3
        match &sheet.get_cell(3, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 2.0).abs() < f64::EPSILON),
            other => panic!("Expected 2, got {other:?}"),
        }
    }

    #[test]
    fn test_delete_rows() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(1, 0, CellValue::Number(2.0));
        sheet.set_cell(2, 0, CellValue::Number(3.0));
        sheet.set_cell(3, 0, CellValue::Number(4.0));

        sheet.delete_rows(1, 2); // Delete rows 1-2

        match &sheet.get_cell(0, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
        // Original row 3 (value=4) should now be at row 1
        match &sheet.get_cell(1, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 4.0).abs() < f64::EPSILON),
            other => panic!("Expected 4, got {other:?}"),
        }
    }

    #[test]
    fn test_insert_cols() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(0, 1, CellValue::Number(2.0));

        sheet.insert_cols(1, 1);

        match &sheet.get_cell(0, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
        assert!(sheet.get_cell(0, 1).is_none()); // inserted col
        match &sheet.get_cell(0, 2).unwrap().value {
            CellValue::Number(n) => assert!((n - 2.0).abs() < f64::EPSILON),
            other => panic!("Expected 2, got {other:?}"),
        }
    }

    #[test]
    fn test_delete_cols() {
        let mut sheet = Sheet::new("T".into());
        sheet.set_cell(0, 0, CellValue::Number(1.0));
        sheet.set_cell(0, 1, CellValue::Number(2.0));
        sheet.set_cell(0, 2, CellValue::Number(3.0));

        sheet.delete_cols(1, 1);

        match &sheet.get_cell(0, 0).unwrap().value {
            CellValue::Number(n) => assert!((n - 1.0).abs() < f64::EPSILON),
            other => panic!("Expected 1, got {other:?}"),
        }
        match &sheet.get_cell(0, 1).unwrap().value {
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON),
            other => panic!("Expected 3, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // NOW / TODAY
    // -----------------------------------------------------------------------

    #[test]
    fn test_now_returns_date() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=NOW()") {
            CellValue::Date(_) => {} // success
            other => panic!("Expected Date, got {other:?}"),
        }
    }

    #[test]
    fn test_today_returns_date() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=TODAY()") {
            CellValue::Date(_) => {}
            other => panic!("Expected Date, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Error handling
    // -----------------------------------------------------------------------

    #[test]
    fn test_unknown_function() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=FOOBAR(1)") {
            CellValue::Error(e) => assert!(e.contains("Unknown function")),
            other => panic!("Expected error, got {other:?}"),
        }
    }

    #[test]
    fn test_bad_syntax() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=1++2") {
            // Either a parse error or the unary + is consumed
            CellValue::Number(n) => assert!((n - 3.0).abs() < f64::EPSILON),
            CellValue::Error(_) => {} // also acceptable
            other => panic!("Unexpected: {other:?}"),
        }
    }

    #[test]
    fn test_empty_formula() {
        let sheet = Sheet::new("T".into());
        match sheet.evaluate_formula("=") {
            CellValue::Empty => {}
            other => panic!("Expected Empty, got {other:?}"),
        }
    }
}
