#!/usr/bin/env python3
"""Regenerate zero-dependency identifier tables from the official Unicode data."""
from pathlib import Path
import argparse
import urllib.request

URL = "https://www.unicode.org/Public/18.0.0/ucd/DerivedCoreProperties.txt"


def generate(text):
    properties = {"XID_Start": [], "XID_Continue": []}
    for line in text.splitlines():
        entry = line.split("#", 1)[0].strip()
        if not entry or ";" not in entry:
            continue
        field, name = [part.strip() for part in entry.split(";", 1)]
        if name not in properties:
            continue
        bounds = field.split("..")
        start, end = int(bounds[0], 16), int(bounds[-1], 16)
        if end < 128:
            continue
        start = max(start, 128)
        ranges = properties[name]
        if ranges and ranges[-1][1] + 1 == start:
            ranges[-1] = (ranges[-1][0], end)
        else:
            ranges.append((start, end))
    lines = [
        "// Generated from Unicode 18.0.0 DerivedCoreProperties.txt (XID_Start/XID_Continue).",
        "// Data license: ../LICENSE-UNICODE. No runtime dependencies.",
        "", "pub(crate) fn contains(ranges: &[(u32, u32)], ch: char) -> bool {",
        "    let value = ch as u32;",
        "    ranges.binary_search_by(|&(start, end)| {",
        "        if end < value { core::cmp::Ordering::Less }",
        "        else if start > value { core::cmp::Ordering::Greater }",
        "        else { core::cmp::Ordering::Equal }",
        "    }).is_ok()", "}",
    ]
    for name, ranges in properties.items():
        lines.append(f"\npub(crate) static {name.upper()}: &[(u32, u32)] = &[")
        lines.extend(f"    (0x{start:x}, 0x{end:x})," for start, end in ranges)
        lines.append("];")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Previously downloaded official data")
    parser.add_argument("--output", type=Path, default=Path("src/unicode.rs"))
    args = parser.parse_args()
    data = args.input.read_text(encoding="utf-8") if args.input else urllib.request.urlopen(URL, timeout=60).read().decode()
    assert "DerivedCoreProperties-18.0.0.txt" in data, "unexpected Unicode version"
    args.output.write_text(generate(data), encoding="utf-8")
