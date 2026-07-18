#!/usr/bin/env python3

"""Validate and report the test-corpus applicability matrix."""

import ast
import os
from pathlib import Path
import re
import sys
from typing import Dict, List, Mapping, Sequence, Tuple


EXPECTED_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "riscv64-unknown-linux-gnu",
    "aarch64-apple-darwin",
)
ALLOWED_STATUSES = {"applicable", "inapplicable"}
ALLOWED_EVIDENCE_KINDS = {"execution", "parse-only"}
TABLE_RE = re.compile(r'^\[target_applicability\."([^"]+)"\]$')
STRING_VALUE_RE = re.compile(r'^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*("(?:[^"\\]|\\.)*")$')


class MatrixError(Exception):
    pass


def parse_catalog(path: Path) -> Tuple[Sequence[str], Sequence[str]]:
    values = {}
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise MatrixError(f"{path}:{line_number}: expected a key/value assignment")
        key, raw_value = (part.strip() for part in line.split("=", 1))
        if key in values:
            raise MatrixError(f"{path}:{line_number}: duplicate key {key!r}")
        try:
            values[key] = ast.literal_eval(raw_value)
        except (SyntaxError, ValueError) as error:
            raise MatrixError(f"{path}:{line_number}: invalid value for {key!r}: {error}") from error

    expected_keys = {"format_version", "enabled_targets", "corpora"}
    if set(values) != expected_keys:
        raise MatrixError(
            f"{path}: expected keys {sorted(expected_keys)}, found {sorted(values)}"
        )
    if values["format_version"] != 1:
        raise MatrixError(f"{path}: unsupported format_version {values['format_version']!r}")

    targets = values["enabled_targets"]
    corpora = values["corpora"]
    if not isinstance(targets, list) or not all(isinstance(item, str) for item in targets):
        raise MatrixError(f"{path}: enabled_targets must be a string array")
    if not isinstance(corpora, list) or not all(isinstance(item, str) for item in corpora):
        raise MatrixError(f"{path}: corpora must be a string array")
    if len(targets) != len(set(targets)):
        raise MatrixError(f"{path}: enabled_targets contains a duplicate")
    if len(corpora) != len(set(corpora)):
        raise MatrixError(f"{path}: corpora contains a duplicate")
    if tuple(targets) != EXPECTED_TARGETS:
        raise MatrixError(
            f"{path}: enabled_targets must be exactly {list(EXPECTED_TARGETS)!r}, found {targets!r}"
        )
    return targets, corpora


def decode_basic_string(path: Path, line_number: int, raw_value: str) -> str:
    try:
        value = ast.literal_eval(raw_value)
    except (SyntaxError, ValueError) as error:
        raise MatrixError(f"{path}:{line_number}: invalid basic string: {error}") from error
    if not isinstance(value, str):
        raise MatrixError(f"{path}:{line_number}: applicability values must be strings")
    return value


def parse_applicability(path: Path) -> Mapping[str, Mapping[str, str]]:
    entries: Dict[str, Dict[str, str]] = {}
    current_target = None
    in_applicability_table = False

    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw_line.strip()
        table_match = TABLE_RE.fullmatch(line)
        if table_match:
            current_target = table_match.group(1)
            in_applicability_table = True
            if current_target in entries:
                raise MatrixError(
                    f"{path}:{line_number}: duplicate applicability table for {current_target}"
                )
            entries[current_target] = {}
            continue
        if line.startswith("["):
            current_target = None
            in_applicability_table = False
            continue
        if not in_applicability_table or not line or line.startswith("#"):
            continue

        value_match = STRING_VALUE_RE.fullmatch(line)
        if value_match is None:
            raise MatrixError(
                f"{path}:{line_number}: applicability entries must be single-line basic strings"
            )
        key, raw_value = value_match.groups()
        if key in entries[current_target]:
            raise MatrixError(
                f"{path}:{line_number}: duplicate {key!r} for target {current_target}"
            )
        entries[current_target][key] = decode_basic_string(path, line_number, raw_value)

    return entries


def parse_top_level_string(path: Path, key: str) -> str:
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw_line.strip()
        if line.startswith("["):
            break
        if not line or line.startswith("#"):
            continue
        value_match = STRING_VALUE_RE.fullmatch(line)
        if value_match is None or value_match.group(1) != key:
            continue
        return decode_basic_string(path, line_number, value_match.group(2))
    return ""


def reject_blocked_execution_claims(
    manifest_path: Path, entries: Mapping[str, Mapping[str, str]]
) -> None:
    execution_status = parse_top_level_string(manifest_path, "execution_status")
    if not execution_status.startswith("blocked"):
        return
    claimed_targets = sorted(
        target
        for target, entry in entries.items()
        if entry.get("status") == "applicable"
        and entry.get("evidence_kind") == "execution"
    )
    if claimed_targets:
        raise MatrixError(
            f"{manifest_path}: execution_status {execution_status!r} cannot claim "
            f"execution evidence for {claimed_targets!r}"
        )


def validate_entry(
    corpus_directory: Path, target: str, entry: Mapping[str, str]
) -> Tuple[str, str, str, str]:
    status = entry.get("status", "")
    reason = entry.get("reason", "").strip()
    if status not in ALLOWED_STATUSES:
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: status must be one of "
            f"{sorted(ALLOWED_STATUSES)}, found {status!r}"
        )
    if not reason:
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: reason must be nonempty"
        )

    if status == "inapplicable":
        expected_keys = {"status", "reason"}
        if set(entry) != expected_keys:
            raise MatrixError(
                f"{corpus_directory / 'manifest.toml'}: {target}: inapplicable entries "
                f"must contain exactly {sorted(expected_keys)}, found {sorted(entry)}"
            )
        return status, "-", "-", reason

    evidence_kind = entry.get("evidence_kind", "")
    if evidence_kind not in ALLOWED_EVIDENCE_KINDS:
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: evidence_kind must be one of "
            f"{sorted(ALLOWED_EVIDENCE_KINDS)}, found {evidence_kind!r}"
        )

    if evidence_kind == "execution":
        expected_keys = {"status", "reason", "evidence_kind", "runner"}
        if set(entry) != expected_keys:
            raise MatrixError(
                f"{corpus_directory / 'manifest.toml'}: {target}: execution entries must "
                f"contain exactly {sorted(expected_keys)}, found {sorted(entry)}"
            )
        runner = entry.get("runner", "").strip()
        if not runner or Path(runner).is_absolute() or ".." in Path(runner).parts:
            raise MatrixError(
                f"{corpus_directory / 'manifest.toml'}: {target}: runner must be a nonempty "
                "corpus-relative path"
            )
        runner_path = corpus_directory / runner
        if not runner_path.is_file():
            raise MatrixError(
                f"{corpus_directory / 'manifest.toml'}: {target}: runner does not exist: {runner}"
            )
        if not os.access(runner_path, os.X_OK):
            raise MatrixError(
                f"{corpus_directory / 'manifest.toml'}: {target}: runner is not executable: {runner}"
            )
        return status, evidence_kind, runner, reason

    expected_keys = {"status", "reason", "evidence_kind", "entrypoint"}
    if set(entry) != expected_keys:
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: parse-only entries must "
            f"contain exactly {sorted(expected_keys)}, found {sorted(entry)}"
        )
    entrypoint = entry.get("entrypoint", "").strip()
    if not entrypoint or Path(entrypoint).is_absolute() or ".." in Path(entrypoint).parts:
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: entrypoint must be a nonempty "
            "corpus-relative path"
        )
    if not (corpus_directory / entrypoint).is_file():
        raise MatrixError(
            f"{corpus_directory / 'manifest.toml'}: {target}: entrypoint does not exist: "
            f"{entrypoint}"
        )
    return status, evidence_kind, entrypoint, reason


def render_table(rows: Sequence[Tuple[str, str, str, str, str, str]]) -> None:
    headers = ("corpus", "target", "status", "evidence", "runner/entrypoint", "reason")
    widths = [len(header) for header in headers]
    for row in rows:
        for index, value in enumerate(row):
            widths[index] = max(widths[index], len(value))

    print("target applicability report")
    print("  ".join(header.ljust(widths[index]) for index, header in enumerate(headers)))
    print("  ".join("-" * width for width in widths))
    for row in rows:
        print("  ".join(value.ljust(widths[index]) for index, value in enumerate(row)))


def main() -> int:
    arguments = sys.argv[1:]
    if not arguments:
        root = Path(__file__).resolve().parent
    elif len(arguments) == 2 and arguments[0] == "--root":
        root = Path(arguments[1]).resolve()
    else:
        raise MatrixError("usage: report-target-applicability.py [--root DIRECTORY]")
    catalog_path = root / "target-applicability.toml"
    targets, corpora = parse_catalog(catalog_path)

    discovered = sorted(
        path.parent.name for path in root.glob("*/manifest.toml") if path.is_file()
    )
    if sorted(corpora) != discovered:
        raise MatrixError(
            f"{catalog_path}: corpora must match discovered manifests exactly; "
            f"catalog={sorted(corpora)!r}, discovered={discovered!r}"
        )

    rows: List[Tuple[str, str, str, str, str, str]] = []
    applicable_counts = {target: 0 for target in targets}
    execution_counts = {target: 0 for target in targets}
    parse_counts = {target: 0 for target in targets}
    inapplicable_counts = {target: 0 for target in targets}

    for corpus in corpora:
        corpus_directory = root / corpus
        manifest_path = corpus_directory / "manifest.toml"
        entries = parse_applicability(manifest_path)
        reject_blocked_execution_claims(manifest_path, entries)
        if set(entries) != set(targets):
            missing = sorted(set(targets) - set(entries))
            extra = sorted(set(entries) - set(targets))
            raise MatrixError(
                f"{manifest_path}: applicability target set mismatch; "
                f"missing={missing!r}, extra={extra!r}"
            )

        for target in targets:
            status, evidence_kind, artifact, reason = validate_entry(
                corpus_directory, target, entries[target]
            )
            rows.append((corpus, target, status, evidence_kind, artifact, reason))
            if status == "applicable":
                applicable_counts[target] += 1
                if evidence_kind == "execution":
                    execution_counts[target] += 1
                else:
                    parse_counts[target] += 1
            else:
                inapplicable_counts[target] += 1

    empty_targets = [target for target, count in applicable_counts.items() if count == 0]
    if empty_targets:
        raise MatrixError(
            "enabled targets must each have applicable evidence; empty targets="
            f"{empty_targets!r}"
        )
    execution_empty_targets = [
        target for target, count in execution_counts.items() if count == 0
    ]
    if execution_empty_targets:
        raise MatrixError(
            "enabled targets must each have execution evidence; empty targets="
            f"{execution_empty_targets!r}"
        )

    render_table(rows)
    print("summary")
    for target in targets:
        print(
            f"{target}: applicable={applicable_counts[target]} "
            f"execution={execution_counts[target]} parse-only={parse_counts[target]} "
            f"inapplicable={inapplicable_counts[target]}"
        )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (MatrixError, OSError) as error:
        print(f"target applicability validation failed: {error}", file=sys.stderr)
        sys.exit(1)
