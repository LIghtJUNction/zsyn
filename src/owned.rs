use crate::{
    incremental, parse_into, Diagnostic, Edit, EditStats, ErrorKind, Limits, Mode, Node,
    ParseError, ParseInfo, Span, Tree,
};
use alloc::{string::String, vec::Vec};
use core::ops::Range;

/// Optional owning convenience interface. Core parsing remains allocation-free.
#[derive(Clone, Debug)]
pub struct OwnedTree {
    source: String,
    nodes: Vec<Node>,
    diagnostics: Vec<Diagnostic>,
    info: ParseInfo,
    recent_leaf: Option<usize>,
}

fn failure(kind: ErrorKind) -> ParseError {
    ParseError {
        kind,
        span: Span::new(0, 0),
    }
}

fn grow<T: Clone>(v: &mut Vec<T>, size: usize, value: T) -> Result<(), ParseError> {
    if size > v.len() {
        v.try_reserve_exact(size - v.len())
            .map_err(|_| failure(ErrorKind::AllocationFailed))?;
        v.resize(size, value);
    }
    Ok(())
}

fn dense_nodes(sample: &[u8]) -> usize {
    let mut estimate = 0usize;
    let mut word = false;
    let mut space = false;
    for &byte in sample {
        if byte.is_ascii_whitespace() {
            estimate += usize::from(!space);
            space = true;
            word = false;
        } else {
            space = false;
            match byte {
                b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'$' | b'"' | b'`' => {
                    estimate += 3;
                    word = false;
                }
                b'+' | b'-' | b'*' | b'/' | b'=' | b'<' | b'>' | b'^' | b'_' | b'&' | b'|'
                | b'!' | b':' | b';' | b',' | b'#' | b'\\' => {
                    estimate += 2;
                    word = false;
                }
                _ => {
                    estimate += usize::from(!word);
                    word = true;
                }
            }
        }
    }
    estimate
}

// Markup words separated by single spaces form one text node. Count those runs
// with the actual lexer, while keeping a conservative dense estimate for code
// and math regions. This bounded sample does not change parsing or its limits.
fn markup_nodes(sample: &str) -> usize {
    use crate::{Lexer, SyntaxKind as K};
    let mut lexer = Lexer::new(sample, Mode::Markup);
    let mut plain = 0usize;
    let mut dense = 0usize;
    while let Some(token) = lexer.next() {
        if matches!(token.kind, K::Hash | K::Dollar) {
            let mode = if token.kind == K::Hash {
                Mode::Code
            } else {
                Mode::Math
            };
            let mut region = Lexer::at_boundary(sample, mode, token.span.end as usize);
            let mut end = token.span.end as usize;
            let mut depth = 0usize;
            for next in &mut region {
                end = next.span.end as usize;
                if mode == Mode::Math && next.kind == K::Dollar {
                    break;
                }
                match next.kind {
                    K::LeftParen | K::LeftBracket | K::LeftBrace => depth += 1,
                    K::RightParen | K::RightBracket | K::RightBrace => {
                        depth = depth.saturating_sub(1)
                    }
                    K::Space
                        if mode == Mode::Code
                            && depth == 0
                            && sample[next.span.range()]
                                .chars()
                                .any(crate::lexer::is_newline) =>
                    {
                        break
                    }
                    _ => {}
                }
            }
            dense += dense_nodes(&sample.as_bytes()[token.span.start as usize..end]);
            lexer.set_position(end);
        } else {
            plain += match token.kind {
                K::Star | K::Underscore | K::RefMarker => 2,
                K::HeadingMarker | K::ListMarker | K::EnumMarker | K::LeftBracket => 3,
                K::TermMarker => 4,
                K::Raw => {
                    5 + 2 * sample.as_bytes()[token.span.range()]
                        .iter()
                        .filter(|&&byte| byte == b'\n' || byte == b'\r')
                        .count()
                }
                _ => 1,
            };
        }
    }
    dense + (plain * 9).div_ceil(8)
}

// Capacity hints use at most 8 KiB. Short sources avoid the additional lexical
// pass; all sources retry with checked growth if their density changes later.
fn initial_nodes(source: &str, mode: Mode, limit: usize) -> usize {
    let mut end = source.len().min(8192);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let sample = &source[..end];
    if sample.is_empty() {
        return 1.min(limit);
    }
    let dense = dense_nodes(sample.as_bytes());
    let estimate = if mode == Mode::Markup && source.len() > 8192 && dense <= sample.len() / 2 {
        markup_nodes(sample)
    } else {
        dense
    };
    source
        .len()
        .saturating_mul(estimate)
        .saturating_add(sample.len() - 1)
        .checked_div(sample.len())
        .unwrap_or(limit)
        .saturating_add(32)
        .min(limit)
}

impl OwnedTree {
    /// Copies the source once and builds a tree using fallible allocation.
    pub fn parse(source: &str, mode: Mode, limits: Limits) -> Result<Self, ParseError> {
        if source.len() > limits.max_source_bytes || source.len() >= u32::MAX as usize {
            return Err(failure(ErrorKind::SourceLimit));
        }
        let mut text = String::new();
        text.try_reserve_exact(source.len())
            .map_err(|_| failure(ErrorKind::AllocationFailed))?;
        text.push_str(source);
        Self::from_string(text, mode, limits)
    }

    fn from_string(source: String, mode: Mode, limits: Limits) -> Result<Self, ParseError> {
        let mut nodes = Vec::new();
        let mut diagnostics = Vec::new();
        grow(
            &mut nodes,
            initial_nodes(&source, mode, limits.max_nodes),
            Node::EMPTY,
        )?;
        grow(
            &mut diagnostics,
            (source.len() / 24 + 16).min(limits.max_diagnostics),
            Diagnostic::EMPTY,
        )?;
        let info = loop {
            match parse_into(&source, mode, &mut nodes, &mut diagnostics, limits) {
                Ok(info) => break info,
                Err(e) if e.kind == ErrorKind::NodeLimit && nodes.len() < limits.max_nodes => {
                    let next = nodes.len().saturating_mul(2).max(16).min(limits.max_nodes);
                    grow(&mut nodes, next, Node::EMPTY)?;
                }
                Err(e)
                    if e.kind == ErrorKind::DiagnosticLimit
                        && diagnostics.len() < limits.max_diagnostics =>
                {
                    let next = diagnostics
                        .len()
                        .saturating_mul(2)
                        .max(16)
                        .min(limits.max_diagnostics);
                    grow(&mut diagnostics, next, Diagnostic::EMPTY)?;
                }
                Err(e) => return Err(e),
            }
        };
        nodes.truncate(info.nodes);
        diagnostics.truncate(info.diagnostics);
        Ok(Self {
            source,
            nodes,
            diagnostics,
            info,
            recent_leaf: None,
        })
    }

    /// Borrows the tree without constructing or copying nodes.
    pub fn as_tree(&self) -> Tree<'_, '_> {
        Tree::new(&self.source, &self.nodes, &self.diagnostics, self.info)
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Actual retained heap capacities, including the owned source string.
    pub fn storage_capacity_bytes(&self) -> usize {
        self.source.capacity()
            + self.nodes.capacity() * core::mem::size_of::<Node>()
            + self.diagnostics.capacity() * core::mem::size_of::<Diagnostic>()
    }

    /// Applies a byte-range edit transactionally. On failure this tree is unchanged.
    ///
    /// Stable edits in tokens up to 256 bytes reuse the source and arena directly.
    /// Structural edits reparse a closed enclosing block when its boundaries and
    /// recovery state remain isolated; otherwise they use a complete parse.
    pub fn edit(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        limits: Limits,
    ) -> Result<EditStats, ParseError> {
        if range.start > range.end
            || range.end > self.source.len()
            || !self.source.is_char_boundary(range.start)
            || !self.source.is_char_boundary(range.end)
        {
            return Err(failure(ErrorKind::InvalidEdit));
        }
        let len = self.source.len() - (range.end - range.start);
        let len = len
            .checked_add(replacement.len())
            .ok_or_else(|| failure(ErrorKind::SourceLimit))?;
        if len > limits.max_source_bytes || len >= u32::MAX as usize {
            return Err(failure(ErrorKind::SourceLimit));
        }
        let edit = Edit {
            range: Span::new(range.start as u32, range.end as u32),
            replacement_len: replacement.len() as u32,
        };
        if incremental::reuse_allowed(self.as_tree(), limits) {
            if let Some(id) =
                incremental::leaf_edit_patch(self.as_tree(), replacement, edit, self.recent_leaf)
            {
                self.source
                    .try_reserve(len.saturating_sub(self.source.len()))
                    .map_err(|_| failure(ErrorKind::AllocationFailed))?;
                // Every fallible check has completed. The byte boundaries are
                // valid and capacity is sufficient, so replacement cannot allocate.
                self.source.replace_range(range, replacement);
                incremental::shift_nodes(&mut self.nodes, id, edit);
                incremental::shift_diagnostics(&mut self.diagnostics, edit);
                self.recent_leaf = Some(id);
                return Ok(EditStats {
                    reparsed: self.nodes[id].span,
                    reused_nodes: self.nodes.len(),
                    full_reparse: false,
                });
            }
        }
        let mut text = String::new();
        text.try_reserve_exact(len)
            .map_err(|_| failure(ErrorKind::AllocationFailed))?;
        text.push_str(&self.source[..range.start]);
        text.push_str(replacement);
        text.push_str(&self.source[range.end..]);
        let mut nodes = Vec::new();
        let mut diagnostics = Vec::new();
        let initial = self
            .nodes
            .len()
            .saturating_add(replacement.len().min(256))
            .saturating_add(16)
            .min(limits.max_nodes);
        grow(&mut nodes, initial, Node::EMPTY)?;
        grow(
            &mut diagnostics,
            self.diagnostics
                .len()
                .saturating_add(16)
                .min(limits.max_diagnostics),
            Diagnostic::EMPTY,
        )?;
        let (info, stats) = loop {
            let result = incremental::reparse_validated(
                self.as_tree(),
                &text,
                edit,
                &mut nodes,
                &mut diagnostics,
                limits,
            )
            .map(|(tree, stats)| (tree.info(), stats));
            match result {
                Ok(result) => break result,
                Err(e) if e.kind == ErrorKind::NodeLimit && nodes.len() < limits.max_nodes => {
                    let next = nodes.len().saturating_mul(2).max(16).min(limits.max_nodes);
                    grow(&mut nodes, next, Node::EMPTY)?;
                }
                Err(e)
                    if e.kind == ErrorKind::DiagnosticLimit
                        && diagnostics.len() < limits.max_diagnostics =>
                {
                    let next = diagnostics
                        .len()
                        .saturating_mul(2)
                        .max(16)
                        .min(limits.max_diagnostics);
                    grow(&mut diagnostics, next, Diagnostic::EMPTY)?;
                }
                Err(e) => return Err(e),
            }
        };
        nodes.truncate(info.nodes);
        diagnostics.truncate(info.diagnostics);
        *self = Self {
            source: text,
            nodes,
            diagnostics,
            info,
            recent_leaf: None,
        };
        Ok(stats)
    }
}
