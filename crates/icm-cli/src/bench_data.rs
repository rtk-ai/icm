//! Test project and session prompts for `bench-agent`.
//!
//! A Rust math library with expression parsing, matrix operations,
//! statistics, and complex numbers — 10 source files, ~550 lines.

pub const PROJECT_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", CARGO_TOML),
    ("CLAUDE.md", CLAUDE_MD),
    ("src/main.rs", MAIN_RS),
    ("src/lib.rs", LIB_RS),
    ("src/error.rs", ERROR_RS),
    ("src/token.rs", TOKEN_RS),
    ("src/ast.rs", AST_RS),
    ("src/parser.rs", PARSER_RS),
    ("src/eval.rs", EVAL_RS),
    ("src/matrix.rs", MATRIX_RS),
    ("src/stats.rs", STATS_RS),
    ("src/complex.rs", COMPLEX_RS),
];

pub const SESSION_PROMPTS: &[&str] = &[
    // Session 1: full exploration (expensive for both modes)
    "Read this entire project. Explain the architecture, the parsing strategy, and how the modules connect to each other.",
    // Sessions 2+: specific recall questions
    "What parsing algorithm does this project use for operator precedence? How are right-associative operators like exponentiation handled?",
    "Explain the matrix determinant algorithm. What is its time complexity and why was cofactor expansion chosen?",
    "How does the evaluator handle variables? What built-in functions are available and how are they registered?",
    "Describe Welford's online algorithm as implemented in stats.rs. Why was it chosen over naive mean/variance?",
    "How does complex number division work in this project? Explain the conjugate multiplication technique.",
    "What error types exist? How does error propagation work from tokenizer through parser to evaluator?",
    "What is the token format for numbers? Can the tokenizer handle scientific notation like 1.5e-3?",
    "How would you add a new binary operator (e.g. modulo %) to this project? Which files need changes?",
    "Summarize the full architecture in 5 bullet points: parsing pipeline, evaluation, matrix ops, stats, and complex numbers.",
];

const CARGO_TOML: &str = r#"[package]
name = "mathlib"
version = "0.1.0"
edition = "2021"
description = "Expression parser, matrix operations, statistics, and complex numbers"

# No external dependencies — pure Rust
[dependencies]
"#;

const CLAUDE_MD: &str = r#"# mathlib — Expression parser and math toolkit

## Architecture

Recursive descent parser using Pratt precedence climbing, AST-based evaluation
with variable binding, matrix algebra, streaming statistics, and complex arithmetic.

## Design decisions

- **No external dependencies** — pure Rust, zero crates
- **Pratt parsing** — precedence climbing, not one grammar rule per level
- **Row-major Matrix** — flat `Vec<f64>` with `(rows, cols)`, not nested `Vec<Vec<f64>>`
- **Welford's algorithm** — streaming mean/variance for numerical stability
- **Cofactor expansion** for determinants — O(n!) but correct and simple for small matrices
- **Custom error enum** — `MathError` with variants per module, no anyhow/thiserror

## Module dependency graph

```
main.rs → lib.rs → parser.rs → token.rs → error.rs
                  → eval.rs   → ast.rs   → error.rs
                  → matrix.rs            → error.rs
                  → stats.rs
                  → complex.rs
```

## Adding a new operator

1. Add token variant in `token.rs`
2. Handle in tokenizer's `next_token()`
3. Add `Op` variant in `ast.rs` with precedence + associativity
4. Handle in `parser.rs` infix parsing
5. Evaluate in `eval.rs`
"#;

const MAIN_RS: &str = r#"//! CLI entry point — interactive expression evaluator.
//!
//! Reads expressions line by line from stdin, evaluates them,
//! and prints results. Supports variable assignment with `=`.
//!
//! Examples:
//!   > 2 + 3 * 4
//!   14
//!   > x = 10
//!   10
//!   > sqrt(x) + 1
//!   4.16227766...

use std::io::{self, BufRead, Write};

use mathlib::{Environment, Evaluator, Parser, Tokenizer};

fn main() {
    let mut env = Environment::new();
    let stdin = io::stdin();
    let stdout = io::stdout();

    loop {
        print!("> ");
        stdout.lock().flush().unwrap();

        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap() == 0 {
            break;
        }

        let line = line.trim();
        if line.is_empty() || line == "quit" {
            break;
        }

        match evaluate_line(line, &mut env) {
            Ok(result) => println!("{result}"),
            Err(e) => eprintln!("error: {e}"),
        }
    }
}

fn evaluate_line(input: &str, env: &mut Environment) -> Result<f64, mathlib::MathError> {
    let mut tokenizer = Tokenizer::new(input);
    let tokens = tokenizer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse()?;
    let mut evaluator = Evaluator::new(env);
    evaluator.eval(&expr)
}
"#;

const LIB_RS: &str = r#"//! mathlib — A pure-Rust math toolkit.
//!
//! Modules:
//! - `token`: Tokenizer (string → token stream)
//! - `ast`: Abstract syntax tree types
//! - `parser`: Pratt precedence parser (tokens → AST)
//! - `eval`: Expression evaluator (AST → f64) with variables
//! - `matrix`: Matrix operations (add, mul, det, inverse)
//! - `stats`: Streaming statistics (Welford's algorithm)
//! - `complex`: Complex number arithmetic

pub mod ast;
pub mod complex;
pub mod error;
pub mod eval;
pub mod matrix;
pub mod parser;
pub mod stats;
pub mod token;

pub use error::MathError;
pub use eval::{Environment, Evaluator};
pub use parser::Parser;
pub use token::Tokenizer;
"#;

const ERROR_RS: &str = r#"//! Error types for mathlib.

use std::fmt;

/// Unified error type for all mathlib operations.
#[derive(Debug, Clone)]
pub enum MathError {
    /// Tokenizer or parser error.
    ParseError(String),
    /// Evaluation error (type mismatch, overflow, etc.).
    EvalError(String),
    /// Matrix dimension or shape error.
    MatrixError(String),
    /// Division by zero in eval or matrix inverse.
    DivisionByZero,
    /// Reference to an undefined variable.
    UndefinedVariable(String),
    /// Dimension mismatch (e.g. matrix multiplication).
    DimensionMismatch { expected: usize, got: usize },
    /// Unknown function name.
    UnknownFunction(String),
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseError(msg) => write!(f, "parse error: {msg}"),
            Self::EvalError(msg) => write!(f, "eval error: {msg}"),
            Self::MatrixError(msg) => write!(f, "matrix error: {msg}"),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::UndefinedVariable(name) => write!(f, "undefined variable: {name}"),
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected}, got {got}")
            }
            Self::UnknownFunction(name) => write!(f, "unknown function: {name}"),
        }
    }
}

impl std::error::Error for MathError {}
"#;

const TOKEN_RS: &str = r#"//! Tokenizer — converts a string into a stream of tokens.
//!
//! Supports: numbers (including `1.5e-3` scientific notation), identifiers,
//! operators (`+ - * / ^`), parentheses, commas, and `=` for assignment.

use crate::error::MathError;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
    Eq,
    Eof,
}

pub struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, MathError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.chars.len() {
                tokens.push(Token::Eof);
                return Ok(tokens);
            }
            let ch = self.chars[self.pos];
            let tok = match ch {
                '+' => { self.pos += 1; Token::Plus }
                '-' => { self.pos += 1; Token::Minus }
                '*' => { self.pos += 1; Token::Star }
                '/' => { self.pos += 1; Token::Slash }
                '^' => { self.pos += 1; Token::Caret }
                '(' => { self.pos += 1; Token::LParen }
                ')' => { self.pos += 1; Token::RParen }
                ',' => { self.pos += 1; Token::Comma }
                '=' => { self.pos += 1; Token::Eq }
                c if c.is_ascii_digit() || c == '.' => self.read_number()?,
                c if c.is_ascii_alphabetic() || c == '_' => self.read_ident(),
                _ => return Err(MathError::ParseError(format!("unexpected char: {ch}"))),
            };
            tokens.push(tok);
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    /// Read a number, including optional decimal point and scientific notation.
    /// Examples: 42, 3.14, 1.5e-3, 2E10
    fn read_number(&mut self) -> Result<Token, MathError> {
        let start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        // Decimal part
        if self.pos < self.chars.len() && self.chars[self.pos] == '.' {
            self.pos += 1;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        // Exponent part (e.g. e-3, E+10)
        if self.pos < self.chars.len()
            && (self.chars[self.pos] == 'e' || self.chars[self.pos] == 'E')
        {
            self.pos += 1;
            if self.pos < self.chars.len()
                && (self.chars[self.pos] == '+' || self.chars[self.pos] == '-')
            {
                self.pos += 1;
            }
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        let n: f64 = s
            .parse()
            .map_err(|_| MathError::ParseError(format!("invalid number: {s}")))?;
        Ok(Token::Number(n))
    }

    fn read_ident(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_alphanumeric() || self.chars[self.pos] == '_')
        {
            self.pos += 1;
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        Token::Ident(name)
    }
}
"#;

const AST_RS: &str = r#"//! Abstract syntax tree types for math expressions.

/// An expression node in the AST.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Literal number: `42`, `3.14`
    Number(f64),
    /// Variable reference: `x`, `pi`
    Variable(String),
    /// Binary operation: `left op right`
    BinOp {
        op: Op,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary negation: `-expr`
    UnaryMinus(Box<Expr>),
    /// Function call: `sin(x)`, `max(a, b)`
    FnCall { name: String, args: Vec<Expr> },
    /// Variable assignment: `x = expr`
    Assign {
        name: String,
        value: Box<Expr>,
    },
}

/// Binary operators with precedence levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add, // precedence 1
    Sub, // precedence 1
    Mul, // precedence 2
    Div, // precedence 2
    Pow, // precedence 3, right-associative
}

impl Op {
    /// Binding power for Pratt parsing.
    /// Higher number = tighter binding.
    pub fn precedence(&self) -> u8 {
        match self {
            Op::Add | Op::Sub => 1,
            Op::Mul | Op::Div => 2,
            Op::Pow => 3,
        }
    }

    /// Right-associative operators (e.g. `2^3^4` = `2^(3^4)`).
    pub fn is_right_assoc(&self) -> bool {
        matches!(self, Op::Pow)
    }
}
"#;

const PARSER_RS: &str = r#"//! Pratt parser — converts token stream into an AST.
//!
//! Uses precedence climbing (Pratt parsing) to handle operator
//! precedence and associativity without recursive grammar rules
//! per precedence level.
//!
//! Grammar (informal):
//!   expr     = assign | infix
//!   assign   = IDENT '=' expr
//!   infix    = prefix (OP prefix)*   (Pratt-driven)
//!   prefix   = '-' prefix | atom
//!   atom     = NUMBER | IDENT | IDENT '(' args ')' | '(' expr ')'
//!   args     = expr (',' expr)*

use crate::ast::{Expr, Op};
use crate::error::MathError;
use crate::token::Token;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(&mut self) -> Result<Expr, MathError> {
        let expr = self.parse_expr(0)?;
        if self.peek() != &Token::Eof {
            return Err(MathError::ParseError(format!(
                "unexpected token: {:?}",
                self.peek()
            )));
        }
        Ok(expr)
    }

    /// Pratt precedence climbing entry point.
    /// `min_bp` is the minimum binding power to continue parsing infix.
    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, MathError> {
        let mut left = self.parse_prefix()?;

        loop {
            let op = match self.peek() {
                Token::Plus => Op::Add,
                Token::Minus => Op::Sub,
                Token::Star => Op::Mul,
                Token::Slash => Op::Div,
                Token::Caret => Op::Pow,
                _ => break,
            };

            let prec = op.precedence();
            if prec < min_bp {
                break;
            }
            self.advance(); // consume operator

            // Right-associative: same precedence continues; left: needs higher
            let next_bp = if op.is_right_assoc() { prec } else { prec + 1 };
            let right = self.parse_expr(next_bp)?;

            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    /// Parse prefix expressions: unary minus or atom.
    fn parse_prefix(&mut self) -> Result<Expr, MathError> {
        match self.peek().clone() {
            Token::Minus => {
                self.advance();
                let expr = self.parse_expr(10)?; // high bp for tight unary binding
                Ok(Expr::UnaryMinus(Box::new(expr)))
            }
            _ => self.parse_atom(),
        }
    }

    /// Parse atomic expressions: numbers, variables, function calls, parenthesized.
    fn parse_atom(&mut self) -> Result<Expr, MathError> {
        match self.peek().clone() {
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Number(n))
            }
            Token::Ident(name) => {
                self.advance();
                // Check for function call: `name(args)`
                if self.peek() == &Token::LParen {
                    self.advance(); // consume '('
                    let args = self.parse_args()?;
                    self.expect(Token::RParen)?;
                    Ok(Expr::FnCall { name, args })
                }
                // Check for assignment: `name = expr`
                else if self.peek() == &Token::Eq {
                    self.advance(); // consume '='
                    let value = self.parse_expr(0)?;
                    Ok(Expr::Assign {
                        name,
                        value: Box::new(value),
                    })
                } else {
                    Ok(Expr::Variable(name))
                }
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            tok => Err(MathError::ParseError(format!(
                "unexpected token in atom: {tok:?}"
            ))),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, MathError> {
        let mut args = Vec::new();
        if self.peek() == &Token::RParen {
            return Ok(args);
        }
        args.push(self.parse_expr(0)?);
        while self.peek() == &Token::Comma {
            self.advance();
            args.push(self.parse_expr(0)?);
        }
        Ok(args)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    fn expect(&mut self, expected: Token) -> Result<(), MathError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(&expected) {
            self.advance();
            Ok(())
        } else {
            Err(MathError::ParseError(format!(
                "expected {expected:?}, got {:?}",
                self.peek()
            )))
        }
    }
}
"#;

const EVAL_RS: &str = r#"//! Expression evaluator with variable environment.
//!
//! Built-in functions: sin, cos, tan, sqrt, abs, ln, exp, log2, floor, ceil,
//! min(a,b), max(a,b), pow(base, exp).
//!
//! Built-in constants: pi, e, tau.

use std::collections::HashMap;

use crate::ast::{Expr, Op};
use crate::error::MathError;

/// Variable environment — maps names to values.
pub struct Environment {
    vars: HashMap<String, f64>,
}

impl Environment {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        vars.insert("pi".into(), std::f64::consts::PI);
        vars.insert("e".into(), std::f64::consts::E);
        vars.insert("tau".into(), std::f64::consts::TAU);
        Self { vars }
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.vars.get(name).copied()
    }

    pub fn set(&mut self, name: String, value: f64) {
        self.vars.insert(name, value);
    }
}

/// Tree-walking evaluator.
pub struct Evaluator<'a> {
    env: &'a mut Environment,
}

impl<'a> Evaluator<'a> {
    pub fn new(env: &'a mut Environment) -> Self {
        Self { env }
    }

    pub fn eval(&mut self, expr: &Expr) -> Result<f64, MathError> {
        match expr {
            Expr::Number(n) => Ok(*n),
            Expr::Variable(name) => self
                .env
                .get(name)
                .ok_or_else(|| MathError::UndefinedVariable(name.clone())),
            Expr::BinOp { op, left, right } => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                match op {
                    Op::Add => Ok(l + r),
                    Op::Sub => Ok(l - r),
                    Op::Mul => Ok(l * r),
                    Op::Div => {
                        if r == 0.0 {
                            return Err(MathError::DivisionByZero);
                        }
                        Ok(l / r)
                    }
                    Op::Pow => Ok(l.powf(r)),
                }
            }
            Expr::UnaryMinus(inner) => Ok(-self.eval(inner)?),
            Expr::FnCall { name, args } => self.call_fn(name, args),
            Expr::Assign { name, value } => {
                let v = self.eval(value)?;
                self.env.set(name.clone(), v);
                Ok(v)
            }
        }
    }

    fn call_fn(&mut self, name: &str, args: &[Expr]) -> Result<f64, MathError> {
        let vals: Vec<f64> = args.iter().map(|a| self.eval(a)).collect::<Result<_, _>>()?;
        match (name, vals.as_slice()) {
            ("sin", [x]) => Ok(x.sin()),
            ("cos", [x]) => Ok(x.cos()),
            ("tan", [x]) => Ok(x.tan()),
            ("sqrt", [x]) => Ok(x.sqrt()),
            ("abs", [x]) => Ok(x.abs()),
            ("ln", [x]) => Ok(x.ln()),
            ("exp", [x]) => Ok(x.exp()),
            ("log2", [x]) => Ok(x.log2()),
            ("floor", [x]) => Ok(x.floor()),
            ("ceil", [x]) => Ok(x.ceil()),
            ("min", [a, b]) => Ok(a.min(*b)),
            ("max", [a, b]) => Ok(a.max(*b)),
            ("pow", [base, exp]) => Ok(base.powf(*exp)),
            _ => Err(MathError::UnknownFunction(name.into())),
        }
    }
}
"#;

const MATRIX_RS: &str = r#"//! Matrix operations — row-major storage.
//!
//! Stores data as a flat `Vec<f64>` with `(rows, cols)` dimensions.
//! Operations: add, mul, transpose, determinant (cofactor expansion),
//! trace, scalar mul, identity, and zeros.

use crate::error::MathError;

#[derive(Debug, Clone)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    data: Vec<f64>,
}

impl Matrix {
    pub fn new(rows: usize, cols: usize, data: Vec<f64>) -> Result<Self, MathError> {
        if data.len() != rows * cols {
            return Err(MathError::MatrixError(format!(
                "data length {} != {}x{} = {}",
                data.len(),
                rows,
                cols,
                rows * cols
            )));
        }
        Ok(Self { rows, cols, data })
    }

    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m.set(i, i, 1.0);
        }
        m
    }

    pub fn get(&self, row: usize, col: usize) -> f64 {
        self.data[row * self.cols + col]
    }

    pub fn set(&mut self, row: usize, col: usize, val: f64) {
        self.data[row * self.cols + col] = val;
    }

    pub fn transpose(&self) -> Self {
        let mut result = Self::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                result.set(c, r, self.get(r, c));
            }
        }
        result
    }

    pub fn add(&self, other: &Matrix) -> Result<Self, MathError> {
        if self.rows != other.rows || self.cols != other.cols {
            return Err(MathError::DimensionMismatch {
                expected: self.rows * self.cols,
                got: other.rows * other.cols,
            });
        }
        let data: Vec<f64> = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(a, b)| a + b)
            .collect();
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            data,
        })
    }

    pub fn mul(&self, other: &Matrix) -> Result<Self, MathError> {
        if self.cols != other.rows {
            return Err(MathError::DimensionMismatch {
                expected: self.cols,
                got: other.rows,
            });
        }
        let mut result = Self::zeros(self.rows, other.cols);
        for i in 0..self.rows {
            for j in 0..other.cols {
                let mut sum = 0.0;
                for k in 0..self.cols {
                    sum += self.get(i, k) * other.get(k, j);
                }
                result.set(i, j, sum);
            }
        }
        Ok(result)
    }

    pub fn scalar_mul(&self, s: f64) -> Self {
        Self {
            rows: self.rows,
            cols: self.cols,
            data: self.data.iter().map(|x| x * s).collect(),
        }
    }

    pub fn trace(&self) -> Result<f64, MathError> {
        if self.rows != self.cols {
            return Err(MathError::MatrixError("trace requires square matrix".into()));
        }
        Ok((0..self.rows).map(|i| self.get(i, i)).sum())
    }

    /// Determinant via cofactor expansion along the first row.
    /// Time complexity: O(n!) — acceptable for small matrices (n <= 10).
    /// For large matrices, LU decomposition would be O(n^3).
    pub fn determinant(&self) -> Result<f64, MathError> {
        if self.rows != self.cols {
            return Err(MathError::MatrixError(
                "determinant requires square matrix".into(),
            ));
        }
        Ok(self.det_recursive())
    }

    fn det_recursive(&self) -> f64 {
        let n = self.rows;
        if n == 1 {
            return self.get(0, 0);
        }
        if n == 2 {
            return self.get(0, 0) * self.get(1, 1) - self.get(0, 1) * self.get(1, 0);
        }
        let mut det = 0.0;
        for col in 0..n {
            let minor = self.minor(0, col);
            let sign = if col % 2 == 0 { 1.0 } else { -1.0 };
            det += sign * self.get(0, col) * minor.det_recursive();
        }
        det
    }

    /// Compute the minor matrix by removing row `r` and column `c`.
    fn minor(&self, r: usize, c: usize) -> Self {
        let n = self.rows - 1;
        let mut data = Vec::with_capacity(n * n);
        for i in 0..self.rows {
            if i == r {
                continue;
            }
            for j in 0..self.cols {
                if j == c {
                    continue;
                }
                data.push(self.get(i, j));
            }
        }
        Self {
            rows: n,
            cols: n,
            data,
        }
    }
}

impl std::fmt::Display for Matrix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for r in 0..self.rows {
            write!(f, "[")?;
            for c in 0..self.cols {
                if c > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{:.4}", self.get(r, c))?;
            }
            writeln!(f, "]")?;
        }
        Ok(())
    }
}
"#;

const STATS_RS: &str = r#"//! Streaming statistics using Welford's online algorithm.
//!
//! Welford's algorithm computes running mean and variance in a single pass
//! with O(1) memory, avoiding the catastrophic cancellation that happens
//! when computing variance as E[X^2] - E[X]^2 with floating point.
//!
//! Reference: Welford, B. P. (1962). "Note on a method for calculating
//! corrected sums of squares and products."

/// Streaming accumulator for mean, variance, min, max.
#[derive(Debug, Clone)]
pub struct StreamingStats {
    count: u64,
    mean: f64,
    m2: f64, // sum of squared deviations from mean
    min: f64,
    max: f64,
}

impl StreamingStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Add a new observation using Welford's update formula:
    ///   delta = x - old_mean
    ///   new_mean = old_mean + delta / n
    ///   delta2 = x - new_mean
    ///   M2 += delta * delta2
    pub fn push(&mut self, x: f64) {
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
        self.min = self.min.min(x);
        self.max = self.max.max(x);
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// Population variance (divide by N).
    pub fn variance_population(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / self.count as f64
    }

    /// Sample variance (divide by N-1) — Bessel's correction.
    pub fn variance_sample(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / (self.count - 1) as f64
    }

    pub fn std_dev(&self) -> f64 {
        self.variance_sample().sqrt()
    }

    pub fn min(&self) -> f64 {
        self.min
    }

    pub fn max(&self) -> f64 {
        self.max
    }
}

/// Simple linear regression via ordinary least squares.
/// Returns (slope, intercept) for y = slope * x + intercept.
pub fn linear_regression(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if n < 2.0 {
        return None;
    }

    let sum_x: f64 = points.iter().map(|(x, _)| x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| y).sum();
    let sum_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_x2: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_x2 - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        return None; // vertical line, undefined slope
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;

    Some((slope, intercept))
}

/// Compute the median of a slice (sorts a copy).
pub fn median(data: &[f64]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}
"#;

const COMPLEX_RS: &str = r#"//! Complex number arithmetic — rectangular form.
//!
//! Supports: add, sub, mul, div, magnitude, conjugate,
//! polar conversion, and display.
//!
//! Division uses the conjugate multiplication technique:
//!   (a+bi) / (c+di) = (a+bi)(c-di) / (c+di)(c-di)
//!                    = (a+bi)(c-di) / (c² + d²)

use std::fmt;
use std::ops;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// Create from polar form: r * (cos(theta) + i*sin(theta))
    pub fn from_polar(r: f64, theta: f64) -> Self {
        Self {
            re: r * theta.cos(),
            im: r * theta.sin(),
        }
    }

    /// Magnitude (absolute value): |z| = sqrt(re² + im²)
    pub fn magnitude(&self) -> f64 {
        (self.re * self.re + self.im * self.im).sqrt()
    }

    /// Phase angle in radians: atan2(im, re)
    pub fn phase(&self) -> f64 {
        self.im.atan2(self.re)
    }

    /// Complex conjugate: a+bi → a-bi
    pub fn conjugate(&self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// Multiplicative inverse: 1/z = conj(z) / |z|²
    pub fn inverse(&self) -> Option<Self> {
        let mag_sq = self.re * self.re + self.im * self.im;
        if mag_sq < f64::EPSILON {
            return None; // division by zero
        }
        Some(Self {
            re: self.re / mag_sq,
            im: -self.im / mag_sq,
        })
    }
}

impl ops::Add for Complex {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            re: self.re + rhs.re,
            im: self.im + rhs.im,
        }
    }
}

impl ops::Sub for Complex {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            re: self.re - rhs.re,
            im: self.im - rhs.im,
        }
    }
}

impl ops::Mul for Complex {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        // (a+bi)(c+di) = (ac-bd) + (ad+bc)i
        Self {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}

/// Division via conjugate multiplication:
///   (a+bi)/(c+di) = (a+bi)(c-di) / (c²+d²)
impl ops::Div for Complex {
    type Output = Option<Self>;
    fn div(self, rhs: Self) -> Option<Self> {
        let denom = rhs.re * rhs.re + rhs.im * rhs.im;
        if denom < f64::EPSILON {
            return None;
        }
        let conj = rhs.conjugate();
        let num = self * conj;
        Some(Self {
            re: num.re / denom,
            im: num.im / denom,
        })
    }
}

impl fmt::Display for Complex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.im >= 0.0 {
            write!(f, "{:.4}+{:.4}i", self.re, self.im)
        } else {
            write!(f, "{:.4}{:.4}i", self.re, self.im)
        }
    }
}
"#;
