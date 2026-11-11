use crate::{
    Diagnostic, ErrorKind, Lexer, Limits, Mode, NONE, Node, ParseError, ParseInfo, Span,
    SyntaxKind as K, Tree, parse, parse_into,
};

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
    /// Whether recovery or changed delimiters required parsing the whole source.
    pub full_reparse: bool,
}

fn invalid(span: Span) -> ParseError {
    ParseError {
        kind: ErrorKind::InvalidEdit,
        span,
    }
}

pub(crate) fn validate_edit(
    old: &str,
    new: &str,
    edit: Edit,
    limits: Limits,
) -> Result<(), ParseError> {
    let range = edit.range.range();
    if range.start > range.end
        || range.end > old.len()
        || !old.is_char_boundary(range.start)
        || !old.is_char_boundary(range.end)
    {
        return Err(invalid(edit.range));
    }
    if new.len() > limits.max_source_bytes || new.len() >= u32::MAX as usize {
        return Err(ParseError {
            kind: ErrorKind::SourceLimit,
            span: edit.range,
        });
    }
    let end = range
        .start
        .checked_add(edit.replacement_len as usize)
        .ok_or_else(|| invalid(edit.range))?;
    if end > new.len()
        || !new.is_char_boundary(range.start)
        || !new.is_char_boundary(end)
        || old[..range.start] != new[..range.start]
        || old[range.end..] != new[end..]
    {
        return Err(invalid(edit.range));
    }
    Ok(())
}

/// Incrementally parses into a separate caller-owned arena.
///
/// Stable leaf edits reuse the existing topology. Other edits first reparse the
/// smallest enclosing closed code block, content block, or equation. Delimiters
/// and the complete replacement subtree are checked before unchanged nodes are
/// reused. Recovery-sensitive edits fall back to the full bounded parser.
///
/// Copying and remapping the unchanged nodes takes linear time in the arena size,
/// even when only a short source range is parsed. The original tree and source
/// remain unchanged on every failure.
pub fn reparse<'s, 'a>(
    old: Tree<'_, '_>,
    new_source: &'s str,
    edit: Edit,
    nodes: &'a mut [Node],
    diagnostics: &'a mut [Diagnostic],
    limits: Limits,
) -> Result<(Tree<'s, 'a>, EditStats), ParseError> {
    validate_edit(old.source, new_source, edit, limits)?;
    reparse_validated(old, new_source, edit, nodes, diagnostics, limits)
}

pub(crate) fn reparse_validated<'s, 'a>(
    old: Tree<'_, '_>,
    new_source: &'s str,
    edit: Edit,
    nodes: &'a mut [Node],
    diagnostics: &'a mut [Diagnostic],
    limits: Limits,
) -> Result<(Tree<'s, 'a>, EditStats), ParseError> {
    if reuse_allowed(old, limits) {
        if let Some(id) = leaf_edit(old, new_source, edit) {
            if old.nodes.len() <= nodes.len() && old.diagnostics.len() <= diagnostics.len() {
                nodes[..old.nodes.len()].copy_from_slice(old.nodes);
                diagnostics[..old.diagnostics.len()].copy_from_slice(old.diagnostics);
                shift_nodes(&mut nodes[..old.nodes.len()], id, edit);
                shift_diagnostics(&mut diagnostics[..old.diagnostics.len()], edit);
                let span = nodes[id].span;
                return Ok((
                    Tree::new(new_source, nodes, diagnostics, old.info),
                    EditStats {
                        reparsed: span,
                        reused_nodes: old.nodes.len(),
                        full_reparse: false,
                    },
                ));
            }
        }
        if let Some((info, stats)) = subtree_edit(old, new_source, edit, nodes, diagnostics, limits)
        {
            return Ok((Tree::new(new_source, nodes, diagnostics, info), stats));
        }
    }
    let tree = parse(new_source, old.mode(), nodes, diagnostics, limits)?;
    Ok((
        tree,
        EditStats {
            reparsed: Span::new(0, new_source.len() as u32),
            reused_nodes: 0,
            full_reparse: true,
        },
    ))
}

pub(crate) fn reuse_allowed(old: Tree<'_, '_>, limits: Limits) -> bool {
    old.nodes.len() <= limits.max_nodes
        && old.diagnostics.len() <= limits.max_diagnostics
        && old.info.depth <= limits.max_depth.min(128)
        && old.info.steps <= limits.max_steps
}

fn delimiters(kind: K) -> Option<(K, K)> {
    match kind {
        K::CodeBlock => Some((K::LeftBrace, K::RightBrace)),
        K::ContentBlock => Some((K::LeftBracket, K::RightBracket)),
        K::Equation => Some((K::Dollar, K::Dollar)),
        _ => None,
    }
}

fn closed(nodes: &[Node], id: usize, span: Span, kind: K) -> bool {
    let Some((open, close)) = delimiters(kind) else {
        return false;
    };
    let Some(node) = nodes.get(id) else {
        return false;
    };
    let Some(first) = nodes.get(node.first_child as usize) else {
        return false;
    };
    if first.kind != open || first.span != Span::new(span.start, span.start + 1) {
        return false;
    }
    let mut last = first;
    while let Some(next) = nodes.get(last.next_sibling as usize) {
        last = next;
    }
    span.len() >= 2 && last.kind == close && last.span == Span::new(span.end - 1, span.end)
}

// Existing errors must form disjoint before/inside/after groups. An error that
// crosses a delimiter can involve surrounding recovery and forces a full parse.
fn diagnostic_partition(errors: &[Diagnostic], span: Span) -> Option<(usize, usize)> {
    let before = errors
        .iter()
        .take_while(|d| d.span.end <= span.start)
        .count();
    let mut end = before;
    while let Some(d) = errors.get(end) {
        if d.span.start >= span.end {
            break;
        }
        if d.span.start <= span.start || d.span.end >= span.end {
            return None;
        }
        end += 1;
    }
    if errors[end..].iter().any(|d| d.span.start < span.end) {
        return None;
    }
    Some((before, end))
}

fn isolated_context(old: Tree<'_, '_>, candidate: usize) -> bool {
    let mut parent = old.nodes[candidate].parent;
    while let Some(node) = old.nodes.get(parent as usize) {
        // Placeholder validation can be deferred until an enclosing collection
        // resolves into an expression, parameter list, or destructuring pattern.
        if matches!(
            node.kind,
            K::Array | K::Dict | K::Parenthesized | K::Params | K::Destructuring
        ) {
            return false;
        }
        parent = node.parent;
    }
    true
}

fn subtree_edit(
    old: Tree<'_, '_>,
    new: &str,
    edit: Edit,
    nodes: &mut [Node],
    diagnostics: &mut [Diagnostic],
    limits: Limits,
) -> Option<(ParseInfo, EditStats)> {
    let mut candidate = old
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            delimiters(node.kind).is_some()
                && node.span.start < edit.range.start
                && node.span.end > edit.range.end
        })
        .min_by_key(|(_, node)| node.span.len())
        .map(|(id, _)| id)?;
    let byte_delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    let mut remaining_steps = limits.max_steps.checked_sub(old.info.steps)?;
    loop {
        let node = old.nodes[candidate];
        let new_span = Span::new(
            node.span.start,
            (i64::from(node.span.end) + byte_delta) as u32,
        );
        if isolated_context(old, candidate) && closed(old.nodes, candidate, node.span, node.kind) {
            if let Some((before, after)) = diagnostic_partition(old.diagnostics, node.span) {
                let retained_errors = old.diagnostics.len() - (after - before);
                let source = new.get(new_span.range())?;
                let budget = Limits {
                    max_depth: limits.max_depth.min(128).checked_sub(old.info.depth)?,
                    max_steps: remaining_steps,
                    max_diagnostics: limits.max_diagnostics.checked_sub(retained_errors)?,
                    ..limits
                };
                if let Ok(local) = parse_into(source, Mode::Code, nodes, diagnostics, budget) {
                    remaining_steps = remaining_steps.checked_sub(local.steps)?;
                    // The artificial Code root must contain exactly the closed
                    // construct. Early closers and unterminated tokens fail here.
                    let wrapper = nodes[local.root as usize];
                    let root = wrapper.first_child as usize;
                    if let Some(replacement) = nodes.get(root) {
                        if local.nodes >= 2
                            && root == local.nodes - 2
                            && replacement.next_sibling == NONE
                            && replacement.kind == node.kind
                            && replacement.span == Span::new(0, source.len() as u32)
                            && closed(nodes, root, replacement.span, node.kind)
                        {
                            if let Some((info, reused)) =
                                splice(old, candidate, new_span, local, nodes, diagnostics, limits)
                            {
                                return Some((
                                    info,
                                    EditStats {
                                        reparsed: new_span,
                                        reused_nodes: reused,
                                        full_reparse: false,
                                    },
                                ));
                            }
                        }
                    }
                } else {
                    // A resource failure ends speculative reparsing. The full
                    // parser then validates the requested budget in context.
                    return None;
                }
            }
        }
        // A broken inner delimiter can still leave its enclosing block intact.
        let mut parent = node.parent;
        loop {
            let ancestor = old.nodes.get(parent as usize)?;
            if delimiters(ancestor.kind).is_some() {
                candidate = parent as usize;
                break;
            }
            parent = ancestor.parent;
        }
    }
}

fn splice(
    old: Tree<'_, '_>,
    candidate: usize,
    new_span: Span,
    local: ParseInfo,
    nodes: &mut [Node],
    diagnostics: &mut [Diagnostic],
    limits: Limits,
) -> Option<(ParseInfo, usize)> {
    let previous = old.nodes[candidate];
    let (before, after) = diagnostic_partition(old.diagnostics, previous.span)?;
    let error_count = old.diagnostics.len() - (after - before) + local.diagnostics;
    if error_count > diagnostics.len().min(limits.max_diagnostics) {
        return None;
    }
    let mut first = candidate;
    let mut last = candidate;
    let mut count = 0;
    for node in old.node(candidate as u32)?.descendants() {
        first = first.min(node.index() as usize);
        last = last.max(node.index() as usize);
        count += 1;
    }
    // These delimited constructs are allocated after their descendants. Check
    // contiguity explicitly before moving a complete subtree as one block.
    if last != candidate || last - first + 1 != count {
        return None;
    }
    let end = last + 1;
    let inserted = local.nodes - 1;
    let total = old.nodes.len().checked_sub(count)?.checked_add(inserted)?;
    if total > nodes.len().min(limits.max_nodes) || total >= u32::MAX as usize {
        return None;
    }
    let index_delta = inserted as i64 - count as i64;
    let byte_delta = i64::from(new_span.end) - i64::from(previous.span.end);
    let new_root = (first + inserted - 1) as u32;
    let remap = |id: u32| -> u32 {
        if id == NONE {
            NONE
        } else if id as usize >= end {
            (i64::from(id) + index_delta) as u32
        } else if id as usize >= first {
            new_root
        } else {
            id
        }
    };
    nodes.copy_within(0..inserted, first);
    for (old_id, old_node) in old.nodes.iter().enumerate() {
        if (first..end).contains(&old_id) {
            continue;
        }
        let id = if old_id < first {
            old_id
        } else {
            (old_id as i64 + index_delta) as usize
        };
        let mut n = *old_node;
        n.first_child = remap(n.first_child);
        n.next_sibling = remap(n.next_sibling);
        n.parent = remap(n.parent);
        if n.span.start >= previous.span.end {
            n.span.start = (i64::from(n.span.start) + byte_delta) as u32;
            n.span.end = (i64::from(n.span.end) + byte_delta) as u32;
        } else if n.span.end >= previous.span.end {
            n.span.end = (i64::from(n.span.end) + byte_delta) as u32;
        }
        nodes[id] = n;
    }
    for n in &mut nodes[first..first + inserted] {
        n.span.start += new_span.start;
        n.span.end += new_span.start;
        if n.first_child != NONE {
            n.first_child += first as u32;
        }
        if n.next_sibling != NONE {
            n.next_sibling += first as u32;
        }
        if n.parent != NONE {
            n.parent += first as u32;
        }
    }
    nodes[new_root as usize].parent = remap(previous.parent);
    nodes[new_root as usize].next_sibling = remap(previous.next_sibling);
    diagnostics.copy_within(0..local.diagnostics, before);
    diagnostics[..before].copy_from_slice(&old.diagnostics[..before]);
    for diagnostic in &mut diagnostics[before..before + local.diagnostics] {
        diagnostic.span.start += new_span.start;
        diagnostic.span.end += new_span.start;
    }
    for (output, input) in diagnostics[before + local.diagnostics..error_count]
        .iter_mut()
        .zip(&old.diagnostics[after..])
    {
        *output = Diagnostic {
            kind: input.kind,
            span: Span::new(
                (i64::from(input.span.start) + byte_delta) as u32,
                (i64::from(input.span.end) + byte_delta) as u32,
            ),
        };
    }
    Some((
        ParseInfo {
            root: remap(old.info.root),
            nodes: total,
            diagnostics: error_count,
            mode: old.mode(),
            depth: old.info.depth + local.depth,
            steps: old.info.steps + local.steps,
        },
        old.nodes.len() - count,
    ))
}

fn stable_token(piece: &str, mode: Mode, kind: K) -> bool {
    let mut lexer = Lexer::new(piece, mode);
    let Some(token) = lexer.next() else {
        return false;
    };
    token.kind == kind
        && token.error.is_none()
        && token.span.end as usize == piece.len()
        && lexer.next().is_none()
}

fn leaf_candidate(
    old: Tree<'_, '_>,
    added: &str,
    edit: Edit,
    preferred: Option<usize>,
    mut check: impl FnMut(Node, Mode) -> bool,
) -> Option<usize> {
    let removed = old.source.get(edit.range.range())?;
    if removed.is_empty()
        || added.is_empty()
        || !removed.bytes().all(|b| b.is_ascii_alphabetic())
        || !added.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }
    if old
        .diagnostics
        .iter()
        .any(|d| d.span.start < edit.range.end && d.span.end > edit.range.start)
    {
        return None;
    }
    let recent = preferred.and_then(|id| old.nodes.get(id).map(|node| (id, node)));
    // Nodes are normally appended in source-end order. A Pratt attachment can
    // extend an earlier node, so the probe is only a hint: validate its subtree
    // and retain a complete scan as the correctness fallback.
    let nearby = core::iter::once_with(|| {
        let mut low = 0;
        let mut high = old.nodes.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if old.nodes[middle].span.end < edit.range.end {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        old.node(low as u32)
    })
    .flatten()
    .flat_map(|node| node.descendants())
    .map(|node| (node.index() as usize, &old.nodes[node.index() as usize]));
    let remaining = old
        .nodes
        .iter()
        .enumerate()
        .filter(|(id, _)| Some(*id) != preferred);
    for (id, n) in recent.into_iter().chain(nearby).chain(remaining) {
        if !n.is_leaf() || n.span.start > edit.range.start || n.span.end < edit.range.end {
            continue;
        }
        let mode = match n.kind {
            K::Text => Mode::Markup,
            K::Ident | K::Str => Mode::Code,
            K::MathText | K::MathIdent => Mode::Math,
            _ => continue,
        };
        if n.kind == K::Text && (edit.range.start <= n.span.start || edit.range.end >= n.span.end) {
            continue;
        }
        if matches!(n.kind, K::Ident | K::MathIdent | K::MathText)
            && (old.source[..n.span.start as usize]
                .chars()
                .next_back()
                .is_some_and(crate::is_id_continue)
                || old.source[n.span.end as usize..]
                    .chars()
                    .next()
                    .is_some_and(crate::is_id_continue))
        {
            continue;
        }
        if check(*n, mode) {
            return Some(id);
        }
    }
    None
}

pub(crate) fn leaf_edit(old: Tree<'_, '_>, new: &str, edit: Edit) -> Option<usize> {
    let end = edit.range.start.checked_add(edit.replacement_len)?;
    let added = new.get(edit.range.start as usize..end as usize)?;
    let delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    leaf_candidate(old, added, edit, None, |node, mode| {
        let end = (i64::from(node.span.end) + delta) as usize;
        new.get(node.span.start as usize..end)
            .is_some_and(|piece| stable_token(piece, mode, node.kind))
    })
}

// Validate a small replacement token before changing an owned source. This lets
// equal-width edits reuse String storage, with no temporary copy of the document.
#[cfg(feature = "alloc")]
pub(crate) fn leaf_edit_patch(
    old: Tree<'_, '_>,
    added: &str,
    edit: Edit,
    preferred: Option<usize>,
) -> Option<usize> {
    let mut scratch = [0u8; 256];
    leaf_candidate(old, added, edit, preferred, |node, mode| {
        let prefix = &old.source.as_bytes()[node.span.start as usize..edit.range.start as usize];
        let suffix = &old.source.as_bytes()[edit.range.end as usize..node.span.end as usize];
        let Some(len) = prefix
            .len()
            .checked_add(added.len())
            .and_then(|n| n.checked_add(suffix.len()))
        else {
            return false;
        };
        if len > scratch.len() {
            return false;
        }
        scratch[..prefix.len()].copy_from_slice(prefix);
        scratch[prefix.len()..prefix.len() + added.len()].copy_from_slice(added.as_bytes());
        scratch[prefix.len() + added.len()..len].copy_from_slice(suffix);
        core::str::from_utf8(&scratch[..len])
            .is_ok_and(|piece| stable_token(piece, mode, node.kind))
    })
}

pub(crate) fn shift_nodes(nodes: &mut [Node], edited: usize, edit: Edit) {
    let delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    if delta == 0 {
        return;
    }
    for (id, n) in nodes.iter_mut().enumerate() {
        if id != edited && n.span.start >= edit.range.end {
            n.span.start = (i64::from(n.span.start) + delta) as u32;
            n.span.end = (i64::from(n.span.end) + delta) as u32;
        } else if n.span.end >= edit.range.end {
            n.span.end = (i64::from(n.span.end) + delta) as u32;
        }
    }
}

pub(crate) fn shift_diagnostics(diagnostics: &mut [Diagnostic], edit: Edit) {
    let delta = i64::from(edit.replacement_len) - i64::from(edit.range.len());
    if delta == 0 {
        return;
    }
    for diagnostic in diagnostics {
        if diagnostic.span.start >= edit.range.end {
            diagnostic.span.start = (i64::from(diagnostic.span.start) + delta) as u32;
        }
        if diagnostic.span.end >= edit.range.end {
            diagnostic.span.end = (i64::from(diagnostic.span.end) + delta) as u32;
        }
    }
}
