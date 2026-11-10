use crate::{parse, Diagnostic, ErrorKind, Lexer, Limits, Mode, Node, ParseError, Span, SyntaxKind as K, Tree};

/// One contiguous replacement in the old source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Edit {
    pub range: Span,
    pub replacement_len: u32,
}

/// Work performed by an incremental update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditStats {
    /// Source range lexed or parsed, in the new source.
    pub reparsed: Span,
    pub reused_nodes: usize,
    /// True when the edit changed lexical structure or invalidated recovery state.
    pub full_reparse: bool,
}

fn invalid(span: Span) -> ParseError { ParseError { kind: ErrorKind::InvalidEdit, span } }

pub(crate) fn validate_edit(old: &str, new: &str, edit: Edit, limits: Limits) -> Result<(), ParseError> {
    let range = edit.range.range();
    if range.start > range.end || range.end > old.len()
        || !old.is_char_boundary(range.start) || !old.is_char_boundary(range.end) {
        return Err(invalid(edit.range));
    }
    if new.len() > limits.max_source_bytes || new.len() >= u32::MAX as usize {
        return Err(ParseError { kind: ErrorKind::SourceLimit, span: edit.range });
    }
    let end = range.start.checked_add(edit.replacement_len as usize).ok_or_else(|| invalid(edit.range))?;
    if end > new.len() || !new.is_char_boundary(range.start) || !new.is_char_boundary(end)
        || old[..range.start] != new[..range.start] || old[range.end..] != new[end..] {
        return Err(invalid(edit.range));
    }
    Ok(())
}

/// Incrementally parses into a separate caller-owned arena.
///
/// A lexically stable leaf edit reuses the existing topology. Node spans after the
/// edit are shifted in one linear pass; source text is not copied. Structural
/// edits use a complete parse so recovery and enclosing syntax remain correct.
pub fn reparse<'s, 'a>(old: Tree<'_, '_>, new_source: &'s str, edit: Edit,
    nodes: &'a mut [Node], diagnostics: &'a mut [Diagnostic], limits: Limits)
    -> Result<(Tree<'s, 'a>, EditStats), ParseError>
{
    validate_edit(old.source, new_source, edit, limits)?;
    // Revalidating the depth/work budget is necessary for caller-selected limits.
    // The in-place owning API retains its original verified budget and can reuse.
    let candidate = if limits == Limits::default() { leaf_edit(old, new_source, edit) } else { None };
    if let Some(id) = candidate {
        if old.nodes.len() <= nodes.len().min(limits.max_nodes) && old.nodes.len() < limits.max_steps {
            nodes[..old.nodes.len()].copy_from_slice(old.nodes);
            shift_nodes(&mut nodes[..old.nodes.len()], id, edit);
            let span = nodes[id].span;
            return Ok((Tree::new(new_source, nodes, diagnostics, old.info),
                EditStats { reparsed: span, reused_nodes: old.nodes.len(), full_reparse: false }));
        }
    }
    let tree = parse(new_source, old.mode(), nodes, diagnostics, limits)?;
    Ok((tree, EditStats { reparsed: Span::new(0, new_source.len() as u32), reused_nodes: 0, full_reparse: true }))
}

pub(crate) fn leaf_edit(old: Tree<'_, '_>, new: &str, edit: Edit) -> Option<usize> {
    if !old.diagnostics.is_empty() { return None; }
    let removed = old.source.get(edit.range.range())?;
    let added_end = edit.range.start.checked_add(edit.replacement_len)?;
    let added = new.get(edit.range.start as usize..added_end as usize)?;
    // Letter-only replacements cannot open or close delimiters or whitespace.
    if removed.is_empty() || added.is_empty()
        || !removed.bytes().all(|b| b.is_ascii_alphabetic())
        || !added.bytes().all(|b| b.is_ascii_alphabetic()) { return None; }
    let delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    for (id, n) in old.nodes.iter().enumerate() {
        if !n.is_leaf() || n.span.start > edit.range.start || n.span.end < edit.range.end { continue; }
        let mode = match n.kind {
            K::Text => Mode::Markup,
            K::Ident | K::Str => Mode::Code,
            K::MathText | K::MathIdent => Mode::Math,
            _ => continue,
        };
        // Text runs can be context-sensitive at edges. Keep a letter on each side.
        if n.kind == K::Text && (edit.range.start <= n.span.start || edit.range.end >= n.span.end) { continue; }
        let end = (i64::from(n.span.end) + delta) as usize;
        let piece = new.get(n.span.start as usize..end)?;
        let mut lexer = Lexer::new(piece, mode);
        let token = lexer.next()?;
        if token.kind != n.kind || token.error.is_some() || token.span.end as usize != piece.len() || lexer.next().is_some() { continue; }
        // Identifier edits must retain token boundaries in the surrounding source.
        if matches!(n.kind, K::Ident | K::MathIdent | K::MathText)
            && (old.source[..n.span.start as usize].chars().next_back().is_some_and(crate::is_id_continue)
            || old.source[n.span.end as usize..].chars().next().is_some_and(crate::is_id_continue)) { continue; }
        return Some(id);
    }
    None
}

pub(crate) fn shift_nodes(nodes: &mut [Node], edited: usize, edit: Edit) {
    let delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    if delta == 0 { return; }
    for (id, n) in nodes.iter_mut().enumerate() {
        if id != edited && n.span.start >= edit.range.end {
            n.span.start = (i64::from(n.span.start) + delta) as u32;
            n.span.end = (i64::from(n.span.end) + delta) as u32;
        } else if n.span.end >= edit.range.end {
            n.span.end = (i64::from(n.span.end) + delta) as u32;
        }
    }
}

