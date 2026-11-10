#!/usr/bin/env python3
"""Join uninstrumented timings with separately collected allocation statistics."""
import argparse
import csv
import math
from pathlib import Path


def read(path):
    with path.open(newline="", encoding="utf-8") as stream:
        rows = list(csv.DictReader(stream))
    result = {}
    for row in rows:
        key = tuple(row[name] for name in ("workload", "operation", "engine"))
        if key in result:
            raise ValueError(f"duplicate measurement in {path}: {key}")
        if row["equivalent"] != "true":
            raise ValueError(f"complete-tree gate failed in {path}: {key}")
        result[key] = row
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("timing", type=Path)
    parser.add_argument("memory", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    timing, memory = read(args.timing), read(args.memory)
    expected = {
        (workload, operation, engine)
        for workload in ("short", "large", "code", "math", "errors")
        for operation, engines in (
            ("full", ("typst", "zsyn-owned", "zsyn-arena")),
            ("edit-first", ("typst-Source", "zsyn-OwnedTree")),
            ("edit-hot", ("typst-Source", "zsyn-OwnedTree")),
        )
        for engine in engines
    }
    if set(timing) != expected or set(memory) != expected:
        raise ValueError("both files must contain all 35 validated workload/operation rows")
    shared = ("bytes", "nodes", "reparsed_bytes", "retained_arena_bytes", "reserved_bytes", "source_fnv1a")
    allocation_fields = ("heap_allocations", "allocated_bytes", "peak_live_bytes")
    output = []
    for key, timed in timing.items():
        measured = memory[key]
        if timed["allocation_instrumented"] != "false":
            raise ValueError(f"timing used an instrumented allocator: {key}")
        if measured["allocation_instrumented"] != "true":
            raise ValueError(f"memory run lacks allocator instrumentation: {key}")
        for field in shared:
            if timed[field] != measured[field]:
                raise ValueError(f"run mismatch at {key}: {field}")
        for field in ("median_ns", "mib_s", "speedup_vs_typst"):
            if not math.isfinite(float(timed[field])) or float(timed[field]) <= 0:
                raise ValueError(f"invalid timing at {key}: {field}")
        row = dict(timed)
        row["allocation_instrumented"] = "false"
        for field in allocation_fields:
            if int(measured[field]) < 0:
                raise ValueError(f"invalid allocation count at {key}: {field}")
            row[field] = measured[field]
        row["memory_from_separate_run"] = "true"
        output.append(row)
    with args.output.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(output[0]))
        writer.writeheader()
        writer.writerows(output)


if __name__ == "__main__":
    main()
