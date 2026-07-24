#!/usr/bin/env python3

"""Collect stable, cross-format object-section size evidence."""

import argparse
import csv
import os
from pathlib import Path
import subprocess
import sys
from typing import Dict, Tuple


CATEGORIES = (
    "text",
    "read_only_data",
    "writable_data",
    "bss",
    "unwind",
    "debug",
    "other",
)


def is_named(name: str, exact: Tuple[str, ...], prefixes: Tuple[str, ...]) -> bool:
    return name in exact or any(name.startswith(prefix) for prefix in prefixes)


def category_for(section: str) -> str:
    """Map common ELF and Mach-O sections into portable comparison buckets."""
    if is_named(
        section,
        (
            ".eh_frame",
            ".gcc_except_table",
            ".ARM.exidx",
            ".ARM.extab",
            "__compact_unwind",
            "__eh_frame",
            "__gcc_except_tab",
            "__unwind_info",
        ),
        (
            ".eh_frame.",
            ".gcc_except_table.",
            ".ARM.exidx.",
            ".ARM.extab.",
        ),
    ):
        return "unwind"
    if is_named(
        section,
        (),
        (".debug", ".zdebug", "__debug", "__zdebug"),
    ):
        return "debug"
    if is_named(
        section,
        (".text", "__text"),
        (".text.",),
    ):
        return "text"
    if is_named(
        section,
        (
            ".rodata",
            ".srodata",
            "__const",
            "__cstring",
            "__literal4",
            "__literal8",
            "__literal16",
        ),
        (".rodata.", ".srodata."),
    ):
        return "read_only_data"
    if is_named(
        section,
        (
            ".data",
            ".got",
            ".got.plt",
            ".sdata",
            ".tdata",
            "__data",
            "__got",
            "__la_symbol_ptr",
            "__nl_symbol_ptr",
            "__thread_data",
            "__thread_vars",
        ),
        (".data.", ".sdata.", ".tdata."),
    ):
        return "writable_data"
    if is_named(
        section,
        (
            ".bss",
            ".sbss",
            ".tbss",
            "COMMON",
            "__bss",
            "__common",
            "__thread_bss",
        ),
        (".bss.", ".sbss.", ".tbss."),
    ):
        return "bss"
    return "other"


def parse_integer(value: str, context: str) -> int:
    try:
        return int(value, 0)
    except ValueError as error:
        raise ValueError(f"{context}: invalid integer {value!r}") from error


def parse_sysv_size(output: str, label: str) -> Dict[str, int]:
    """Parse GNU/LLVM `size --format=sysv` output and validate its total."""
    sections: Dict[str, int] = {}
    reported_total = None
    saw_heading = False
    for raw_line in output.splitlines():
        fields = raw_line.split()
        if not fields:
            continue
        if fields[0].lower() == "section":
            saw_heading = True
            continue
        if fields[0] == "Total":
            if len(fields) < 2:
                raise ValueError(f"{label}: malformed Total row")
            if reported_total is not None:
                raise ValueError(f"{label}: duplicate Total row")
            reported_total = parse_integer(fields[1], f"{label} Total")
            continue
        if not saw_heading:
            # Both GNU size and llvm-size prefix the table with `filename :`.
            continue
        if len(fields) < 2:
            raise ValueError(f"{label}: malformed section row {raw_line!r}")
        section = fields[0]
        size = parse_integer(fields[1], f"{label} section {section}")
        if size < 0:
            raise ValueError(f"{label}: negative size for section {section}")
        sections[section] = sections.get(section, 0) + size

    if not saw_heading:
        raise ValueError(f"{label}: SysV section heading is missing")
    if reported_total is None:
        raise ValueError(f"{label}: SysV Total row is missing")
    if not sections:
        raise ValueError(f"{label}: no object sections found")
    computed_total = sum(sections.values())
    if computed_total != reported_total:
        raise ValueError(
            f"{label}: section total is {computed_total}, "
            f"but size reported {reported_total}"
        )
    return sections


def collect(
    size_tool: Path, label: str, artifact: Path
) -> Tuple[Dict[str, int], str]:
    if not artifact.is_file():
        raise ValueError(f"{label}: artifact does not exist: {artifact}")
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"
    completed = subprocess.run(
        [str(size_tool), "--format=sysv", artifact.name],
        cwd=artifact.parent,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or f"exit status {completed.returncode}"
        raise ValueError(f"{label}: size tool failed: {detail}")
    return parse_sysv_size(completed.stdout, label), completed.stdout


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--size-tool", required=True, type=Path)
    parser.add_argument("--sections-output", required=True, type=Path)
    parser.add_argument("--totals-output", required=True, type=Path)
    parser.add_argument("--raw-output", required=True, type=Path)
    parser.add_argument(
        "--artifact",
        action="append",
        nargs=2,
        required=True,
        metavar=("LABEL", "PATH"),
    )
    arguments = parser.parse_args()

    try:
        size_tool = arguments.size_tool.resolve()
        if not size_tool.is_file():
            raise ValueError(f"size tool does not exist: {size_tool}")

        seen_labels = set()
        collected = []
        for label, path_string in arguments.artifact:
            if label in seen_labels:
                raise ValueError(f"duplicate artifact label: {label}")
            seen_labels.add(label)
            path = Path(path_string).resolve()
            sections, raw_output = collect(size_tool, label, path)
            collected.append((label, sections, raw_output))

        arguments.sections_output.parent.mkdir(parents=True, exist_ok=True)
        with arguments.sections_output.open(
            "w", encoding="utf-8", newline=""
        ) as output:
            writer = csv.DictWriter(
                output,
                fieldnames=("label", "section", "category", "bytes"),
                delimiter="\t",
                lineterminator="\n",
            )
            writer.writeheader()
            for label, sections, _ in collected:
                for section in sorted(sections):
                    writer.writerow(
                        {
                            "label": label,
                            "section": section,
                            "category": category_for(section),
                            "bytes": sections[section],
                        }
                    )

        total_fields = tuple(f"{category}_bytes" for category in CATEGORIES)
        with arguments.totals_output.open(
            "w", encoding="utf-8", newline=""
        ) as output:
            writer = csv.DictWriter(
                output,
                fieldnames=("label", *total_fields, "total_section_bytes"),
                delimiter="\t",
                lineterminator="\n",
            )
            writer.writeheader()
            for label, sections, _ in collected:
                totals = {category: 0 for category in CATEGORIES}
                for section, size in sections.items():
                    totals[category_for(section)] += size
                writer.writerow(
                    {
                        "label": label,
                        **{
                            f"{category}_bytes": totals[category]
                            for category in CATEGORIES
                        },
                        "total_section_bytes": sum(totals.values()),
                    }
                )

        with arguments.raw_output.open("w", encoding="utf-8") as output:
            for label, _, raw_output in collected:
                output.write(f"== {label} ==\n")
                output.write(raw_output)
                if raw_output and not raw_output.endswith("\n"):
                    output.write("\n")
    except (OSError, ValueError) as error:
        print(f"C-Ray object-section collection failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
