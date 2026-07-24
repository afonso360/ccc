#!/usr/bin/env python3
"""Reject broken repository-local links in tracked Markdown files."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[2]
LINK = re.compile(r"!?\[[^\]\n]*\]\((?P<target>[^)\n]+)\)")
INLINE_CODE = re.compile(r"`[^`\n]*`")
FENCE = re.compile(r"^\s*(`{3,}|~{3,})")


def tracked_markdown() -> list[Path]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--", "*.md"], cwd=ROOT
    )
    return [
        ROOT / path.decode("utf-8")
        for path in output.split(b"\0")
        if path
    ]


def prose_only(text: str) -> str:
    lines: list[str] = []
    fence: str | None = None
    for line in text.splitlines(keepends=True):
        marker = FENCE.match(line)
        if marker:
            token = marker.group(1)[0]
            if fence is None:
                fence = token
            elif token == fence:
                fence = None
            lines.append("\n" if line.endswith("\n") else "")
            continue
        if fence is not None:
            lines.append("\n" if line.endswith("\n") else "")
            continue
        lines.append(INLINE_CODE.sub("", line))
    return "".join(lines)


def destination(raw: str) -> str:
    value = raw.strip()
    if value.startswith("<"):
        closing = value.find(">")
        return value[1:closing] if closing >= 0 else value
    return value.split(maxsplit=1)[0]


def main() -> int:
    failures: list[str] = []
    for markdown in tracked_markdown():
        text = prose_only(markdown.read_text(encoding="utf-8"))
        for match in LINK.finditer(text):
            target = destination(match.group("target"))
            if not target or target.startswith("#"):
                continue
            parsed = urlsplit(target)
            if parsed.scheme:
                if parsed.scheme in {"http", "https"} and not parsed.netloc:
                    line = text.count("\n", 0, match.start()) + 1
                    failures.append(
                        f"{markdown.relative_to(ROOT)}:{line}: malformed URL {target!r}"
                    )
                continue

            local = unquote(parsed.path)
            if not local:
                continue
            candidate = (
                ROOT / local.removeprefix("/")
                if local.startswith("/")
                else markdown.parent / local
            )
            if not candidate.exists():
                line = text.count("\n", 0, match.start()) + 1
                failures.append(
                    f"{markdown.relative_to(ROOT)}:{line}: missing link target {target!r}"
                )

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("tracked Markdown links are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
