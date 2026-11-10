//! Oracle comparison and reproducible workloads, outside the published crate.
use zsyn::{Limits, Mode, OwnedTree, Tree};

pub const ORACLE_VERSION: &str = "0.15.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub kind: String,
    pub start: usize,
    pub end: usize,
    pub depth: usize,
}

pub fn canonical(tree: Tree<'_, '_>) -> Vec<Record> {
    fn visit(node: zsyn::NodeRef<'_, '_>, depth: usize, out: &mut Vec<Record>) {
        let span = node.span();
        out.push(Record {
            kind: format!("{:?}", node.kind()),
            start: span.start as usize,
            end: span.end as usize,
            depth,
        });
        for child in node.children() {
            visit(child, depth + 1, out);
        }
    }
    let mut result = Vec::with_capacity(tree.nodes().len());
    visit(tree.root(), 0, &mut result);
    result
}

pub fn parsed(source: &str, mode: Mode) -> OwnedTree {
    OwnedTree::parse(source, mode, Limits::default()).unwrap_or_else(|error| {
        panic!(
            "zsyn parse failed for {mode:?}, {} bytes: {error:?}",
            source.len()
        )
    })
}

#[cfg(feature = "oracle")]
pub fn oracle_parse(source: &str, mode: Mode) -> typst_syntax::SyntaxNode {
    match mode {
        Mode::Markup => typst_syntax::parse(source),
        Mode::Code => typst_syntax::parse_code(source),
        Mode::Math => typst_syntax::parse_math(source),
    }
}

#[cfg(feature = "oracle")]
pub fn oracle_canonical(tree: &typst_syntax::SyntaxNode) -> Vec<Record> {
    fn visit(node: &typst_syntax::SyntaxNode, start: usize, depth: usize, out: &mut Vec<Record>) {
        out.push(Record {
            kind: format!("{:?}", node.kind()),
            start,
            end: start + node.len(),
            depth,
        });
        let mut at = start;
        for child in node.children() {
            visit(child, at, depth + 1, out);
            at += child.len();
        }
    }
    let mut result = Vec::new();
    visit(tree, 0, 0, &mut result);
    result
}

#[cfg(feature = "oracle")]
pub fn equivalence(source: &str, mode: Mode) -> Result<usize, String> {
    let actual = parsed(source, mode);
    let expected = oracle_parse(source, mode);
    compare_records(&canonical(actual.as_tree()), &oracle_canonical(&expected))
}

pub fn compare_records(actual: &[Record], expected: &[Record]) -> Result<usize, String> {
    for index in 0..actual.len().max(expected.len()) {
        if actual.get(index) != expected.get(index) {
            return Err(format!(
                "node {index}: zsyn={:?}; typst={:?}; node counts={}/{}",
                actual.get(index),
                expected.get(index),
                actual.len(),
                expected.len()
            ));
        }
    }
    Ok(actual.len())
}

pub struct Workload {
    pub name: &'static str,
    pub source: String,
    pub edit_at: usize,
}

pub fn workloads() -> Vec<Workload> {
    let specs = [
        ("short", include_str!("../corpus/short.typ"), 1),
        ("large", include_str!("../corpus/large-unit.typ"), 2048),
        ("code", include_str!("../corpus/code-unit.typ"), 256),
        ("math", include_str!("../corpus/math-unit.typ"), 512),
        ("errors", include_str!("../corpus/errors-unit.typ"), 256),
    ];
    specs
        .into_iter()
        .map(|(name, unit, repeats)| {
            let source = unit.repeat(repeats);
            let middle = source.len() / 2;
            let edit_at = source[middle..]
                .find('a')
                .map(|x| middle + x)
                .or_else(|| source.find('x'))
                .unwrap_or(0);
            Workload {
                name,
                source,
                edit_at,
            }
        })
        .collect()
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
