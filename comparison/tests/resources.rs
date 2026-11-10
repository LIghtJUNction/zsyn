use zsyn::{parse, Diagnostic, ErrorKind, Lexer, Limits, Mode, Node, SyntaxKind};

#[test]
fn flat_trivia_and_attachments_have_linear_work() {
    let sizes: &[usize] = if cfg!(miri) {
        &[32, 128]
    } else {
        &[256, 2048, 8192]
    };
    for &repeats in sizes {
        let inputs = [
            (
                format!("#{{a; {}b}}", "/*x*/ ".repeat(repeats)),
                Mode::Markup,
            ),
            ("x_1^2 ".repeat(repeats), Mode::Math),
        ];
        for (source, mode) in inputs {
            let mut nodes = vec![Node::EMPTY; source.len() * 2 + 32];
            let mut diagnostics = [Diagnostic::EMPTY; 4];
            // The budget includes actual trivia lookahead and attachment
            // reparent visits, so repeatedly scanning prefixes fails without
            // relying on machine speed or a wall-clock assertion.
            let limits = Limits {
                max_steps: 18 * repeats + 100,
                ..Limits::default()
            };
            let tree = parse(&source, mode, &mut nodes, &mut diagnostics, limits)
                .unwrap_or_else(|error| panic!("{mode:?}, repeats={repeats}: {error:?}"));
            assert!(tree.diagnostics().is_empty());
            assert_eq!(tree.descendants().count(), tree.nodes().len());
            let rebuilt: String = tree
                .descendants()
                .filter(|node| node.is_leaf())
                .map(|node| node.text())
                .collect();
            assert_eq!(rebuilt, source);

            let error = parse(
                &source,
                mode,
                &mut nodes,
                &mut diagnostics,
                Limits {
                    max_steps: repeats * 2,
                    ..limits
                },
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::WorkLimit);
        }
    }
}

#[test]
fn alternating_attachments_obey_depth_and_work_limits() {
    let source = format!("x{}", "_1^2".repeat(if cfg!(miri) { 128 } else { 8192 }));
    let mut nodes = vec![Node::EMPTY; 4096];
    let mut diagnostics = [Diagnostic::EMPTY; 4];
    for (limits, kind) in [
        (Limits::default(), ErrorKind::DepthLimit),
        (
            Limits {
                max_steps: 32,
                ..Limits::default()
            },
            ErrorKind::WorkLimit,
        ),
    ] {
        let error = parse(&source, Mode::Math, &mut nodes, &mut diagnostics, limits).unwrap_err();
        assert_eq!(error.kind, kind);
        assert!(error.span.start < 1024);
    }
}

#[test]
fn newline_classification_stops_counting_after_a_paragraph() {
    for newline in [
        "\n", "\r\n", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}",
    ] {
        let source = newline.repeat(if cfg!(miri) { 16 } else { 4096 });
        for mode in [Mode::Markup, Mode::Code, Mode::Math] {
            let mut lexer = Lexer::new(&source, mode);
            let token = lexer.next().unwrap();
            assert_eq!(
                token.kind,
                if mode == Mode::Markup {
                    SyntaxKind::Parbreak
                } else {
                    SyntaxKind::Space
                }
            );
            assert_eq!(token.span.range(), 0..source.len());
            assert!(lexer.next().is_none());
        }
    }
}

#[cfg(feature = "oracle")]
#[test]
fn lookahead_cache_preserves_newline_and_attachment_trees() {
    use zsyn_comparison::equivalence;

    for newline in [
        "\n", "\r\n", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}",
    ] {
        for source in [
            format!("a{newline}+b"),
            format!("{{a; /* first{newline}*/ /* next */ b + c}}"),
        ] {
            equivalence(&source, Mode::Code).unwrap_or_else(|error| panic!("{source:?}: {error}"));
        }
    }
    for source in [
        "#{a;\n/*c*/ b + c}",
        "#if a { b } /*c*/\nelse [d]",
        "$ /* first\n*/ /* next */ x_1^2$",
    ] {
        equivalence(source, Mode::Markup).unwrap_or_else(|error| panic!("{source:?}: {error}"));
    }
    for source in [
        format!("x{}", "_1^2".repeat(64)),
        format!("x{}", "' _1^2".repeat(16)),
        format!("#{{a; {}b}}", "/*x*/ ".repeat(64)),
        "x_1^2_3".into(),
        "x_1_2^3".into(),
        "x^1_2^3_4".into(),
        "x'_1^2_3".into(),
        "x_1'^2_3".into(),
    ] {
        let mode = if source.starts_with('#') {
            Mode::Markup
        } else {
            Mode::Math
        };
        equivalence(&source, mode).unwrap_or_else(|error| panic!("{source:?}: {error}"));
    }
}
