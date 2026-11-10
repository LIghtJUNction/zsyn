#!/usr/bin/env python3
"""Generate Unicode 18 UAX #29 property ranges from the Unicode Consortium."""
import argparse
from pathlib import Path
import urllib.request

BASE = "https://www.unicode.org/Public/18.0.0/ucd/"
FILES = ("auxiliary/GraphemeBreakProperty.txt", "emoji/emoji-data.txt", "DerivedCoreProperties.txt")
USED = {"Prepend", "Control", "Extend", "SpacingMark", "L", "V", "T", "Extended_Pictographic", "InCB_Linker", "InCB_Consonant", "InCB_Extend"}


def generate(documents):
    properties = {}
    for name, text in documents:
        for line in text.splitlines():
            data = line.split("#", 1)[0].strip()
            if not data:
                continue
            parts = [part.strip() for part in data.split(";")]
            field, prop = parts[:2]
            if name.endswith("emoji-data.txt") and prop != "Extended_Pictographic":
                continue
            if name.endswith("DerivedCoreProperties.txt"):
                if prop != "InCB":
                    continue
                prop = "InCB_" + parts[2]
            if prop not in USED:
                continue
            bounds = field.split("..")
            properties.setdefault(prop, []).append((int(bounds[0], 16), int(bounds[-1], 16)))
    assert set(properties) == USED
    lines = ["// Unicode 18.0.0 grapheme boundary data. See LICENSE-UNICODE.",
             "// Regenerate using scripts/generate-graphemes.py."]
    for prop, ranges in properties.items():
        merged = []
        for start, end in sorted(ranges):
            if merged and merged[-1][1] + 1 == start:
                merged[-1] = (merged[-1][0], end)
            else:
                merged.append((start, end))
        lines.append(f"pub(crate) static {prop.upper()}: &[(u32, u32)] = &[")
        lines.extend(f"    (0x{start:x}, 0x{end:x})," for start, end in merged)
        lines.append("];")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, help="Directory with official data files")
    parser.add_argument("--output", type=Path, default=Path("src/grapheme_data.rs"))
    args = parser.parse_args()
    documents = []
    for name in FILES:
        text = ((args.data_dir / Path(name).name).read_text(encoding="utf-8") if args.data_dir
                else urllib.request.urlopen(BASE + name, timeout=60).read().decode())
        assert "18.0.0" in text, f"unexpected Unicode version in {name}"
        documents.append((name, text))
    args.output.write_text(generate(documents), encoding="utf-8")
