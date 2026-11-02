#!/usr/bin/env python3
"""Measure comparable executables without modifying or executing them.

Usage: python3 scripts/measure_size.py baseline=/path/to/bin candidate=/path/to/bin
gzip_bytes is the executable alone, with level 9 and a deterministic timestamp;
it is not the size of a signed release archive with documentation and licenses.
"""
import gzip
import hashlib
import json
from pathlib import Path
import sys


def main():
    results = []
    for argument in sys.argv[1:]:
        label, path = argument.split("=", 1)
        content = Path(path).read_bytes()
        results.append({
            "label": label,
            "bytes": len(content),
            "gzip_bytes": len(gzip.compress(content, compresslevel=9, mtime=0)),
            "sha256": hashlib.sha256(content).hexdigest(),
        })
    if not results:
        raise SystemExit("Expected at least one label=/path/to/executable")
    baseline = results[0]["bytes"]
    for result in results:
        result["reduction_percent"] = round(100 * (1 - result["bytes"] / baseline), 2)
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
