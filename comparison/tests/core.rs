use std::collections::HashSet;
use zsyn::{
    parse, parse_bytes, Diagnostic, ErrorKind, Lexer, Limits, Mode, Node, OwnedTree, Span, Tree,
};
fn parsed(source: &str, mode: Mode) -> OwnedTree {
    OwnedTree::parse(source, mode, Limits::default())
        .unwrap_or_else(|error| panic!("parse failed for {mode:?} {source:?}: {error:?}"))
}
fn canonical(tree: Tree<'_, '_>) -> Vec<(String, Span, usize)> {
    fn walk(node: zsyn::NodeRef<'_, '_>, depth: usize, out: &mut Vec<(String, Span, usize)>) {
        out.push((format!("{:?}", node.kind()), node.span(), depth));
        for child in node.children() {
            walk(child, depth + 1, out);
        }
    }
    let mut output = Vec::new();
    walk(tree.root(), 0, &mut output);
    output
}

fn assert_tree(tree: Tree<'_, '_>) {
    if std::panic::catch_unwind(|| assert_tree_inner(tree)).is_err() {
        panic!(
            "invalid {:?} syntax tree for {:?}",
            tree.mode(),
            tree.source()
        );
    }
}

fn assert_tree_inner(tree: Tree<'_, '_>) {
    let source = tree.source();
    assert_eq!(tree.root().span(), Span::new(0, source.len() as u32));
    assert_eq!(tree.root().text(), source);
    let mut visited = HashSet::new();
    let mut leaves = String::new();
    for node in tree.descendants() {
        assert!(visited.insert(node.index()), "cycle or duplicate child");
        let span = node.span();
        assert!(span.start <= span.end);
        assert!(span.end as usize <= source.len());
        assert!(source.is_char_boundary(span.start as usize));
        assert!(source.is_char_boundary(span.end as usize));
        assert_eq!(node.text(), &source[span.range()]);
        if node.is_leaf() {
            leaves.push_str(node.text());
        }
        let mut cursor = span.start;
        for child in node.children() {
            assert_eq!(child.parent().unwrap().index(), node.index());
            assert_eq!(
                child.span().start,
                cursor,
                "non-contiguous children at {:?}",
                node.kind()
            );
            cursor = child.span().end;
        }
        if !node.is_leaf() {
            assert_eq!(cursor, span.end);
        }
    }
    assert_eq!(visited.len(), tree.nodes().len(), "unreachable arena nodes");
    assert_eq!(leaves, source, "leaf text must reproduce the exact source");
    for diagnostic in tree.diagnostics() {
        assert!(diagnostic.span.start <= diagnostic.span.end);
        assert!(diagnostic.span.end as usize <= source.len());
        assert!(source.is_char_boundary(diagnostic.span.start as usize));
        assert!(source.is_char_boundary(diagnostic.span.end as usize));
    }
}

#[test]
fn caller_owned_arenas_and_zero_copy_text() {
    let source = "你好 *café* $x^2$ #let x = 123\n";
    let mut nodes = [Node::EMPTY; 256];
    let mut diagnostics = [Diagnostic::EMPTY; 32];
    let tree = parse(
        source,
        Mode::Markup,
        &mut nodes,
        &mut diagnostics,
        Limits::default(),
    )
    .unwrap();
    assert_tree(tree);
    assert_eq!(tree.root().text().as_ptr(), source.as_ptr());
    assert_eq!(
        tree.storage_bytes(),
        tree.nodes().len() * std::mem::size_of::<Node>()
            + tree.diagnostics().len() * std::mem::size_of::<Diagnostic>()
    );
    assert_eq!(std::mem::size_of::<Node>(), 24);
    assert!(tree.node(u32::MAX).is_none());
}

#[test]
fn empty_and_boundary_inputs() {
    for mode in [Mode::Markup, Mode::Code, Mode::Math] {
        for text in [
            "",
            " ",
            "\r\n",
            "\0",
            "🦀",
            "中",
            "\\",
            "#",
            "$",
            "(",
            ")",
            "{",
            "}",
            "[",
            "]",
            "`",
            "/*",
            "/* /* nested */ */",
            "\"\\u{1f980}\"",
            "\u{feff}",
        ] {
            assert_tree(parsed(text, mode).as_tree());
        }
    }
}

#[test]
fn all_tokens_are_contiguous_utf8_ranges() {
    let inputs = [
        "abc\n中文 café _x 123",
        "/* nested /* a */ b */ // comment\n",
        "\"a\\\"b\" 1.5e-3 12pt <= =>",
        "@ref <label> https://example.com",
        "x_1^2 + sqrt(π) \\u{1f980}",
    ];
    for mode in [Mode::Markup, Mode::Code, Mode::Math] {
        for input in inputs {
            let mut cursor = 0;
            for token in Lexer::new(input, mode) {
                assert_eq!(token.span.start as usize, cursor);
                assert!(token.span.end > token.span.start);
                assert!(input.is_char_boundary(token.span.end as usize));
                cursor = token.span.end as usize;
            }
            assert_eq!(cursor, input.len());
        }
    }
}

#[test]
fn rejects_invalid_utf8_without_panicking() {
    for bytes in [
        &b"\xff"[..],
        &b"a\xc0\xaf"[..],
        &b"ok\xf0\x9f"[..],
        &b"\xed\xa0\x80"[..],
    ] {
        let mut nodes = [Node::EMPTY; 64];
        let mut diagnostics = [Diagnostic::EMPTY; 16];
        let error = parse_bytes(
            bytes,
            Mode::Markup,
            &mut nodes,
            &mut diagnostics,
            Limits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidUtf8);
        assert!(error.span.start < error.span.end);
        assert!(error.span.end as usize <= bytes.len());
    }
}

#[test]
fn resource_limits_return_errors() {
    let mut nodes = [Node::EMPTY; 64];
    let mut diagnostics = [Diagnostic::EMPTY; 16];
    let limits = Limits {
        max_source_bytes: 2,
        ..Limits::default()
    };
    assert_eq!(
        parse("abc", Mode::Markup, &mut nodes, &mut diagnostics, limits)
            .unwrap_err()
            .kind,
        ErrorKind::SourceLimit
    );
    assert_eq!(
        parse(
            "abc",
            Mode::Markup,
            &mut [],
            &mut diagnostics,
            Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::NodeLimit
    );
    let limits = Limits {
        max_nodes: 1,
        ..Limits::default()
    };
    assert_eq!(
        parse("abc", Mode::Markup, &mut nodes, &mut diagnostics, limits)
            .unwrap_err()
            .kind,
        ErrorKind::NodeLimit
    );
    let limits = Limits {
        max_steps: 0,
        ..Limits::default()
    };
    assert_eq!(
        parse("abc", Mode::Markup, &mut nodes, &mut diagnostics, limits)
            .unwrap_err()
            .kind,
        ErrorKind::WorkLimit
    );
}

#[test]
fn deep_nesting_stops_at_the_declared_budget() {
    let source = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
    let limits = Limits {
        max_depth: 12,
        ..Limits::default()
    };
    let error = OwnedTree::parse(&source, Mode::Code, limits).unwrap_err();
    assert_eq!(error.kind, ErrorKind::DepthLimit);
}

#[test]
fn malformed_documents_recover_with_located_diagnostics() {
    for input in [
        "#(1 + )",
        "#let x = \"unterminated",
        "/* never closed",
        "#foo(1,,2)",
    ] {
        let tree = parsed(input, Mode::Markup);
        assert_tree(tree.as_tree());
        assert!(
            !tree.as_tree().diagnostics().is_empty(),
            "no diagnostic for {input:?}"
        );
    }
}

fn random(seed: &mut u64) -> usize {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed as usize
}

#[test]
fn deterministic_random_utf8_is_lossless_and_bounded() {
    let alphabet = [
        "a", "0", " ", "\n", "\r", "\t", "#", "$", "[", "]", "{", "}", "(", ")", "*", "_", "`",
        "\\", "/", "\"", "'", "@", "<", ">", "+", "-", ":", ";", "=", "中", "🦀", "\u{301}", "\0",
    ];
    let count = if cfg!(miri) { 32 } else { 3000 };
    let mut seed = 0xf81c_a5b3_7d92_064e;
    for _ in 0..count {
        let len = random(&mut seed) % 80;
        let mut input = String::new();
        for _ in 0..len {
            input.push_str(alphabet[random(&mut seed) % alphabet.len()]);
        }
        for mode in [Mode::Markup, Mode::Code, Mode::Math] {
            assert_tree(parsed(&input, mode).as_tree());
        }
    }
}

#[test]
fn edits_equal_complete_reparsing() {
    let seeds = [
        "Hello ordinary world!",
        "= Title\n\nA *strong* phrase with $x^2$.",
        "#let x = 123\n#foo(a: x)[body]\n",
        "中文 café and 🦀",
        "#(1 + )\nunfinished *",
    ];
    let replacements = ["a", "", "中", "\n", "#", "$", "/*", "\"", "[]"];
    for mode in [Mode::Markup, Mode::Code, Mode::Math] {
        for source in seeds {
            let boundaries: Vec<usize> = source
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(source.len()))
                .collect();
            for (index, &at) in boundaries.iter().enumerate() {
                if cfg!(miri) && index % 5 != 0 {
                    continue;
                }
                for replacement in replacements {
                    let end = boundaries.get(index + 1).copied().unwrap_or(at);
                    let mut edited = parsed(source, mode);
                    edited
                        .edit(at..end, replacement, Limits::default())
                        .unwrap();
                    let mut expected = source.to_owned();
                    expected.replace_range(at..end, replacement);
                    let full = parsed(&expected, mode);
                    assert_eq!(edited.as_tree().source(), expected);
                    assert_eq!(
                        canonical(edited.as_tree()),
                        canonical(full.as_tree()),
                        "edit {at}..{end} -> {replacement:?} in {source:?} ({mode:?})"
                    );
                    assert_eq!(edited.as_tree().diagnostics(), full.as_tree().diagnostics());
                    assert_tree(edited.as_tree());
                }
            }
        }
    }
}

#[test]
fn invalid_edits_leave_the_original_tree_intact() {
    let mut tree = parsed("a🦀b", Mode::Markup);
    let before = canonical(tree.as_tree());
    for range in [2..3, 10..10, 4..1] {
        assert_eq!(
            tree.edit(range, "x", Limits::default()).unwrap_err().kind,
            ErrorKind::InvalidEdit
        );
        assert_eq!(tree.as_tree().source(), "a🦀b");
        assert_eq!(canonical(tree.as_tree()), before);
    }
}

#[test]
fn plain_text_edit_reuses_syntax_nodes() {
    let source = "Hello ordinary world.\n\nA separate paragraph.";
    let mut tree = parsed(source, Mode::Markup);
    let stats = tree.edit(8..9, "Z", Limits::default()).unwrap();
    assert!(!stats.full_reparse);
    assert!(stats.reused_nodes > 0);
    assert!((stats.reparsed.len() as usize) < source.len());
    assert_tree(tree.as_tree());
}
