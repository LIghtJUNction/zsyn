use zsyn::{Limits, Mode, OwnedTree};

#[test]
fn long_prose_uses_storage_proportional_to_its_coalesced_syntax() {
    let paragraph =
        "A paragraph of ordinary words has many spaces but only a few syntax nodes.\n\n";
    let source = format!("#set text(size: 12pt)\n\n{}", paragraph.repeat(1024));
    let tree = OwnedTree::parse(&source, Mode::Markup, Limits::default()).unwrap();
    assert!(tree.as_tree().diagnostics().is_empty());
    assert_eq!(tree.as_tree().root().text(), source);
    assert!(tree.storage_capacity_bytes() < source.len() * 4);
}

#[test]
fn capacity_sampling_respects_utf8_boundaries() {
    let source = format!("{}🦀\n\nTail", "a".repeat(8191));
    let tree = OwnedTree::parse(&source, Mode::Markup, Limits::default()).unwrap();
    assert!(tree.as_tree().diagnostics().is_empty());
    assert_eq!(tree.as_tree().root().text(), source);
}
