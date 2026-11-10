#![cfg(feature = "oracle")]
use zsyn::Mode;
use zsyn_comparison::equivalence;

#[test]
fn upstream_syntax_fixture_trees_match() {
    let fixtures = [
        (
            "backtracking",
            include_str!("../upstream/syntax/backtracking.typ"),
        ),
        ("code", include_str!("../upstream/syntax/code.typ")),
        ("comment", include_str!("../upstream/syntax/comment.typ")),
        ("depth", include_str!("../upstream/syntax/depth.typ")),
        ("escape", include_str!("../upstream/syntax/escape.typ")),
        ("newlines", include_str!("../upstream/syntax/newlines.typ")),
        ("numbers", include_str!("../upstream/syntax/numbers.typ")),
        ("panics", include_str!("../upstream/syntax/panics.typ")),
        ("shebang", include_str!("../upstream/syntax/shebang.typ")),
        (
            "shorthand",
            include_str!("../upstream/syntax/shorthand.typ"),
        ),
    ];
    let mut failures = Vec::new();
    let mut checked = 0;
    for (file, source) in fixtures {
        let mut name = "preamble";
        let mut snippet = String::new();
        for line in source
            .split_inclusive('\n')
            .chain(std::iter::once("--- end ---"))
        {
            if line.starts_with("--- ") {
                if name != "preamble" {
                    checked += 1;
                    if let Err(error) = equivalence(&snippet, Mode::Markup) {
                        failures.push(format!("{file}/{name}: {error}"));
                    }
                }
                name = line.trim();
                snippet.clear();
            } else {
                snippet.push_str(line);
            }
        }
    }
    assert!(checked > 20, "fixture sections must be discovered");
    assert!(
        failures.is_empty(),
        "{}/{} upstream syntax sections failed:\n{}",
        failures.len(),
        checked,
        failures.join("\n")
    );
}

#[test]
fn recovered_assignment_keeps_every_node_and_source_byte() {
    let fixture = include_str!("../upstream/syntax/panics.typ");
    let source = fixture
        .split_once("--- issue-4571-panic-when-compiling-invalid-file eval ---\n")
        .unwrap()
        .1;
    let owned = zsyn_comparison::parsed(source, Mode::Markup);
    let tree = owned.as_tree();
    assert_eq!(tree.descendants().count(), tree.nodes().len());
    let rebuilt: String = tree
        .descendants()
        .filter(|n| n.is_leaf())
        .map(|n| n.text())
        .collect();
    assert_eq!(rebuilt, source);
    let error = zsyn::OwnedTree::parse(
        source,
        Mode::Markup,
        zsyn::Limits {
            max_steps: 32,
            ..zsyn::Limits::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, zsyn::ErrorKind::WorkLimit);
}
