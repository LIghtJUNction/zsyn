#![cfg(feature = "oracle")]
use zsyn::Mode;
use zsyn_comparison::{equivalence, workloads};

#[test]
fn full_cst_matches_pinned_oracle() {
    let cases = [
        (Mode::Markup, ""),
        (Mode::Markup, "Hello world.\n\n中文 café λ 🦀"),
        (Mode::Markup, "*strong* and _emphasis_"),
        (Mode::Markup, "= Title\n== Subtitle\n\nBody"),
        (Mode::Markup, "- first\n- second\n+ first\n1. second\n"),
        (Mode::Markup, "/ Term: description\n"),
        (Mode::Markup, "@intro <intro> https://example.com/a"),
        (Mode::Markup, "`code` ```rust\nlet x = 1;\n```"),
        (
            Mode::Markup,
            "\\# \\u{1f980} ~ --- -- -? ... 'quote' \"quote\"",
        ),
        (Mode::Markup, "/* outer /* inner */ tail */ // line\ntext"),
        (Mode::Markup, "#let x = 1 + 2\n#x"),
        (Mode::Markup, "#foo(a: 1, b: (2, 3))[hello]"),
        (Mode::Markup, "$x^2 + y_1$"),
        (
            Mode::Code,
            "none; auto; true; false; 123; 0xff; 1.2e-3; 12pt; 50%",
        ),
        (Mode::Code, "\"escape \\u{1f980} \\n \\\"\""),
        (Mode::Code, "1 + 2 * 3 - 4 / 5"),
        (Mode::Code, "not false and true or false"),
        (Mode::Code, "x = 1; x += 2; x -= 1; x *= 3; x /= 2"),
        (Mode::Code, "(1 + 2); (); (1,); (1, 2); (:); (a: 1, b: 2)"),
        (Mode::Code, "foo(1, a: 2, ..xs)[body]"),
        (Mode::Code, "let square(x) = x * x; square(3)"),
        (Mode::Code, "let f = (a, b: 1, ..rest) => a + b"),
        (Mode::Code, "let (a, b) = (1, 2)"),
        (Mode::Code, "if x > 0 { x } else { -x }"),
        (Mode::Code, "for (key, value) in items { value }"),
        (
            Mode::Code,
            "while x < 10 { x += 1; if x == 3 { continue }; break }",
        ),
        (Mode::Code, "set text(size: 12pt) if true"),
        (Mode::Code, "show heading: it => [*#it.body*]"),
        (Mode::Code, "context text.lang"),
        (Mode::Code, "import \"a.typ\": f, g as h; include \"a.typ\""),
        (Mode::Code, "return 1"),
        (Mode::Math, "x^2 + y_1"),
        (Mode::Math, "(a+b)/(c+d)"),
        (Mode::Math, "alpha + beta = gamma"),
        (Mode::Math, "sqrt(x) + sin(x)"),
        (Mode::Math, "sum_(i=1)^n i"),
        (Mode::Math, "x''' + √x + ∛y"),
        (Mode::Math, "x &= 1 \\ y &= 2"),
        (Mode::Math, "arrow.r.long + #foo(1)"),
        (Mode::Markup, "#(1 + )"),
        (Mode::Markup, "#let x = \"unterminated"),
        (Mode::Markup, "/* never closed"),
        (Mode::Code, "{ /*c*/ a; /*tail*/ }"),
        (Mode::Markup, "$ x $"),
        (
            Mode::Markup,
            "- first\n  continuation\n  - nested\n- second\n",
        ),
        (Mode::Markup, "= Title <tag>\n"),
        (Mode::Math, "frac(a, b) + mat(delim: \"[\", 1, 2; 3, 4)"),
        (Mode::Math, "a / b + a/ b + a /b"),
        (Mode::Code, "let (a, (b, c)) = (1, (2, 3))"),
    ];
    let mut failures = Vec::new();
    for (mode, source) in cases {
        if let Err(error) = equivalence(source, mode) {
            failures.push(format!("{mode:?} {source:?}: {error}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differential cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn benchmark_corpora_are_equivalent_full_syntax_trees() {
    let mut failures = Vec::new();
    for workload in workloads() {
        if let Err(error) = equivalence(&workload.source, Mode::Markup) {
            failures.push(format!("{}: {error}", workload.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unicode_identifier_classes_match_the_oracle() {
    for value in 0..=0x10ffff {
        if let Some(ch) = char::from_u32(value) {
            assert_eq!(
                zsyn::is_id_start(ch),
                typst_syntax::is_id_start(ch),
                "XID_Start U+{value:04X}"
            );
            assert_eq!(
                zsyn::is_id_continue(ch),
                typst_syntax::is_id_continue(ch),
                "XID_Continue U+{value:04X}"
            );
        }
    }
}
