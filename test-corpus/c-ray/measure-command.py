#!/usr/bin/env python3

"""Run one command and retain portable raw timing and peak-RSS evidence."""

import argparse
import csv
import json
import os
from pathlib import Path
import platform
import resource
import subprocess
import sys
import time


FIELDS = (
    "stage",
    "label",
    "iteration",
    "wall_seconds",
    "user_seconds",
    "system_seconds",
    "peak_rss_bytes",
    "exit_status",
)


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--iteration", required=True, type=int)
    parser.add_argument("--json", required=True, type=Path)
    parser.add_argument("--results", required=True, type=Path)
    parser.add_argument("--stdout", required=True, type=Path)
    parser.add_argument("--stderr", required=True, type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    if arguments.command[:1] == ["--"]:
        arguments.command = arguments.command[1:]
    if not arguments.command:
        parser.error("a command is required after --")
    if arguments.iteration < 0:
        parser.error("--iteration must be nonnegative")
    return arguments


def normalized_peak_rss_bytes(usage: resource.struct_rusage) -> int:
    # POSIX leaves ru_maxrss units implementation-defined. Darwin reports
    # bytes; Linux and the other supported Unix runners report KiB.
    if platform.system() == "Darwin":
        return int(usage.ru_maxrss)
    return int(usage.ru_maxrss) * 1024


def append_result(path: Path, result: dict) -> None:
    existed = path.exists()
    with path.open("a", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=FIELDS, delimiter="\t")
        if not existed:
            writer.writeheader()
        writer.writerow({field: result[field] for field in FIELDS})


def main() -> int:
    arguments = parse_arguments()
    for path in (arguments.json, arguments.results, arguments.stdout, arguments.stderr):
        path.parent.mkdir(parents=True, exist_ok=True)

    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.monotonic_ns()
    with arguments.stdout.open("wb") as standard_output, arguments.stderr.open(
        "wb"
    ) as standard_error:
        try:
            completed = subprocess.run(
                arguments.command,
                stdin=subprocess.DEVNULL,
                stdout=standard_output,
                stderr=standard_error,
                check=False,
            )
            exit_status = completed.returncode
        except OSError as error:
            print(f"could not execute {arguments.command[0]}: {error}", file=sys.stderr)
            exit_status = 127
    elapsed_ns = time.monotonic_ns() - started
    after = resource.getrusage(resource.RUSAGE_CHILDREN)

    result = {
        "format_version": 1,
        "stage": arguments.stage,
        "label": arguments.label,
        "iteration": arguments.iteration,
        "wall_seconds": elapsed_ns / 1_000_000_000,
        "user_seconds": after.ru_utime - before.ru_utime,
        "system_seconds": after.ru_stime - before.ru_stime,
        "peak_rss_bytes": normalized_peak_rss_bytes(after),
        "exit_status": exit_status,
        "command": arguments.command,
        "stdout": os.fspath(arguments.stdout),
        "stderr": os.fspath(arguments.stderr),
    }
    temporary_json = arguments.json.with_name(arguments.json.name + f".part.{os.getpid()}")
    temporary_json.write_text(
        json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    temporary_json.replace(arguments.json)
    append_result(arguments.results, result)
    return exit_status if 0 <= exit_status <= 255 else 1


if __name__ == "__main__":
    sys.exit(main())
