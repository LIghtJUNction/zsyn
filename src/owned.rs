use alloc::{string::String, vec::Vec};
use core::ops::Range;
use crate::{incremental, parse_into, Diagnostic, Edit, EditStats, ErrorKind, Limits, Mode, Node, ParseError, ParseInfo, Span, Tree};

/// Optional owning convenience interface. Core parsing remains allocation-free.
#[derive(Clone, Debug)]
pub struct OwnedTree {
    source: String,
    nodes: Vec<Node>,
    diagnostics: Vec<Diagnostic>,
    info: ParseInfo,
    limits: Limits,
}

fn failure(kind: ErrorKind) -> ParseError { ParseError { kind, span: Span::new(0, 0) } }

fn grow<T: Clone>(v: &mut Vec<T>, size: usize, value: T) -> Result<(), ParseError> {
    if size > v.len() {
        v.try_reserve_exact(size - v.len()).map_err(|_| failure(ErrorKind::AllocationFailed))?;
        v.resize(size, value);
    }
    Ok(())
}

impl OwnedTree {
    /// Copies the source once and builds an owning tree using fallible allocation.
    pub fn parse(source: &str, mode: Mode, limits: Limits) -> Result<Self, ParseError> {
        if source.len() > limits.max_source_bytes || source.len() >= u32::MAX as usize {
            return Err(failure(ErrorKind::SourceLimit));
        }
        let mut text = String::new();
        text.try_reserve_exact(source.len()).map_err(|_| failure(ErrorKind::AllocationFailed))?;
        text.push_str(source);
        Self::from_string(text, mode, limits)
    }

    fn from_string(source: String, mode: Mode, limits: Limits) -> Result<Self, ParseError> {
        let mut nodes = Vec::new();
        let mut diagnostics = Vec::new();
        grow(&mut nodes, (source.len() / 2 + 16).min(limits.max_nodes), Node::EMPTY)?;
        grow(&mut diagnostics, 16.min(limits.max_diagnostics), Diagnostic::EMPTY)?;
        let info = loop {
            match parse_into(&source, mode, &mut nodes, &mut diagnostics, limits) {
                Ok(info) => break info,
                Err(e) if e.kind == ErrorKind::NodeLimit && nodes.len() < limits.max_nodes => {
                    let next = nodes.len().saturating_mul(2).max(16).min(limits.max_nodes);
                    grow(&mut nodes, next, Node::EMPTY)?;
                }
                Err(e) if e.kind == ErrorKind::DiagnosticLimit && diagnostics.len() < limits.max_diagnostics => {
                    let next = diagnostics.len().saturating_mul(2).max(16).min(limits.max_diagnostics);
                    grow(&mut diagnostics, next, Diagnostic::EMPTY)?;
                }
                Err(e) => return Err(e),
            }
        };
        nodes.truncate(info.nodes);
        diagnostics.truncate(info.diagnostics);
        Ok(Self { source, nodes, diagnostics, info, limits })
    }

    /// Borrows the tree without constructing or copying nodes.
    pub fn as_tree(&self) -> Tree<'_, '_> { Tree::new(&self.source, &self.nodes, &self.diagnostics, self.info) }
    pub fn source(&self) -> &str { &self.source }
    /// Actual retained heap capacities, including the owned source string.
    pub fn storage_capacity_bytes(&self) -> usize {
        self.source.capacity() + self.nodes.capacity() * core::mem::size_of::<Node>()
            + self.diagnostics.capacity() * core::mem::size_of::<Diagnostic>()
    }

    /// Applies a byte-range edit transactionally. On failure this tree is unchanged.
    ///
    /// Lexically stable leaf changes reuse the arena. Structural changes run the
    /// same bounded parser as a complete parse, ensuring identical recovery.
    pub fn edit(&mut self, range: Range<usize>, replacement: &str, limits: Limits) -> Result<EditStats, ParseError> {
        if range.start > range.end || range.end > self.source.len()
            || !self.source.is_char_boundary(range.start) || !self.source.is_char_boundary(range.end) {
            return Err(failure(ErrorKind::InvalidEdit));
        }
        let len = self.source.len() - (range.end - range.start);
        let len = len.checked_add(replacement.len()).ok_or_else(|| failure(ErrorKind::SourceLimit))?;
        if len > limits.max_source_bytes || len >= u32::MAX as usize { return Err(failure(ErrorKind::SourceLimit)); }
        let mut text = String::new();
        text.try_reserve_exact(len).map_err(|_| failure(ErrorKind::AllocationFailed))?;
        text.push_str(&self.source[..range.start]);
        text.push_str(replacement);
        text.push_str(&self.source[range.end..]);
        let edit = Edit { range: Span::new(range.start as u32, range.end as u32), replacement_len: replacement.len() as u32 };
        if limits == self.limits {
            if let Some(id) = incremental::leaf_edit(self.as_tree(), &text, edit) {
                incremental::shift_nodes(&mut self.nodes, id, edit);
                self.source = text;
                return Ok(EditStats { reparsed: self.nodes[id].span, reused_nodes: self.nodes.len(), full_reparse: false });
            }
        }
        let next = Self::from_string(text, self.info.mode, limits)?;
        *self = next;
        Ok(EditStats { reparsed: Span::new(0, len as u32), reused_nodes: 0, full_reparse: true })
    }
}
