#![cfg(feature = "oracle")]
use zsyn::Mode;
use zsyn_comparison::equivalence;

#[test]
fn mode_boundaries_literals_and_combinations() {
    let cases = [
        (Mode::Markup, "hello_world a*b foo_bar_baz"),
        (Mode::Markup, "#foo(1). #foo(1); text"),
        (Mode::Markup, "#! /usr/bin/env typst\nText"),
        (Mode::Markup, "@ref[Supplement *bold*]"),
        (Mode::Markup, "https://example.org/a(b). <a:b.c> @a:b.c"),
        (Mode::Markup, "```rust\n  let x = 1;\n  x\n```"),
        (Mode::Markup, "`` `a` ``"),
        (Mode::Markup, "*bold _nested_* and _italic *nested*_"),
        (Mode::Markup, "== Heading *bold* <tag>\nbody"),
        (
            Mode::Markup,
            "/ Term *bold*: _definition_\n  continuation\n",
        ),
        (Mode::Markup, "#let x = [a #foo[inner] b]\n#x"),
        (Mode::Markup, "\\u{110000} \\u{zz} \\u{}"),
        (Mode::Code, "0b1010; 0o755; 0xCAFE; 1_000; 1.0; 1e-2"),
        (Mode::Code, "1em; 1in; 1mm; 1cm; 1pt; 1deg; 1rad; 1fr; 1%"),
        (Mode::Code, "x not in y; x in y; x < y and y <= z"),
        (Mode::Code, "(\"key\": 1, ..rest)"),
        (
            Mode::Code,
            "import \"module.typ\" as module; import \"m.typ\": *",
        ),
        (Mode::Code, "show: template; set page(width: 20cm)"),
        (Mode::Code, "x => x + 1; let f(a, b: 2, ..sink) = a"),
        (Mode::Code, "{ let x = 1\n x + 2\n }"),
        (Mode::Code, "if true [yes] else if false [maybe] else [no]"),
        (Mode::Code, "0x; 1e; 12foo; \"\\q\""),
        (Mode::Math, "x!! + y!"),
        (Mode::Math, "x'''_a^b"),
        (Mode::Math, "a <= b != c -> d <=> e"),
        (Mode::Math, "[a + {b}]"),
        (Mode::Math, "#foo[bar] + #($x$)"),
        (Mode::Math, "mat(1, 2; 3, 4)"),
        (Mode::Math, "x_(i+1)^(n-1) / y"),
        (Mode::Markup, "*unfinished"),
        (Mode::Markup, "_unfinished"),
        (Mode::Markup, "$unfinished"),
        (Mode::Code, "_ => 1"),
        (Mode::Code, "(_, a) => a"),
        (Mode::Code, "((a,b), _) => a"),
        (Mode::Code, "let (a: (b,c)) = d"),
        (Mode::Code, "let 1 = 2"),
        (Mode::Math, "( x"),
        (Mode::Math, ".5"),
        (Mode::Code, "import \"f\": (q, x)"),
        (Mode::Code, "import \"f\": ( q,\n x, )"),
        (Mode::Markup, "#import \"f\": (q, x)\n#q"),
    ];
    let mut failures = Vec::new();
    for (mode, source) in cases {
        if let Err(error) = equivalence(source, mode) {
            failures.push(format!("{mode:?} {source:?}: {error}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} extended cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
