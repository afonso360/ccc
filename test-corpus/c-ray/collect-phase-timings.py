#!/usr/bin/env python3

"""Run and validate one untimed C-Ray phase-instrumented compilation."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
from typing import Iterable


RESULT_FORMAT_VERSION = 3
PHASE_TIMING_SCHEMA_VERSION = 1
PHASE_METRICS = (
    "preprocessing",
    "parsing",
    "semantic_analysis",
    "ccc_ir_lowering",
    "ccc_ir_optimization",
    "codegen.total",
    "object_packaging",
    "pipeline",
)
PHASE_FIELDS = ("label", "metric", "value")
ARTIFACT_FIELDS = (
    "label",
    "canonical_object_bytes",
    "canonical_object_sha256",
    "instrumented_object_bytes",
    "instrumented_object_sha256",
    "objects_match",
)


class CollectionError(Exception):
    """A phase-timing command or its retained evidence was invalid."""


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", required=True)
    parser.add_argument("--canonical-object", required=True, type=Path)
    parser.add_argument("--instrumented-object", required=True, type=Path)
    parser.add_argument("--phase-sidecar", required=True, type=Path)
    parser.add_argument("--stdout", required=True, type=Path)
    parser.add_argument("--stderr", required=True, type=Path)
    parser.add_argument("--command-output", required=True, type=Path)
    parser.add_argument("--result-output", required=True, type=Path)
    parser.add_argument("--phase-results", required=True, type=Path)
    parser.add_argument("--artifact-results", required=True, type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    if arguments.command[:1] == ["--"]:
        arguments.command = arguments.command[1:]
    if not arguments.command:
        parser.error("a compiler command is required after --")
    if re.fullmatch(r"[a-z0-9][a-z0-9-]*", arguments.label) is None:
        parser.error("--label must contain only lowercase letters, digits, and hyphens")
    return arguments


def atomic_write_text(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.part.{os.getpid()}")
    temporary.write_text(contents, encoding="utf-8")
    temporary.replace(path)


def write_json(path: Path, value: object) -> None:
    atomic_write_text(
        path,
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_phase_timings(path: Path) -> list[tuple[str, str]]:
    try:
        with path.open(encoding="utf-8", newline="") as source:
            rows = list(csv.reader(source, delimiter="\t"))
    except (OSError, UnicodeError, csv.Error) as error:
        raise CollectionError(f"{path}: cannot read phase timings: {error}") from error

    values: dict[str, str] = {}
    order: list[str] = []
    for line_number, row in enumerate(rows, 1):
        if len(row) != 2:
            raise CollectionError(
                f"{path}:{line_number}: expected one tab-separated metric/value row"
            )
        metric, value = row
        if re.fullmatch(r"[a-z][a-z0-9_.]*", metric) is None:
            raise CollectionError(
                f"{path}:{line_number}: invalid phase-timing metric {metric!r}"
            )
        if metric in values:
            raise CollectionError(
                f"{path}:{line_number}: duplicate phase-timing metric {metric!r}"
            )
        if re.fullmatch(r"(0|[1-9][0-9]*)", value) is None:
            raise CollectionError(
                f"{path}:{line_number}: phase-timing metric {metric!r} "
                "is not canonical unsigned decimal"
            )
        values[metric] = value
        order.append(metric)

    expected_order = ("schema_version", *PHASE_METRICS)
    if not order or order[0] != "schema_version":
        raise CollectionError(
            f"{path}: first phase-timing metric must be schema_version"
        )
    if values["schema_version"] != str(PHASE_TIMING_SCHEMA_VERSION):
        raise CollectionError(
            f"{path}: unsupported phase-timing schema {values['schema_version']}"
        )
    if tuple(order) != expected_order:
        missing = [metric for metric in expected_order if metric not in values]
        unexpected = [metric for metric in order if metric not in expected_order]
        details = []
        if missing:
            details.append(f"missing {', '.join(missing)}")
        if unexpected:
            details.append(f"unexpected {', '.join(unexpected)}")
        if not details:
            details.append("metrics are out of schema order")
        raise CollectionError(
            f"{path}: invalid phase-timing schema: {'; '.join(details)}"
        )
    return [(metric, values[metric]) for metric in expected_order]


def read_normalized_rows(path: Path, fields: tuple[str, ...]) -> list[dict[str, str]]:
    if not path.exists():
        return []
    try:
        with path.open(encoding="utf-8", newline="") as source:
            reader = csv.DictReader(source, delimiter="\t")
            if tuple(reader.fieldnames or ()) != fields:
                raise CollectionError(
                    f"{path}: expected normalized header {'/'.join(fields)}"
                )
            rows = list(reader)
    except (OSError, UnicodeError, csv.Error) as error:
        raise CollectionError(f"{path}: cannot read normalized results: {error}") from error
    for line_number, row in enumerate(rows, 2):
        if None in row or any(row[field] is None for field in fields):
            raise CollectionError(f"{path}:{line_number}: malformed normalized row")
    return rows


def ensure_label_absent(
    path: Path, fields: tuple[str, ...], label: str
) -> list[dict[str, str]]:
    rows = read_normalized_rows(path, fields)
    if any(row["label"] == label for row in rows):
        raise CollectionError(f"{path}: duplicate phase-timing record for {label}")
    return rows


def render_rows(
    fields: tuple[str, ...], rows: Iterable[dict[str, object]]
) -> str:
    from io import StringIO

    output = StringIO(newline="")
    writer = csv.DictWriter(
        output, fieldnames=fields, delimiter="\t", lineterminator="\n"
    )
    writer.writeheader()
    writer.writerows(rows)
    return output.getvalue()


def main() -> int:
    arguments = parse_arguments()
    payload: dict[str, object] = {
        "format_version": RESULT_FORMAT_VERSION,
        "label": arguments.label,
        "command": arguments.command,
        "stdout": os.fspath(arguments.stdout),
        "stderr": os.fspath(arguments.stderr),
        "canonical_object": os.fspath(arguments.canonical_object),
        "instrumented_object": os.fspath(arguments.instrumented_object),
        "phase_timings": os.fspath(arguments.phase_sidecar),
        "timed": False,
        "executed": False,
    }

    try:
        phase_rows = ensure_label_absent(
            arguments.phase_results, PHASE_FIELDS, arguments.label
        )
        artifact_rows = ensure_label_absent(
            arguments.artifact_results, ARTIFACT_FIELDS, arguments.label
        )
        for path in (arguments.instrumented_object, arguments.phase_sidecar):
            if path.exists():
                raise CollectionError(
                    f"{path}: refusing to reuse stale phase-timing evidence"
                )

        atomic_write_text(
            arguments.command_output,
            "LC_ALL=C " + shlex.join(arguments.command) + "\n",
        )
        for path in (
            arguments.stdout,
            arguments.stderr,
            arguments.result_output,
            arguments.instrumented_object,
            arguments.phase_sidecar,
        ):
            path.parent.mkdir(parents=True, exist_ok=True)

        environment = os.environ.copy()
        environment["LC_ALL"] = "C"
        try:
            with arguments.stdout.open("wb") as standard_output, arguments.stderr.open(
                "wb"
            ) as standard_error:
                completed = subprocess.run(
                    arguments.command,
                    stdin=subprocess.DEVNULL,
                    stdout=standard_output,
                    stderr=standard_error,
                    env=environment,
                    check=False,
                )
            exit_status = completed.returncode
        except OSError as error:
            exit_status = 127
            with arguments.stderr.open("ab") as standard_error:
                standard_error.write(
                    f"could not execute {arguments.command[0]}: {error}\n".encode()
                )
        payload.update({"executed": True, "exit_status": exit_status})
        write_json(arguments.result_output, payload)
        if exit_status != 0:
            raise CollectionError(
                f"{arguments.label}: phase-timing compile failed with status "
                f"{exit_status}; see {arguments.stderr}"
            )
        if not arguments.canonical_object.is_file():
            raise CollectionError(
                f"{arguments.label}: measured object is missing: "
                f"{arguments.canonical_object}"
            )
        if not arguments.instrumented_object.is_file():
            raise CollectionError(
                f"{arguments.label}: phase-timing compile did not create "
                f"{arguments.instrumented_object}"
            )

        canonical_bytes = arguments.canonical_object.stat().st_size
        canonical_digest = sha256(arguments.canonical_object)
        instrumented_bytes = arguments.instrumented_object.stat().st_size
        instrumented_digest = sha256(arguments.instrumented_object)
        objects_match = (
            canonical_bytes == instrumented_bytes
            and canonical_digest == instrumented_digest
        )
        artifact_row: dict[str, object] = {
            "label": arguments.label,
            "canonical_object_bytes": canonical_bytes,
            "canonical_object_sha256": canonical_digest,
            "instrumented_object_bytes": instrumented_bytes,
            "instrumented_object_sha256": instrumented_digest,
            "objects_match": int(objects_match),
        }
        payload.update(artifact_row)
        write_json(arguments.result_output, payload)
        if not objects_match:
            raise CollectionError(
                f"{arguments.label}: phase-timing instrumentation changed the "
                f"measured object: canonical {canonical_bytes} bytes/"
                f"{canonical_digest}, instrumented {instrumented_bytes} bytes/"
                f"{instrumented_digest}"
            )
        if not arguments.phase_sidecar.is_file():
            raise CollectionError(
                f"{arguments.label}: phase-timing compile did not create "
                f"{arguments.phase_sidecar}"
            )

        parsed_timings = parse_phase_timings(arguments.phase_sidecar)
        artifact_rows.append({key: str(value) for key, value in artifact_row.items()})
        phase_rows.extend(
            {
                "label": arguments.label,
                "metric": metric,
                "value": value,
            }
            for metric, value in parsed_timings
        )
        # Both normalized tables remain untouched until the command, object
        # equivalence, and complete sidecar schema have all been validated.
        atomic_write_text(
            arguments.phase_results,
            render_rows(PHASE_FIELDS, phase_rows),
        )
        atomic_write_text(
            arguments.artifact_results,
            render_rows(ARTIFACT_FIELDS, artifact_rows),
        )
        payload["phase_timing_schema_version"] = PHASE_TIMING_SCHEMA_VERSION
        payload["phase_metrics"] = {
            metric: int(value)
            for metric, value in parsed_timings
            if metric != "schema_version"
        }
        write_json(arguments.result_output, payload)
    except (CollectionError, OSError, UnicodeError) as error:
        payload["validation_error"] = str(error)
        try:
            write_json(arguments.result_output, payload)
        except OSError:
            pass
        print(f"C-Ray phase-timing collection failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
