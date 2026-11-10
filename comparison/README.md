# Reproducible parser comparison

This independent Cargo workspace checks complete syntax trees and measures parsing against **typst-syntax 0.15.1**. Its registry dependencies and `Cargo.lock` belong only to this workspace. The published `zsyn` package contains none of them.

## Correctness gate

`tests/differential.rs` compares every node's syntax kind, UTF-8 byte range, and depth in preorder. This preserves parent/child structure, trivia, delimiters, and error nodes. Node text follows directly from the shared source and matching ranges. There is no token-only comparison, ignored whitespace, or wrapper normalization. `benchmark` runs the same gate before timing each corpus and its edited form. A mismatch emits `equivalent=false` and withholds its speedup.

The gate establishes equivalence for the exact tested documents. It does not establish language-wide compatibility. The grammar cases cover markup, code, math, malformed input, and mode transitions; the deterministic fuzz tests independently check bounded execution, valid byte ranges, complete source retention, and incremental/full-parse consistency.

```sh
cargo test --manifest-path comparison/Cargo.toml
cargo test --manifest-path comparison/Cargo.toml --no-default-features --test core
cargo +nightly miri test --manifest-path comparison/Cargo.toml --no-default-features --test core
```

`core.rs` imports only `std` and `zsyn`, so the MSRV job can compile it directly with `rustc --test` without resolving a newer oracle dependency graph. The root crate retains an empty third-party dependency tree in every feature configuration.

## Fixed workloads

| Corpus | Source | Repetitions | Purpose |
| --- | --- | ---: | --- |
| short | `corpus/short.typ` | 1 | Small document latency and fixed overhead |
| large | `corpus/large-unit.typ` | 2,048 | Prose, Unicode, formatting, lists, math |
| code | `corpus/code-unit.typ` | 256 | Expressions, calls, bindings, rules, control flow |
| math | `corpus/math-unit.typ` | 512 | Attachments, fractions, functions, delimiters |
| errors | `corpus/errors-unit.typ` | 256 | Incomplete expressions and recovery |

Workload construction is deterministic and occurs before measurement. CSV rows include the exact byte count and an FNV-1a fingerprint so that compared runs can be matched. The fingerprint identifies the input and is not a security checksum.

## Measurement protocol

Run on an x86-64 machine with the exact release flags below. Record the compiler, CPU, OS, and commit alongside the output.

```sh
rustc --version --verbose
RUSTFLAGS='-C target-cpu=x86-64' cargo run --locked --release \
  --manifest-path comparison/Cargo.toml --bin benchmark > benchmark.csv
```

Each timing performs eight warm-up calls, calibrates a batch to at least 15 ms, and reports the median of nine equal-sized batches. `std::hint::black_box` surrounds inputs and results. Complete parsed results are dropped inside the timed region. The release profile uses optimization level 3, thin LTO, and one codegen unit for both implementations.

Full parsing reports two zsyn interfaces. `zsyn-arena` reuses caller-allocated node and diagnostic buffers, measuring the core parser's advertised allocation-free API. `zsyn-owned` includes source ownership and allocations on every call. Both build the same verified complete tree. The separate rows show the cost of owning the source and allocating storage; the arena row assumes the application already has reusable buffers.

Single-character editing alternates a fixed ASCII character and its replacement at the same byte position. Initial source/tree construction happens outside timing. Both edited states pass the complete-tree gate before measurement. The `reparsed_bytes` column records the affected region returned by each implementation. `typst-Source` also maintains Typst's source line index and numbered spans; zsyn maintains its documented syntax-tree state. These interfaces have different supporting metadata, which should be considered when interpreting edit latency.

The `mib_s` value for a full parse is source MiB divided by elapsed seconds. For an edit it expresses document-size throughput; **edit latency and reparsed bytes are the meaningful incremental metrics**, since the parser is expected to reuse most of the document.

## Memory accounting

A counting wrapper around Rust's `System` allocator records allocation calls, requested bytes, and peak live requested bytes during one operation. It is confined to this unpublished benchmark executable. All tree comparisons and formatting happen outside the measurement window. Allocation accounting is disabled during latency measurements except for a single atomic flag read in the wrapper.

`retained_arena_bytes` counts zsyn's initialized nodes and diagnostics. `reserved_bytes` includes unused storage capacity reported by the interface. The core arena's capacity is allocated before timing and is reported even though its parse operation performs no allocations. Typst rows expose allocator measurements; a zero in an arena-specific column means that metric is not applicable. Requested heap bytes exclude allocator bookkeeping, rounding, call stack, and executable pages; they are not process RSS. Incremental allocation counts and peaks describe additional allocations during the edit, not the already retained document.

## Baseline design

The pinned baseline's [node representation](https://docs.rs/crate/typst-syntax/0.15.1/source/src/node.rs) uses owned leaf strings and reference-counted inner nodes with child vectors. Its [source wrapper](https://docs.rs/crate/typst-syntax/0.15.1/source/src/source.rs) owns the text and line map. The [incremental parser](https://docs.rs/crate/typst-syntax/0.15.1/source/src/reparser.rs) reparses code/content blocks or eligible markup ranges, expanding the range when surrounding syntax must be reconsidered. These are implementation observations about version 0.15.1, not performance results.

zsyn tests whether a caller-owned flat arena and borrowed source spans reduce allocation and traversal costs. Any claimed acceleration must be read from a row whose complete-tree gate passed. Failure to reach a target speedup is a measurement result, not permission to relax the gate.

## Review criteria

- **Contribution:** the measured operations construct complete trees; tokenization alone is not a performance result.
- **Clarity:** latency, full-document throughput, requested heap memory, arena storage, and source metadata are reported separately.
- **Evidence:** every published ratio requires a passing complete-tree comparison and a fixed compiler configuration.
- **Coverage:** representative fixed corpora are supplemented by grammar cases, malformed inputs, Unicode, resource budgets, and repeated edits; results remain specific to the tested scope.
- **Design:** caller-buffer and owned interfaces are measured separately so allocation setup does not disappear from the report.
