# Parsing and incremental-edit contract

## Representation

The parser writes a lossless concrete syntax tree into a caller-owned slice.
Each 24-byte `Node` contains its syntax kind, UTF-8 byte range, first child,
next sibling, and parent. All links are indices into the initialized arena.
Source bytes remain in the caller's `str`; syntax nodes do not own text.

Composite nodes are initially allocated after their children; a mathematical
attachment can append additional children later. Pratt expression parsing can
therefore wrap an existing operand in a binary expression, call, or attachment
without moving its descendants. Iteration follows explicit links, so walking
the tree does not consume a recursive call stack or allocate a traversal buffer.

Every child range lies inside its parent's range. Children cover a composite's
range in source order, and leaf ranges reconstruct the complete input, including
whitespace and comments. A recovered missing token has an empty range. The
independent test suite checks these invariants on valid and malformed documents.

## Lexing and parsing

The lexer reads bytes on the common ASCII path and decodes Unicode when a rule
needs a character property. Plain markup runs are scanned together instead of
constructing a token per word. Operators use their leading byte to select a
small candidate set. Unicode identifiers use generated XID tables; math symbols
retain Unicode 18 extended grapheme clusters, including combining marks, emoji
sequences, regional indicators, Hangul, and Indic conjuncts.

The parser controls transitions between markup, code, and math. Code expressions
use binding powers for precedence and associativity. Mathematical attachment,
fraction, root, and delimiter rules build explicit composite nodes. Cached
lookahead avoids repeatedly lexing the same token and intervening trivia.

The compatibility oracle is the separately pinned `typst-syntax 0.15.1` crate.
Tests compare full node kinds, byte ranges, and nesting depth. Unicode data is
generated from the [Unicode Character Database](https://www.unicode.org/Public/18.0.0/ucd/)
and the grapheme implementation is tested against the official
[UAX #29 conformance cases](https://www.unicode.org/Public/18.0.0/ucd/auxiliary/GraphemeBreakTest.txt).
The production parser has no dependency on the oracle or on a Unicode crate.

## Resource accounting

The source byte limit is checked before lexing. Node and diagnostic writes are
checked against both the supplied slice length and the corresponding explicit
budget. Recursive parser entry checks the configured depth budget and a hard
ceiling of 128 parser nesting levels. Node creation, parser progress, trivia
lookahead visits, and attachment reparenting consume a work budget. Lookahead
caches the remaining trivia interval and its last newline, so consuming one
comment does not scan the entire remaining comment sequence again. Attachment
updates retain the child tail and visit only newly appended children.
Returned parse metadata records observed depth and work, allowing later edits
to verify tighter limits before reusing an existing tree.

Recoverable syntax errors produce `Diagnostic` values and error nodes. Exhausted
resources produce `ParseError` instead of a partially valid tree. UTF-8 byte
input is validated before conversion to `str`. Public source and edit boundaries
are validated; internal links are constructed from initialized arena indices.

The optional `OwnedTree` estimates capacity from a bounded sample, reserves
memory fallibly, and grows an arena when the estimate is insufficient. Its
capacity is a convenience policy, not the core tree's memory requirement.
Applications that need exact memory control supply their own node slices.

## Incremental updates

Incremental parsing chooses the smallest region for which it can establish a
stable surrounding syntax context. It reports the chosen region and reuse count
in `EditStats` so an editor can inspect the actual work performed.

| Path | Validation | Work |
| --- | --- | --- |
| Stable token replacement | Token kind and complete token boundaries remain valid; relevant diagnostics do not intersect the edit | Reuse topology, update source and affected byte offsets |
| Closed subtree replacement | A code block, content block, or equation still parses as one complete node with an independent surrounding context | Parse the subtree and remap arena links while copying unchanged nodes |
| Complete parse | A delimiter, recovery state, ancestor pattern, or resource budget prevents local reuse | Run the same bounded parser used for an initial parse |

Reusing a tree does not make every update constant-time. Length-changing edits
shift later byte spans. Subtree splicing remaps arena indices and copies retained
nodes into the output arena. Owning edits can also move or allocate source text.
Equal-width stable token replacements can reuse the source and arena in place;
repeated editing at the same token uses a locality hint that is validated before
use. Benchmarks report first edits and repeated edits separately.

`reparse` validates that the declared edit explains the new source's unchanged
prefix and suffix. Owning edits validate ranges and complete all fallible work
before committing their result. A failed owning update preserves the old text
and tree. Incremental tests compare the entire result and all diagnostics with
a fresh complete parse, including delimiter changes, Unicode byte shifts,
malformed syntax, and context-sensitive destructuring.

## Evidence and limits

The core's zero allocation property follows from its API and dependency boundary:
its storage consists of caller slices and scalar state, and it imports only
`core`. `#![forbid(unsafe_code)]` is checked in every feature configuration. The
separate benchmark's allocator instrumentation measures allocation counts
without being linked into its timing build.

Exact differential equality is established for the checked-in test corpus and
the version recorded in its lockfile. It is a testable compatibility contract;
new language revisions require new oracle data and regression cases. Performance
claims are limited to the exact compiler flags, corpus fingerprints, hardware,
allocation mode, and edit patterns included with each measurement.
