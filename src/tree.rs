use crate::{Diagnostic, Mode, NONE, ParseInfo, Span, SyntaxKind};

/// Compact arena node. Text stays in the original source; links are arena indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Node {
    pub(crate) kind: SyntaxKind,
    pub(crate) span: Span,
    pub(crate) first_child: u32,
    pub(crate) next_sibling: u32,
    pub(crate) parent: u32,
}

impl Node {
    /// Initializer for caller-provided node storage.
    pub const EMPTY: Self = Self {
        kind: SyntaxKind::End,
        span: Span::new(0, 0),
        first_child: NONE,
        next_sibling: NONE,
        parent: NONE,
    };
    pub const fn kind(&self) -> SyntaxKind {
        self.kind
    }
    pub const fn span(&self) -> Span {
        self.span
    }
    pub const fn is_leaf(&self) -> bool {
        self.first_child == NONE
    }
    pub const fn first_child(&self) -> Option<u32> {
        if self.first_child == NONE {
            None
        } else {
            Some(self.first_child)
        }
    }
    pub const fn next_sibling(&self) -> Option<u32> {
        if self.next_sibling == NONE {
            None
        } else {
            Some(self.next_sibling)
        }
    }
    pub const fn parent(&self) -> Option<u32> {
        if self.parent == NONE {
            None
        } else {
            Some(self.parent)
        }
    }
}

/// Immutable view of source and the initialized portion of an arena.
#[derive(Clone, Copy, Debug)]
pub struct Tree<'s, 'a> {
    pub(crate) source: &'s str,
    pub(crate) nodes: &'a [Node],
    pub(crate) diagnostics: &'a [Diagnostic],
    pub(crate) info: ParseInfo,
}

impl<'s, 'a> Tree<'s, 'a> {
    pub(crate) fn new(
        source: &'s str,
        nodes: &'a [Node],
        diagnostics: &'a [Diagnostic],
        info: ParseInfo,
    ) -> Self {
        Self {
            source,
            nodes: &nodes[..info.nodes],
            diagnostics: &diagnostics[..info.diagnostics],
            info,
        }
    }
    pub fn source(self) -> &'s str {
        self.source
    }
    pub fn nodes(self) -> &'a [Node] {
        self.nodes
    }
    pub fn diagnostics(self) -> &'a [Diagnostic] {
        self.diagnostics
    }
    pub fn mode(self) -> Mode {
        self.info.mode
    }
    pub fn info(self) -> ParseInfo {
        self.info
    }
    pub fn root(self) -> NodeRef<'s, 'a> {
        NodeRef {
            tree: self,
            index: self.info.root,
        }
    }
    pub fn node(self, index: u32) -> Option<NodeRef<'s, 'a>> {
        self.nodes
            .get(index as usize)
            .map(|_| NodeRef { tree: self, index })
    }
    /// Depth-first iteration without recursion or a heap-allocated traversal stack.
    pub fn descendants(self) -> Descendants<'s, 'a> {
        self.root().descendants()
    }
    /// Retained arena storage in bytes (excluding source and unused capacity).
    pub fn storage_bytes(self) -> usize {
        core::mem::size_of_val(self.nodes) + core::mem::size_of_val(self.diagnostics)
    }
}

/// Read-only syntax node borrowing its source and arena.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'s, 'a> {
    tree: Tree<'s, 'a>,
    index: u32,
}

impl<'s, 'a> NodeRef<'s, 'a> {
    pub fn index(self) -> u32 {
        self.index
    }
    pub fn kind(self) -> SyntaxKind {
        self.tree.nodes[self.index as usize].kind
    }
    pub fn span(self) -> Span {
        self.tree.nodes[self.index as usize].span
    }
    pub fn text(self) -> &'s str {
        &self.tree.source[self.span().range()]
    }
    pub fn is_leaf(self) -> bool {
        self.tree.nodes[self.index as usize].is_leaf()
    }
    pub fn children(self) -> Children<'s, 'a> {
        Children {
            tree: self.tree,
            next: self.tree.nodes[self.index as usize].first_child,
        }
    }
    pub fn parent(self) -> Option<Self> {
        self.tree.node(self.tree.nodes[self.index as usize].parent)
    }
    /// Includes this node as the first element.
    pub fn descendants(self) -> Descendants<'s, 'a> {
        Descendants {
            tree: self.tree,
            root: self.index,
            next: self.index,
        }
    }
}

pub struct Children<'s, 'a> {
    tree: Tree<'s, 'a>,
    next: u32,
}
impl<'s, 'a> Iterator for Children<'s, 'a> {
    type Item = NodeRef<'s, 'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.tree.node(self.next)?;
        self.next = self.tree.nodes[self.next as usize].next_sibling;
        Some(node)
    }
}
impl core::iter::FusedIterator for Children<'_, '_> {}

pub struct Descendants<'s, 'a> {
    tree: Tree<'s, 'a>,
    root: u32,
    next: u32,
}
impl<'s, 'a> Iterator for Descendants<'s, 'a> {
    type Item = NodeRef<'s, 'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let out = self.tree.node(self.next)?;
        let n = self.tree.nodes[self.next as usize];
        if n.first_child != NONE {
            self.next = n.first_child;
        } else {
            let mut id = self.next;
            self.next = NONE;
            while id != self.root && id != NONE {
                let node = self.tree.nodes[id as usize];
                if node.next_sibling != NONE {
                    self.next = node.next_sibling;
                    break;
                }
                id = node.parent;
            }
        }
        Some(out)
    }
}
impl core::iter::FusedIterator for Descendants<'_, '_> {}
