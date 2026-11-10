# zsyn

An independent, zero-dependency Rust parser for Typst markup, code, and math.
`zsyn` builds lossless syntax trees in caller-owned memory, exposes exact UTF-8
byte ranges, and supports incremental editing. Its core is always `#![no_std]`
and forbids unsafe code. The optional `alloc` feature provides an owning tree.

The minimum supported Rust version is **1.74.0**. The published crate has no
normal, build, development, optional, or target-specific third-party dependencies
and no build script. Linux, Windows, macOS, WebAssembly, AArch64, and a bare-metal
ARM target are covered by CI.

## Parse without an allocator

```toml
[dependencies]
zsyn = "0.1.0"
```

```rust
use zsyn::{parse, Diagnostic, Limits, Mode, Node};

let source = "= Greeting\nHello *world*! $x^2$";
let mut nodes = [Node::EMPTY; 256];
let mut diagnostics = [Diagnostic::EMPTY; 16];
let tree = parse(
    source,
    Mode::Markup,
    &mut nodes,
    &mut diagnostics,
    Limits::default(),
)?;

for node in tree.descendants() {
    // Every text slice borrows source. Offsets are UTF-8 bytes.
    let _ = (node.kind(), node.span(), node.text());
}
for diagnostic in tree.diagnostics() {
    let _ = (diagnostic.kind, diagnostic.span);
}
# Ok::<(), zsyn::ParseError>(())
```

`Mode::Markup` parses a document, `Mode::Code` parses an expression/statement
sequence, and `Mode::Math` parses mathematical notation without surrounding
dollar signs. Embedded mode changes are handled by the parser.

`parse_bytes` validates UTF-8 before parsing raw bytes. `Lexer` exposes individual
mode-specific tokens and errors. `parse_into` returns arena lengths and the root
index for applications that manage the tree lifetime themselves.

## Owning trees and edits

```toml
[dependencies]
zsyn = { version = "0.1.0", features = ["alloc"] }
```

```rust
use zsyn::{Limits, Mode, OwnedTree};

let mut document = OwnedTree::parse(
    "Hello ordinary world.", Mode::Markup, Limits::default(),
)?;
let stats = document.edit(8..9, "Z", Limits::default())?;
assert_eq!(document.source(), "Hello orZinary world.");
assert!(!stats.full_reparse);
let tree = document.as_tree();
assert_eq!(tree.root().text(), document.source());
# Ok::<(), zsyn::ParseError>(())
```

An edit uses a half-open byte range in the old source. Invalid ranges and
non-boundary UTF-8 offsets return `InvalidEdit`. Owning edits are transactional:
an error leaves the previous source and tree intact. `EditStats` reports the
reparsed range, reused nodes, and whether a complete parse was required.

The allocation-free `reparse` function accepts the old tree, the new source,
an `Edit`, and a separate caller-owned output arena. The caller owns source
editing and allocation policy.

## Tree representation

Each arena node occupies **24 bytes** and stores a syntax kind, two byte offsets,
and three integer links. Nodes borrow the original source instead of copying
token text. Children, parents, and depth-first descendants are available through
read-only `NodeRef` views. Iteration uses no heap and no recursive traversal stack.
Node indices are local to a tree revision; retain byte spans across updates.

The concrete syntax tree retains whitespace, comments, delimiters, and error
nodes. Leaf text reconstructs the original source exactly. Nodes represent
expressions and statements, including precedence, calls, closures, destructuring,
rules, conditionals, loops, imports, math attachments, fractions, and roots.
The crate handles parsing; evaluation and document layout belong to consumers.

Identifier classification uses checked-in Unicode 18 XID tables generated from
the Unicode Character Database. This keeps the published crate independent of
Unicode helper crates. The generator and its source URL are in
`scripts/generate-unicode.py`; the data license is included in the package.

## Recovery and resource limits

Incomplete input returns a tree with located diagnostics when the parser can
recover. Resource exhaustion returns `ParseError` without exposing a partially
initialized tree. Caller buffer sizes are hard limits in addition to `Limits`:

| Limit | Default |
| --- | ---: |
| Source bytes | 64 MiB |
| Nodes | 4,000,000 |
| Diagnostics | 4,096 |
| Recursive parser depth | 96, hard ceiling 128 |
| Parser work steps | 134,217,728 |

Set smaller limits for constrained devices. Depth counts parser frames rather
than delimiters, so one nested expression can consume several frames. Source
length is also restricted to the `u32` byte-span domain. The owning interface
uses fallible reservation; the core requires no allocator at all.

## Correctness and performance

The independent [`comparison/`](comparison) workspace pins **typst-syntax
0.15.1**. Its strict differential gate compares every node's kind, byte range,
and depth, including trivia and error nodes. Throughput comparisons run only
after the complete trees match for both the original and edited documents.
No benchmark substitutes lexical scanning for tree construction.

The suite includes fixed short, large, code-heavy, math-heavy, and malformed
documents; one-character edits; deterministic random Unicode input; boundary
conditions; invalid UTF-8; resource limits; and incremental/full-parse equality.
Unicode identifier classes are compared across every valid scalar value.

See [`comparison/README.md`](comparison/README.md) for the exact compiler flags,
corpus definitions, timing protocol, and memory accounting. Performance depends
on the source, CPU, allocation policy, and edit shape. Only measured, equivalent
workloads should be used for speed claims.

```sh
cargo test --all-features
cargo test --manifest-path comparison/Cargo.toml
cargo test --manifest-path comparison/Cargo.toml --no-default-features --test core
cargo +nightly miri test --manifest-path comparison/Cargo.toml --no-default-features --test core
RUSTFLAGS='-C target-cpu=x86-64' cargo run --release \
  --manifest-path comparison/Cargo.toml --bin benchmark
```

## CI and releases

CI runs the OS matrix, stable Rust, actual MSRV core tests, cross-target checks,
rustfmt, clippy, documentation tests, Miri, dependency/package checks,
`cargo package`, and `cargo publish --dry-run`.

To release, open **Actions → Release → Run workflow** on the default branch and
select `none`, `patch`, `minor`, or `major`. The same workflow calculates the
candidate version, runs the complete reusable CI workflow, updates the manifest
and lockfiles, pushes the version commit and annotated tag, publishes to
crates.io, verifies its package checksum, and creates the GitHub Release.

The release job reads `CARGO_REGISTRY_TOKEN` from GitHub Actions secrets. PRs,
forks, and non-default branches cannot publish. An existing registry version is
rejected. Release serialization and same-run recovery protect against duplicate
or partially completed publication; generated tags do not need to trigger
another workflow.

## License

Project code: MIT OR Apache-2.0. Unicode data: Unicode License V3.
