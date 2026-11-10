# Measured parser performance

The allocation-free parser exceeds **2× typst-syntax 0.15.1 on the code and error corpora in both recorded runs**. Short documents, prose, and math also improve, with smaller gains. The 3× full-parse target remains unmet. The owning convenience interface has separate measurements because it copies the source and allocates storage.

All results below use source commit [`3f0fce64cdcac746a547aec03d6e21b5f1974f37`](https://github.com/LIghtJUNction/zsyn/tree/3f0fce64cdcac746a547aec03d6e21b5f1974f37). They cover the five fixed documents in this repository, not every possible Typst program or processor. The first frozen-source timing run supplies the main tables; the second run is retained without selecting its fastest cells.

## Evidence and setup

The [joined results](../comparison/results/2026-11-10-3f0fce64/results.csv), [first timing run](../comparison/results/2026-11-10-3f0fce64/timing.csv), [repeat](../comparison/results/2026-11-10-3f0fce64/repeat.csv), [separate memory run](../comparison/results/2026-11-10-3f0fce64/memory.csv), and [environment record](../comparison/results/2026-11-10-3f0fce64/environment.json) contain the raw measurements. Both timing runs use the ordinary Rust allocator with no counting wrapper. A different build collects memory, and the merge script rejects instrumented timing rows or mismatched inputs, node counts, capacities, and reparse ranges.

The host was Linux x86-64 on an Intel Xeon Platinum 8573C, with the timing process pinned to CPU 8. Rust was **1.99.1**. Both implementations used `-C target-cpu=x86-64`, optimization level 3, thin LTO, and one codegen unit. No local Miri run shared the timed interval. This was a shared execution host; CPU affinity does not make absolute latency portable to another machine.

Every corpus passes an exact complete-CST comparison before timing: syntax kind, byte span, and preorder depth match at every node. That also fixes the source text and parent/child hierarchy. Trivia and error nodes are retained, with no token-only comparison or normalization. Canonical comparison records and validation trees are released before timing. Both states of each benchmark edit are checked against complete reparsing. The [methodology](../comparison/README.md) and [harness](../comparison/src/bin/benchmark.rs) specify the gate and workload construction.

Full parses and hot edits use eight warm-up calls, a batch calibrated to at least 15 ms, and the median of nine batches. `black_box` surrounds inputs and results. Full-parse result destruction occurs within the timed interval. For first edits, eight independent source trees are created outside timing for each of nine batches; no clone-induced copy-on-write cost is added to the reference.

## Complete parsing

**Run 1, complete syntax-tree construction.** Lower latency and higher speedup are better. The arena buffers are already allocated. The owning interface includes source ownership, buffer allocation, initialization, and destruction.

| Corpus | Input bytes | typst-syntax µs | zsyn arena µs | Arena speedup | zsyn owned µs | Owned speedup |
| --- | --- | --- | --- | --- | --- | --- |
| Short | 90 | 3.368 | 1.976 | 1.704× | 2.220 | 1.517× |
| Large | 604,160 | 9,141.122 | 5,894.961 | 1.551× | 6,436.449 | 1.420× |
| Code | 60,416 | 3,291.779 | 1,626.113 | 2.024× | 1,719.680 | 1.914× |
| Math | 64,000 | 5,033.140 | 2,606.147 | 1.931× | 2,730.119 | 1.844× |
| Errors | 24,832 | 1,013.616 | 471.239 | 2.151× | 523.898 | 1.935× |

**Full-document throughput from the same run.** One MiB is 1,048,576 bytes.

| Corpus | typst-syntax MiB/s | zsyn arena MiB/s | zsyn owned MiB/s |
| --- | --- | --- | --- |
| Short | 25.48 | 43.43 | 38.65 |
| Large | 63.03 | 97.74 | 89.52 |
| Code | 17.50 | 35.43 | 33.50 |
| Math | 12.13 | 23.42 | 22.36 |
| Errors | 23.36 | 50.25 | 45.20 |

**Corroborating run on the identical executable.** These are two observed medians per cell, not confidence intervals or a best-of-run aggregation.

| Corpus | Arena ratio: run 1 | Arena ratio: run 2 | Owned ratio: run 1 | Owned ratio: run 2 |
| --- | --- | --- | --- | --- |
| Short | 1.704× | 1.640× | 1.517× | 1.424× |
| Large | 1.551× | 1.614× | 1.420× | 1.519× |
| Code | 2.024× | 2.026× | 1.914× | 1.889× |
| Math | 1.931× | 1.798× | 1.844× | 1.766× |
| Errors | 2.151× | 2.064× | 1.935× | 1.984× |

The code corpus is just above the 2× threshold in both runs; the error corpus has a larger margin. The final math measurements remain below 2×. The owning interface remains below 2× across these final measurements. The complete parsing goal is therefore met in two of five measured arena workloads, with the remaining cases and the 3× goal reported explicitly.

## Incremental editing

These edit measurements exercise **a fixed, equal-width ASCII-letter replacement inside an existing token**. `edit-first` starts with a newly parsed tree and no previously edited-leaf cache. `edit-hot` repeatedly toggles the same character at the same location after warm-up. zsyn validates that the token kind and boundaries remain stable, then reuses the existing topology; the processed-byte column is the checked token span.

The reference is Typst's public `Source::edit` interface. It also maintains a line index and numbered source spans. zsyn maintains the documented syntax-tree state. The ratios below compare those complete public operations and include this metadata difference; they do not isolate Typst's AST splice cost. Structural insertions, deletions, delimiter edits, and recovery changes have separate correctness tests but are not performance claims from these rows.

**First edit, run 1.** Initial parsing is outside the timed interval.

| Corpus | typst Source first edit µs | zsyn first edit µs | Measured ratio | Typst reparsed bytes | zsyn processed bytes |
| --- | --- | --- | --- | --- | --- |
| Short | 1.056 | 0.096 | 11.056× | 36 | 8 |
| Large | 739.898 | 0.891 | 829.947× | 144 | 130 |
| Code | 83.029 | 0.410 | 202.634× | 44 | 6 |
| Math | 82.791 | 0.497 | 166.707× | 35 | 5 |
| Errors | 32.065 | 0.867 | 36.973× | 53 | 41 |

**Repeated edit at the same location, run 1.** This table describes cursor locality and the existing leaf cache.

| Corpus | typst Source hot edit µs | zsyn hot edit µs | Measured ratio |
| --- | --- | --- | --- |
| Short | 1.034 | 0.069 | 14.914× |
| Large | 655.783 | 0.356 | 1840.216× |
| Code | 58.729 | 0.065 | 902.159× |
| Math | 69.660 | 0.066 | 1058.165× |
| Errors | 24.890 | 0.576 | 43.232× |

All measured zsyn edits in this corpus perform zero heap allocations. The repeat file retains the corresponding second-run edit timings. Small edit latencies should be interpreted with their exact workload and host context; the full-parser table is the basis for the 2× parsing claim.

## Memory

**Requested heap bytes and caller-provided arena storage.** Memory comes from a separate instrumented executable. Allocation counts describe a single complete parse. A core parse makes zero allocation calls while using the arena shown below.

| Corpus | Nodes | Core initialized arena B | Core reserved arena B | Owned peak heap B | Typst peak heap B | Owned / Typst allocation calls |
| --- | --- | --- | --- | --- | --- | --- |
| Short | 55 | 1,320 | 3,048 | 2,694 | 2,983 | 3 / 28 |
| Large | 126,977 | 3,047,448 | 3,049,176 | 4,109,120 | 7,049,288 | 3 / 67600 |
| Code | 44,545 | 1,069,080 | 1,070,808 | 1,321,748 | 2,154,568 | 3 / 33798 |
| Math | 67,073 | 1,609,752 | 1,611,480 | 2,197,144 | 3,317,832 | 3 / 30733 |
| Errors | 13,825 | 341,016 | 342,744 | 473,176 | 758,088 | 3 / 9483 |

The node representation is 24 bytes. Text stays in the caller's UTF-8 source for the core interface. `Core initialized arena` counts retained nodes and diagnostics; `Core reserved arena` also includes the unused space supplied by the benchmark. Owned peak heap includes its source string and buffer capacities. Allocation counts and bytes exclude allocator metadata, rounding, stack, executable pages, and shared runtime state; these are not RSS measurements.

For the 604,160-byte prose document, the core retains 3,047,448 arena bytes. The owning parser peaks at 4,109,120 requested heap bytes, compared with 7,049,288 for the reference in this memory run. The owning parser uses three allocations on each of the five corpora. Its sparse-markup capacity estimate reduces unused space without adding repeated parse attempts on these workloads.

The separate giant-input regression parses **16 MiB of plain text into two caller-owned nodes and a zero-length diagnostic buffer**, and rejects the same input when its source-byte budget is lowered below that size. That is a resource test, distinct from the 604,160-byte large benchmark corpus. Miri uses a 4 KiB instance of that same regression to keep interpreter work bounded.

## Implementation choices and performance limits

The measured implementation uses byte-first ASCII scanning, coalesced text runs, a flat arena, cached lookahead, bounded recovery, and a source-sensitive capacity estimate for the owning interface. Trivia lookahead is reused across the scanned interval. Math attachment updates maintain their tail and avoid repeated traversal of an existing child chain. Work-budget tests cover these paths, so their linear behavior is checked alongside syntax equivalence.

There is **no explicit SIMD or unsafe implementation path** in these measurements. The crate forbids unsafe code and keeps the portable MSRV 1.74 implementation. Portable `core::simd` was rejected as unstable by the tested stable compiler; platform intrinsics were not added without a measured complete-parser benefit. Compiler-generated vector instructions in ordinary optimized Rust are not an explicit SIMD API in zsyn.

Node construction and parser control flow remain substantial work in these corpora: the code document produces 44,545 nodes from 60,416 source bytes, and the math document produces 67,073 nodes from 64,000 bytes. Each result retains a complete hierarchy with byte ranges and parent/sibling links. Plain-text scanning therefore accounts for only part of the complete operation. The owning interface also copies the source and prepares its buffers; its run-1 median exceeds the arena path by 93.568 µs for code, 123.972 µs for math, and 541.488 µs for the large document. These measurements quantify the combined public-interface cost. The report's timing resolution is the complete operation, so it provides no per-instruction attribution.

## Reproduction

Use the frozen implementation commit above and the commands in [comparison/README.md](../comparison/README.md). Retain both raw timing runs and the separate memory output. `comparison/merge-results.py` joins only compatible rows and does not choose the faster timing run. The manual performance workflow performs the same separate timing/memory builds and uploads the raw files.

The owning API is optional. The published crate's dependency tree remains empty, and the comparison workspace and its pinned reference dependency are excluded from the package. The core MSRV is 1.74; the oracle workspace runs on the current stable compiler recorded in the environment file.

## Claim and evidence review

| Claim | Evidence | Scope |
| --- | --- | --- |
| More than 2× on code and errors | Both final timing CSVs | Reused caller arena, fixed corpora, recorded host |
| Complete syntax results match | Harness gate and differential suites | Every retained kind, byte span, and hierarchy in tested sources |
| Core parsing allocates zero times | Separate allocation-count rows and caller-arena API | Caller-owned source and buffers |
| Stable token edits reuse syntax | Edit statistics, full-parse checks, and first/hot rows | Equal-width ASCII replacements used by this benchmark |
| Memory use is bounded | Explicit parser budgets and resource regressions | Requested limits and caller buffer capacities |

