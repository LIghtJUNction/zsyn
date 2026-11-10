//! Bounded, lossless Typst syntax trees in caller-owned memory.
//!
//! ```
//! use zsyn::{parse, Diagnostic, Limits, Mode, Node};
//! let mut nodes = [Node::EMPTY; 128];
//! let mut errors = [Diagnostic::EMPTY; 16];
//! let tree = parse("Hello *world*!", Mode::Markup, &mut nodes, &mut errors,
//!                  Limits::default())?;
//! assert_eq!(tree.root().text(), "Hello *world*!");
//! assert!(tree.diagnostics().is_empty());
//! # Ok::<(), zsyn::ParseError>(())
//! ```
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod grapheme;
mod grapheme_data;
mod incremental;
mod kind;
mod lexer;
#[cfg(feature = "alloc")]
mod owned;
mod parser;
mod tree;
mod unicode;

pub use incremental::{reparse, Edit, EditStats};
pub use kind::SyntaxKind;
pub use lexer::{is_id_continue, is_id_start, Lexer, Token};
#[cfg(feature = "alloc")]
pub use owned::OwnedTree;
pub use parser::parse_into;
pub use tree::{Children, Descendants, Node, NodeRef, Tree};

pub(crate) const NONE: u32 = u32::MAX;

/// Lexical and parsing context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Mode {
    /// Document text, inline formatting, and embedded code and math.
    Markup,
    /// Expressions and statements.
    Code,
    /// Mathematical notation and embedded code.
    Math,
}

/// Half-open UTF-8 byte range. Offsets are never character indices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Span {
    /// Inclusive byte offset.
    pub start: u32,
    /// Exclusive byte offset.
    pub end: u32,
}

impl Span {
    /// Constructs a byte span. Parsing validates externally supplied edit ranges.
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
    /// Byte length (zero for a reversed externally constructed range).
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
    /// Whether this range contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }
    /// Converts the span to an indexing range.
    pub fn range(self) -> core::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Syntax diagnostics and hard resource failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ErrorKind {
    UnexpectedToken,
    ExpectedToken,
    Unterminated,
    InvalidEscape,
    InvalidNumber,
    InvalidUtf8,
    InvalidEdit,
    SourceLimit,
    NodeLimit,
    DiagnosticLimit,
    DepthLimit,
    WorkLimit,
    AllocationFailed,
}

/// Recoverable syntax diagnostic with its exact byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub kind: ErrorKind,
    pub span: Span,
}

impl Diagnostic {
    /// Initializer for caller-owned diagnostic storage.
    pub const EMPTY: Self = Self {
        kind: ErrorKind::UnexpectedToken,
        span: Span::new(0, 0),
    };
}

/// A hard failure; no valid tree is returned after exhausting a resource budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub kind: ErrorKind,
    pub span: Span,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:?} at bytes {}..{}",
            self.kind, self.span.start, self.span.end
        )
    }
}

/// Explicit resource budgets. Caller slice lengths are additional hard limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Recursion budget, additionally capped at 128 by the implementation.
    pub max_depth: u16,
    pub max_nodes: usize,
    pub max_diagnostics: usize,
    pub max_source_bytes: usize,
    pub max_steps: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 96,
            max_nodes: 4_000_000,
            max_diagnostics: 4096,
            max_source_bytes: 64 * 1024 * 1024,
            max_steps: 128 * 1024 * 1024,
        }
    }
}

/// Used lengths and root index returned by the allocation-free parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseInfo {
    pub root: u32,
    pub nodes: usize,
    pub diagnostics: usize,
    pub mode: Mode,
    pub(crate) depth: u16,
    pub(crate) steps: usize,
}

/// Parses a complete source into caller-provided node and diagnostic slices.
///
/// Recoverable syntax errors are returned in the tree. Resource exhaustion returns
/// `Err`; partially filled buffers must not be interpreted as a complete tree.
pub fn parse<'s, 'a>(
    source: &'s str,
    mode: Mode,
    nodes: &'a mut [Node],
    diagnostics: &'a mut [Diagnostic],
    limits: Limits,
) -> Result<Tree<'s, 'a>, ParseError> {
    let info = parse_into(source, mode, nodes, diagnostics, limits)?;
    Ok(Tree::new(source, nodes, diagnostics, info))
}

/// Validates UTF-8 before parsing bytes. Invalid input is located without panicking.
pub fn parse_bytes<'s, 'a>(
    source: &'s [u8],
    mode: Mode,
    nodes: &'a mut [Node],
    diagnostics: &'a mut [Diagnostic],
    limits: Limits,
) -> Result<Tree<'s, 'a>, ParseError> {
    if source.len() > limits.max_source_bytes || source.len() >= u32::MAX as usize {
        return Err(ParseError {
            kind: ErrorKind::SourceLimit,
            span: Span::default(),
        });
    }
    let source = core::str::from_utf8(source).map_err(|e| ParseError {
        kind: ErrorKind::InvalidUtf8,
        span: Span::new(
            e.valid_up_to() as u32,
            (e.valid_up_to() + e.error_len().unwrap_or(1)).min(source.len()) as u32,
        ),
    })?;
    parse(source, mode, nodes, diagnostics, limits)
}
