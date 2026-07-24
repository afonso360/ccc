#!/usr/bin/env python3
"""Check whitespace invariants across tracked text files."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def tracked_files() -> list[Path]:
    output = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT)
    return [
        ROOT / path.decode("utf-8")
        for path in output.split(b"\0")
        if path
    ]


def main() -> int:
    failures: list[str] = []
    for path in tracked_files():
        data = path.read_bytes()
        if b"\0" in data:
            continue
        relative = path.relative_to(ROOT)
        for number, line in enumerate(data.splitlines(), 1):
            if line.endswith((b" ", b"\t")):
                failures.append(f"{relative}:{number}: trailing whitespace")
        if data and not data.endswith(b"\n"):
            failures.append(f"{relative}: missing final newline")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("tracked text files satisfy whitespace policy")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
