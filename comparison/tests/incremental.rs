use zsyn::{
    parse, reparse, Diagnostic, Edit, ErrorKind, Limits, Mode, Node, OwnedTree, Span, Tree,
};

fn assert_same(actual: Tree<'_, '_>, expected: Tree<'_, '_>) {
    let record = |tree: Tree<'_, '_>| {
        tree.descendants()
            .map(|node| {
                let mut depth = 0;
                let mut parent = node.parent();
                while let Some(value) = parent {
                    depth += 1;
                    parent = value.parent();
                }
                (node.kind(), node.span(), depth, node.text().to_owned())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(actual.source(), expected.source());
    assert_eq!(
        record(actual),
        record(expected),
        "source: {:?}",
        actual.source()
    );
    assert_eq!(
        actual.diagnostics(),
        expected.diagnostics(),
        "source: {:?}",
        actual.source()
    );
    assert_eq!(actual.nodes().len(), actual.descendants().count());
}

fn update(source: &str, needle: &str, replacement: &str) -> (OwnedTree, zsyn::EditStats) {
    let limits = Limits::default();
    let mut tree = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
    let start = source.find(needle).unwrap();
    let stats = tree
        .edit(start..start + needle.len(), replacement, limits)
        .unwrap();
    let expected = OwnedTree::parse(tree.source(), Mode::Markup, limits).unwrap();
    assert_same(tree.as_tree(), expected.as_tree());
    (tree, stats)
}

#[test]
fn reparses_closed_code_content_and_math_subtrees() {
    for (source, needle, replacement, open, close) in [
        (
            "Before #{ let a = 1; a + 2 } after #let tail = 7\n",
            "1",
            "3 * 4",
            '{',
            '}',
        ),
        (
            "Before #box[Hello *world* and $x^2$] after",
            "world",
            "_new_ + #text(\"ok\")",
            '[',
            ']',
        ),
        ("Before $x^2 + y_1$ after", "2", "(a+b)", '$', '$'),
    ] {
        let (tree, stats) = update(source, needle, replacement);
        assert!(!stats.full_reparse, "{source:?}");
        assert!(stats.reused_nodes > 0);
        assert_eq!(
            tree.source().as_bytes()[stats.reparsed.start as usize],
            open as u8
        );
        assert_eq!(
            tree.source().as_bytes()[stats.reparsed.end as usize - 1],
            close as u8
        );
        assert!(stats.reparsed.len() < tree.source().len() as u32);
    }
}

#[test]
fn subtree_growth_and_shrinkage_preserve_all_links() {
    let source = "Lead #{ let v = (1, 2, 3); [body #v] } then $a_1^2$ tail";
    let (tree, grown) = update(source, "(1, 2, 3)", "(a: 1, b: [Hello *world*], c: $x^2$)");
    assert!(!grown.full_reparse);
    let (tree, shrunk) = update(tree.source(), "(a: 1, b: [Hello *world*], c: $x^2$)", "4");
    assert!(!shrunk.full_reparse);
    assert!(tree.as_tree().descendants().any(|n| n.text() == "$a_1^2$"));
}

#[test]
fn preserves_diagnostics_before_and_after_a_changed_subtree() {
    let source = "#(1 + ) before #{ let a = 1; a } after #(2 + )";
    let (tree, stats) = update(source, "= 1;", "= 1 + ;");
    assert!(!stats.full_reparse);
    assert!(tree.as_tree().diagnostics().len() >= 3);
    let (_, repaired) = update(tree.source(), "= 1 + ;", "= 7;");
    assert!(!repaired.full_reparse);
}

#[test]
fn stable_leaf_edit_reuses_storage_with_disjoint_errors() {
    let source = "Ordinary text before #(1 + ) and after";
    let limits = Limits::default();
    let mut tree = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
    let text_pointer = tree.source().as_ptr();
    let arena_pointer = tree.as_tree().nodes().as_ptr();
    let capacity = tree.storage_capacity_bytes();
    let stats = tree.edit(1..2, "b", limits).unwrap();
    assert!(!stats.full_reparse);
    assert_eq!(text_pointer, tree.source().as_ptr());
    assert_eq!(arena_pointer, tree.as_tree().nodes().as_ptr());
    assert_eq!(capacity, tree.storage_capacity_bytes());
    let expected = OwnedTree::parse(tree.source(), Mode::Markup, limits).unwrap();
    assert_same(tree.as_tree(), expected.as_tree());
    let stats = tree.edit(1..2, "long", limits).unwrap();
    assert!(!stats.full_reparse);
    let expected = OwnedTree::parse(tree.source(), Mode::Markup, limits).unwrap();
    assert_same(tree.as_tree(), expected.as_tree());
}

#[test]
fn changed_delimiters_expand_or_fall_back_correctly() {
    for replacement in ["}", "]", "$", "\"", "/*", "`", "#("] {
        let source = "Before #{ let x = [hello]; x } after";
        let (tree, _) = update(source, "hello", replacement);
        assert_eq!(tree.as_tree().root().text(), tree.source());
    }
}

#[test]
fn separate_arena_splices_without_extra_capacity() {
    let source = "Before #{ let a = 1; a } after $x^2$";
    let limits = Limits::default();
    let old = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
    let at = source.find('1').unwrap();
    let new = source.replacen('1', "(1 + 2)", 1);
    let expected = OwnedTree::parse(&new, Mode::Markup, limits).unwrap();
    let mut nodes = vec![Node::EMPTY; expected.as_tree().nodes().len()];
    let mut errors = vec![Diagnostic::EMPTY; expected.as_tree().diagnostics().len()];
    let (actual, stats) = reparse(
        old.as_tree(),
        &new,
        Edit {
            range: Span::new(at as u32, at as u32 + 1),
            replacement_len: 7,
        },
        &mut nodes,
        &mut errors,
        limits,
    )
    .unwrap();
    assert!(!stats.full_reparse);
    assert_same(actual, expected.as_tree());
}

#[test]
fn narrower_budgets_are_enforced_before_reuse() {
    let generous = Limits {
        max_depth: 128,
        ..Limits::default()
    };
    let source = format!("#{}\"alpha\"{}", "{".repeat(58), "}".repeat(58));
    let mut tree = OwnedTree::parse(&source, Mode::Markup, generous).unwrap();
    let at = source.find("alpha").unwrap() + 1;
    let limits = Limits::default();
    let new = format!("{}b{}", &source[..at], &source[at + 1..]);
    let expected = OwnedTree::parse(&new, Mode::Markup, limits).unwrap_err();
    assert_eq!(expected.kind, ErrorKind::DepthLimit);
    assert_eq!(
        tree.edit(at..at + 1, "b", limits).unwrap_err().kind,
        expected.kind
    );
    assert_eq!(tree.source(), source);
    let mut nodes = vec![Node::EMPTY; tree.as_tree().nodes().len() + 16];
    let mut errors = [Diagnostic::EMPTY; 32];
    let error = reparse(
        tree.as_tree(),
        &new,
        Edit {
            range: Span::new(at as u32, at as u32 + 1),
            replacement_len: 1,
        },
        &mut nodes,
        &mut errors,
        limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, expected.kind);
    for limits in [
        Limits {
            max_steps: 1,
            ..generous
        },
        Limits {
            max_nodes: 1,
            ..generous
        },
    ] {
        assert!(tree.edit(at..at + 1, "b", limits).is_err());
        assert_eq!(tree.source(), source);
    }
}

#[test]
fn failed_structural_edit_is_transactional() {
    let source = "Before #{ let a = 1; a } after";
    let limits = Limits::default();
    let mut tree = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
    let before = tree.clone();
    let at = source.find('1').unwrap();
    let small = Limits {
        max_nodes: tree.as_tree().nodes().len(),
        ..limits
    };
    assert_eq!(
        tree.edit(at..at + 1, "(1, 2, 3, 4, 5)", small)
            .unwrap_err()
            .kind,
        ErrorKind::NodeLimit
    );
    assert_same(tree.as_tree(), before.as_tree());
    let no_errors = Limits {
        max_diagnostics: 0,
        ..limits
    };
    assert_eq!(
        tree.edit(at..at + 1, "(1 + )", no_errors).unwrap_err().kind,
        ErrorKind::DiagnosticLimit
    );
    assert_same(tree.as_tree(), before.as_tree());
}

#[test]
fn arbitrary_structural_edits_match_full_reparse() {
    let cases = [
        "Lead #{ let x = (1, 2); [hello *world* #x] } and $x_1^2$ tail",
        "#(1 + ) before #box[hello $x^2$] after #(2 + )",
        "中文 #box[hello #{let x = 2; x}] / $alpha + sqrt(x)$",
    ];
    let patches = [
        "", "(", ")", "[", "]", "{", "}", "$", "\"", "\\", "2", "alpha", "π", "#[x]", "\n", "/*",
        "*/",
    ];
    let rounds = if cfg!(miri) { 8 } else { 1500 };
    let limits = Limits::default();
    let mut state = 0x7e57857c19u64;
    for round in 0..rounds {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let source = cases[round % cases.len()];
        let boundaries = source
            .char_indices()
            .map(|(at, _)| at)
            .chain([source.len()])
            .collect::<Vec<_>>();
        let start = (state as usize) % boundaries.len();
        let end = (start + (state >> 32) as usize % 3).min(boundaries.len() - 1);
        let range = boundaries[start]..boundaries[end];
        let replacement = patches[(state >> 16) as usize % patches.len()];
        let mut new = source.to_owned();
        new.replace_range(range.clone(), replacement);
        let expected = OwnedTree::parse(&new, Mode::Markup, limits);
        let mut actual = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
        match (actual.edit(range.clone(), replacement, limits), expected) {
            (Ok(_), Ok(expected)) => assert_same(actual.as_tree(), expected.as_tree()),
            (Err(a), Err(b)) => assert_eq!(a.kind, b.kind),
            (a, b) => panic!("edit mismatch source={source:?}, range={range:?}, replacement={replacement:?}, actual={a:?}, expected={b:?}"),
        }
    }
}

#[test]
fn core_output_capacity_failure_does_not_change_old_tree() {
    let source = "#box[hello $x^2$] after";
    let limits = Limits::default();
    let old = OwnedTree::parse(source, Mode::Markup, limits).unwrap();
    let new = "#box[hello $x^(1+2)$] after";
    let at = source.find('2').unwrap();
    let mut errors = [Diagnostic::EMPTY; 16];
    let error = reparse(
        old.as_tree(),
        new,
        Edit {
            range: Span::new(at as u32, at as u32 + 1),
            replacement_len: 5,
        },
        &mut [],
        &mut errors,
        limits,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::NodeLimit);
    assert_eq!(old.source(), source);
    let mut nodes = [Node::EMPTY; 256];
    let expected = parse(source, Mode::Markup, &mut nodes, &mut errors, limits).unwrap();
    assert_same(old.as_tree(), expected);
}

#[test]
fn sequential_edits_preserve_tree_and_cached_leaf_validity() {
    let limits = Limits::default();
    let mut tree = OwnedTree::parse("Alpha #box[beta] $x^2$ Gamma", Mode::Markup, limits).unwrap();
    for (old, new) in [
        ("l", "long"),
        ("long", "z"),
        ("beta", "delta"),
        ("x", "y"),
        ("y", "z"),
        ("#box", "#foo"),
        ("$z^2$", "$sum_(i=1)^n i$"),
        ("delta", "a *bold* line"),
        ("bold", "_"),
        ("Gamma", "Tail"),
    ] {
        let at = tree.source().rfind(old).unwrap();
        tree.edit(at..at + old.len(), new, limits).unwrap();
        let full = OwnedTree::parse(tree.source(), Mode::Markup, limits).unwrap();
        assert_same(tree.as_tree(), full.as_tree());
    }
}

#[test]
fn preserves_enclosing_pattern_validation_context() {
    for source in [
        "#let (name: [#(alpha)], body: {(alpha)}) = data",
        "#let f(x: [#(alpha)]) = x",
        "#((a: [#(alpha)], b: {(alpha)}) = x)",
    ] {
        let _ = update(source, "alpha", "_");
    }
}
