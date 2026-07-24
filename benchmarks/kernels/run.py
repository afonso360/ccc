#!/usr/bin/env python3

"""Compile, link, validate, and measure defined-behavior CCC kernels."""

from __future__ import annotations

import argparse
import csv
from contextlib import ExitStack
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import resource
import shutil
import statistics
import subprocess
import sys
import time
import tomllib
from typing import Iterable


FORMAT_VERSION = 2
MANIFEST_SCHEMA_VERSION = 1
CODEGEN_STATS_SCHEMA_VERSION = 2
PROFILE_FLAGS = {
    "O0": "-O0",
    "O1": "-O1",
    "O2": "-O2",
    "O3": "-O3",
    "Os": "-Os",
    "Oz": "-Oz",
}
REQUIRED_METRICS = (
    "post_inline_ir.functions",
    "post_inline_ir.blocks",
    "post_inline_ir.values",
    "post_inline_ir.instructions",
    "post_inline_ir.call_instructions",
    "primary_object.file_bytes",
    "primary_object.text_bytes",
)
BUILD_FIELDS = (
    "benchmark",
    "profile",
    "stage",
    "phase",
    "iteration",
    "wall_seconds",
    "user_seconds",
    "system_seconds",
    "peak_rss_bytes",
    "minor_page_faults",
    "major_page_faults",
    "voluntary_context_switches",
    "involuntary_context_switches",
    "artifact_kind",
    "artifact_bytes",
    "artifact_sha256",
    "exit_status",
)
RUN_FIELDS = (
    "benchmark",
    "profile",
    "execution_kind",
    "phase",
    "iteration",
    "wall_seconds",
    "user_seconds",
    "system_seconds",
    "peak_rss_bytes",
    "minor_page_faults",
    "major_page_faults",
    "voluntary_context_switches",
    "involuntary_context_switches",
    "exit_status",
)
STATS_FIELDS = (
    "benchmark",
    "family",
    "profile",
    "metric",
    "value",
)
ARTIFACT_FIELDS = (
    "benchmark",
    "profile",
    "kind",
    "path",
    "file_bytes",
    "sha256",
)
SUMMARY_FIELDS = (
    "benchmark",
    "family",
    "profile",
    "target",
    "mode",
    "execution_kind",
    "performance_comparable",
    "work_unit",
    "work_count",
    "compile_samples",
    "compile_median_wall_seconds",
    "compile_min_wall_seconds",
    "compile_max_wall_seconds",
    "compile_median_peak_rss_bytes",
    "link_wall_seconds",
    "validation_wall_seconds",
    "runtime_samples",
    "runtime_median_wall_seconds",
    "runtime_min_wall_seconds",
    "runtime_max_wall_seconds",
    "runtime_median_peak_rss_bytes",
    "runtime_ns_per_work_unit",
    "post_inline_ir.functions",
    "post_inline_ir.blocks",
    "post_inline_ir.values",
    "post_inline_ir.instructions",
    "post_inline_ir.call_instructions",
    "primary_object.file_bytes",
    "primary_object.text_bytes",
    "final_object.file_bytes",
    "final_object.sha256",
    "executable.file_bytes",
    "executable.sha256",
)


class BenchmarkError(Exception):
    """An actionable benchmark setup or execution failure."""


@dataclass(frozen=True)
class Case:
    name: str
    family: str
    source: Path
    work_unit: str
    work_count: int
    expected_result: str
    expected_functions: int
    expected_calls: dict[str, int]


@dataclass(frozen=True)
class BuildInvocation:
    case: Case
    profile: str
    stage: str
    phase: str
    iteration: int
    timing: dict[str, object]
    artifact_kind: str
    artifact_path: Path


@dataclass(frozen=True)
class RunInvocation:
    case: Case
    profile: str
    execution_kind: str
    phase: str
    iteration: int
    timing: dict[str, object]


@dataclass(frozen=True)
class StatsRecord:
    case: Case
    profile: str
    stats: dict[str, int]
    order: tuple[str, ...]


@dataclass(frozen=True)
class Artifact:
    case: Case
    profile: str
    kind: str
    path: Path
    file_bytes: int
    digest: str


def comma_separated(
    value: str,
    *,
    label: str,
    allowed: Iterable[str] | None = None,
) -> list[str]:
    values = value.split(",")
    if not values or any(not item for item in values):
        raise BenchmarkError(f"{label} must be a nonempty comma-separated list")
    if len(set(values)) != len(values):
        raise BenchmarkError(f"{label} contains a duplicate value")
    if allowed is not None:
        allowed_values = set(allowed)
        unknown = [item for item in values if item not in allowed_values]
        if unknown:
            raise BenchmarkError(
                f"{label} contains unsupported value(s): {', '.join(unknown)}"
            )
    return values


def nonnegative(value: str) -> int:
    if not value.isdecimal():
        raise argparse.ArgumentTypeError("must be a nonnegative integer")
    return int(value)


def positive(value: str) -> int:
    parsed = nonnegative(value)
    if parsed == 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def parse_arguments(case_names: tuple[str, ...]) -> argparse.Namespace:
    benchmark_directory = Path(__file__).resolve().parent
    repository = benchmark_directory.parents[1]
    parser = argparse.ArgumentParser(
        description=("Compile and measure self-validating, fixed-work CCC kernels.")
    )
    parser.add_argument(
        "--ccc",
        default=os.fspath(repository / "target" / "release" / "ccc"),
        help="CCC executable (default: target/release/ccc)",
    )
    parser.add_argument(
        "--output",
        required=True,
        type=Path,
        help="new or empty result directory",
    )
    parser.add_argument(
        "--mode",
        choices=("object", "correctness", "performance"),
        default="correctness",
        help="evidence mode (default: correctness)",
    )
    parser.add_argument(
        "--profiles",
        default="O0,O2,Oz",
        help="comma-separated optimization profiles (default: O0,O2,Oz)",
    )
    parser.add_argument(
        "--cases",
        default=",".join(case_names),
        help="comma-separated kernel names (default: all)",
    )
    parser.add_argument("--target", help="optional enabled CCC target triple")
    parser.add_argument(
        "--ccc-arg",
        action="append",
        default=[],
        help=(
            "extra CCC target/configuration argument; repeat as needed "
            "(use --ccc-arg=VALUE for values beginning with '-')"
        ),
    )
    parser.add_argument(
        "--runner",
        help="cross-target correctness runner executable",
    )
    parser.add_argument(
        "--runner-arg",
        action="append",
        default=[],
        help=(
            "argument placed before the executable in the runner command; "
            "repeat as needed"
        ),
    )
    parser.add_argument(
        "--compile-warmups",
        type=nonnegative,
        default=0,
        help="compiler warmups per case/profile, excluded from summary (default: 0)",
    )
    parser.add_argument(
        "--compile-samples",
        type=positive,
        default=1,
        help="measured object compilations per case/profile (default: 1)",
    )
    parser.add_argument(
        "--run-warmups",
        type=nonnegative,
        default=1,
        help="runtime warmups in performance mode (default: 1)",
    )
    parser.add_argument(
        "--run-samples",
        type=positive,
        default=5,
        help="measured executions in performance mode (default: 5)",
    )
    arguments = parser.parse_args()
    try:
        arguments.profiles = comma_separated(
            arguments.profiles,
            label="--profiles",
            allowed=PROFILE_FLAGS,
        )
        arguments.cases = comma_separated(
            arguments.cases,
            label="--cases",
            allowed=case_names,
        )
    except BenchmarkError as error:
        parser.error(str(error))
    if arguments.runner is None and arguments.runner_arg:
        parser.error("--runner-arg requires --runner")
    return arguments


def load_manifest(path: Path) -> list[Case]:
    try:
        with path.open("rb") as source:
            manifest = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise BenchmarkError(
            f"could not read kernel manifest {path}: {error}"
        ) from error
    if manifest.get("schema_version") != MANIFEST_SCHEMA_VERSION:
        raise BenchmarkError(
            f"{path}: unsupported schema_version {manifest.get('schema_version')!r}"
        )
    raw_cases = manifest.get("case")
    if not isinstance(raw_cases, list) or not raw_cases:
        raise BenchmarkError(f"{path}: at least one [[case]] is required")

    benchmark_directory = path.parent.resolve()
    cases: list[Case] = []
    names: set[str] = set()
    for index, raw in enumerate(raw_cases, 1):
        if not isinstance(raw, dict):
            raise BenchmarkError(f"{path}: case {index} is not a table")
        try:
            name = raw["name"]
            family = raw["family"]
            source_name = raw["source"]
            work_unit = raw["work_unit"]
            work_count = raw["work_count"]
            expected_result = raw["expected_result"]
            expected_functions = raw["expected_post_inline_functions"]
            expected_calls = raw["expected_post_inline_calls"]
        except KeyError as error:
            raise BenchmarkError(
                f"{path}: case {index} is missing {error.args[0]!r}"
            ) from error
        strings = {
            "name": name,
            "family": family,
            "source": source_name,
            "work_unit": work_unit,
            "expected_result": expected_result,
        }
        for label, value in strings.items():
            if not isinstance(value, str) or not value:
                raise BenchmarkError(
                    f"{path}: case {index} {label} must be a nonempty string"
                )
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name):
            raise BenchmarkError(f"{path}: invalid case name {name!r}")
        if name in names:
            raise BenchmarkError(f"{path}: duplicate case name {name!r}")
        names.add(name)
        if (
            not isinstance(work_count, int)
            or isinstance(work_count, bool)
            or work_count <= 0
        ):
            raise BenchmarkError(f"{path}: {name} work_count must be positive")
        if (
            not isinstance(expected_functions, int)
            or isinstance(expected_functions, bool)
            or expected_functions <= 0
        ):
            raise BenchmarkError(
                f"{path}: {name} expected_post_inline_functions must be positive"
            )
        if (
            len(expected_result) > 128
            or any(character.isspace() for character in expected_result)
            or not expected_result.isprintable()
        ):
            raise BenchmarkError(
                f"{path}: {name} expected_result must be a compact printable token"
            )
        if not isinstance(expected_calls, dict) or not expected_calls:
            raise BenchmarkError(
                f"{path}: {name} expected_post_inline_calls must be a table"
            )
        parsed_calls: dict[str, int] = {}
        for profile, count in expected_calls.items():
            if profile not in PROFILE_FLAGS:
                raise BenchmarkError(
                    f"{path}: {name} has unsupported call profile {profile!r}"
                )
            if not isinstance(count, int) or isinstance(count, bool) or count < 0:
                raise BenchmarkError(
                    f"{path}: {name} call count for {profile} must be nonnegative"
                )
            parsed_calls[profile] = count

        source = (benchmark_directory / source_name).resolve()
        try:
            source.relative_to(benchmark_directory)
        except ValueError as error:
            raise BenchmarkError(
                f"{path}: {name} source escapes the benchmark directory"
            ) from error
        if not source.is_file():
            raise BenchmarkError(f"{path}: {name} source does not exist: {source}")
        cases.append(
            Case(
                name,
                family,
                source,
                work_unit,
                work_count,
                expected_result,
                expected_functions,
                parsed_calls,
            )
        )
    return cases


def select_cases(cases: list[Case], selected: list[str]) -> list[Case]:
    by_name = {case.name: case for case in cases}
    return [by_name[name] for name in selected]


def resolve_executable(value: str) -> Path:
    if os.sep not in value:
        found = shutil.which(value)
        if found is None:
            raise BenchmarkError(f"executable is not available: {value}")
        path = Path(found)
    else:
        path = Path(value)
    try:
        path = path.expanduser().resolve(strict=True)
    except OSError as error:
        raise BenchmarkError(f"executable does not exist: {value}") from error
    if not path.is_file() or not os.access(path, os.X_OK):
        raise BenchmarkError(f"file is not executable: {path}")
    return path


def prepare_output(path: Path) -> Path:
    path = path.expanduser().resolve()
    if path.exists():
        if not path.is_dir():
            raise BenchmarkError(f"output is not a directory: {path}")
        if any(path.iterdir()):
            raise BenchmarkError(f"output directory is not empty: {path}")
    else:
        path.mkdir(parents=True)
    return path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    temporary = path.with_name(f"{path.name}.part.{os.getpid()}")
    temporary.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def capture_command(
    command: list[str],
    standard_output: Path,
    standard_error: Path,
) -> int:
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"
    with standard_output.open("wb") as stdout, standard_error.open("wb") as stderr:
        completed = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            check=False,
            env=environment,
        )
    return completed.returncode


def compiler_version(compiler: Path, output: Path) -> dict[str, object]:
    stdout = output / "compiler-version.stdout.txt"
    stderr = output / "compiler-version.stderr.txt"
    command = [os.fspath(compiler), "--version"]
    status = capture_command(command, stdout, stderr)
    if status != 0:
        raise BenchmarkError(
            f"compiler version query failed with status {status}; see {stderr}"
        )
    return {
        "command": command,
        "exit_status": status,
        "stdout": stdout.name,
        "stderr": stderr.name,
    }


def compiler_target(
    compiler: Path,
    requested_target: str | None,
    output: Path,
) -> tuple[str, dict[str, object]]:
    stdout = output / "compiler-dumpmachine.stdout.txt"
    stderr = output / "compiler-dumpmachine.stderr.txt"
    command = [os.fspath(compiler)]
    if requested_target:
        command.append(f"--target={requested_target}")
    command.append("-dumpmachine")
    status = capture_command(command, stdout, stderr)
    if status != 0:
        raise BenchmarkError(
            f"compiler target query failed with status {status}; see {stderr}"
        )
    try:
        target = stdout.read_text(encoding="utf-8").strip()
    except UnicodeDecodeError as error:
        raise BenchmarkError(f"compiler target query is not UTF-8: {stdout}") from error
    if not target or any(character.isspace() for character in target):
        raise BenchmarkError(
            f"compiler target query returned an invalid triple: {target!r}"
        )
    return target, {
        "command": command,
        "exit_status": status,
        "stdout": stdout.name,
        "stderr": stderr.name,
    }


def effective_configs(
    compiler: Path,
    target: str,
    profiles: list[str],
    ccc_arguments: list[str],
    output: Path,
) -> dict[str, dict[str, object]]:
    directory = output / "effective-config"
    directory.mkdir()
    records: dict[str, dict[str, object]] = {}
    for profile in profiles:
        stdout = directory / f"{profile}.stdout.txt"
        stderr = directory / f"{profile}.stderr.txt"
        command = [
            os.fspath(compiler),
            PROFILE_FLAGS[profile],
            f"--target={target}",
            *ccc_arguments,
            "--print-effective-config",
        ]
        status = capture_command(command, stdout, stderr)
        if status != 0:
            raise BenchmarkError(
                f"effective-config query for -{profile} failed with status "
                f"{status}; see {stderr}"
            )
        records[profile] = {
            "command": command,
            "exit_status": status,
            "stdout": os.fspath(stdout.relative_to(output)),
            "stderr": os.fspath(stderr.relative_to(output)),
        }
    return records


def canonical_host_target() -> str | None:
    system = platform.system()
    machine = platform.machine().lower()
    if system == "Linux" and machine in ("x86_64", "amd64"):
        return "x86_64-unknown-linux-gnu"
    if system == "Linux" and machine in ("aarch64", "arm64"):
        return "aarch64-unknown-linux-gnu"
    if system == "Linux" and machine == "riscv64":
        return "riscv64-unknown-linux-gnu"
    if system == "Darwin" and machine in ("aarch64", "arm64"):
        return "aarch64-apple-darwin"
    return None


def execution_configuration(
    mode: str,
    target: str,
    runner: Path | None,
    runner_arguments: list[str],
) -> tuple[str, list[str]]:
    if mode == "object":
        return "not-run", []
    host_target = canonical_host_target()
    if mode == "performance":
        if runner is not None:
            raise BenchmarkError("performance mode does not permit an emulated runner")
        if host_target != target:
            raise BenchmarkError(
                f"performance mode requires native target {host_target or '<unknown>'}; "
                f"compiler selected {target}"
            )
        return "native", []
    if runner is not None:
        return "runner", [os.fspath(runner), *runner_arguments]
    if host_target != target:
        raise BenchmarkError(
            f"correctness mode cannot execute target {target} on "
            f"{host_target or 'this host'} without --runner"
        )
    return "native", []


def copy_cases(cases: list[Case], output: Path) -> list[Case]:
    source_directory = output / "sources"
    source_directory.mkdir()
    copied: list[Case] = []
    for case in cases:
        destination = source_directory / f"{case.name}.c"
        shutil.copyfile(case.source, destination)
        copied.append(
            Case(
                case.name,
                case.family,
                destination,
                case.work_unit,
                case.work_count,
                case.expected_result,
                case.expected_functions,
                case.expected_calls,
            )
        )
    return copied


def write_manifest(cases: list[Case], output: Path) -> None:
    with (output / "manifest.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.writer(file, delimiter="\t", lineterminator="\n")
        writer.writerow(
            (
                "benchmark",
                "family",
                "work_unit",
                "work_count",
                "expected_result",
                "expected_post_inline_functions",
                "expected_post_inline_calls",
                "source",
                "source_bytes",
                "source_sha256",
            )
        )
        for case in cases:
            writer.writerow(
                (
                    case.name,
                    case.family,
                    case.work_unit,
                    case.work_count,
                    case.expected_result,
                    case.expected_functions,
                    json.dumps(
                        case.expected_calls,
                        sort_keys=True,
                        separators=(",", ":"),
                    ),
                    case.source.relative_to(output),
                    case.source.stat().st_size,
                    sha256(case.source),
                )
            )


def exit_status(wait_status: int) -> tuple[int, int | None]:
    if os.WIFEXITED(wait_status):
        return os.WEXITSTATUS(wait_status), None
    if os.WIFSIGNALED(wait_status):
        signal = os.WTERMSIG(wait_status)
        return 128 + signal, signal
    return 255, None


def peak_rss_bytes(usage: resource.struct_rusage) -> int:
    value = int(usage.ru_maxrss)
    if platform.system() == "Darwin":
        return value
    return value * 1024


def measured_run(
    command: list[str],
    standard_output: Path,
    standard_error: Path,
) -> dict[str, object]:
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"
    with standard_output.open("wb") as stdout, standard_error.open("wb") as stderr:
        started = time.monotonic_ns()
        process = os.fork()
        if process == 0:
            try:
                os.dup2(stdout.fileno(), 1)
                os.dup2(stderr.fileno(), 2)
                os.execve(command[0], command, environment)
            except OSError as error:
                os.write(2, f"could not execute {command[0]}: {error}\n".encode())
                os._exit(127)
        while True:
            try:
                _, wait_status, usage = os.wait4(process, 0)
                break
            except InterruptedError:
                continue
        elapsed = (time.monotonic_ns() - started) / 1_000_000_000
    status, signal = exit_status(wait_status)
    return {
        "format_version": FORMAT_VERSION,
        "command": command,
        "wall_seconds": elapsed,
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "peak_rss_bytes": peak_rss_bytes(usage),
        "minor_page_faults": usage.ru_minflt,
        "major_page_faults": usage.ru_majflt,
        "voluntary_context_switches": usage.ru_nvcsw,
        "involuntary_context_switches": usage.ru_nivcsw,
        "exit_status": status,
        "signal": signal,
        "stdout": os.fspath(standard_output),
        "stderr": os.fspath(standard_error),
    }


def parse_stats(path: Path) -> tuple[dict[str, int], tuple[str, ...]]:
    stats: dict[str, int] = {}
    order: list[str] = []
    with path.open(encoding="utf-8", newline="") as file:
        reader = csv.reader(file, delimiter="\t")
        for line_number, row in enumerate(reader, 1):
            if len(row) != 2:
                raise BenchmarkError(
                    f"{path}:{line_number}: expected one metric/value row"
                )
            metric, raw_value = row
            if not re.fullmatch(r"[a-z][a-z0-9_.]*", metric):
                raise BenchmarkError(
                    f"{path}:{line_number}: invalid metric name {metric!r}"
                )
            if metric in stats:
                raise BenchmarkError(
                    f"{path}:{line_number}: duplicate metric {metric!r}"
                )
            if not raw_value.isdecimal():
                raise BenchmarkError(
                    f"{path}:{line_number}: metric {metric!r} is not unsigned decimal"
                )
            stats[metric] = int(raw_value)
            order.append(metric)
    if not order or order[0] != "schema_version":
        raise BenchmarkError(f"{path}: first metric must be schema_version")
    if stats["schema_version"] != CODEGEN_STATS_SCHEMA_VERSION:
        raise BenchmarkError(
            f"{path}: unsupported codegen-stats schema {stats['schema_version']}"
        )
    missing = [metric for metric in REQUIRED_METRICS if metric not in stats]
    if missing:
        raise BenchmarkError(f"{path}: missing metric(s): {', '.join(missing)}")
    return stats, tuple(order)


def format_seconds(value: float) -> str:
    return f"{value:.9f}"


def timing_columns(timing: dict[str, object]) -> dict[str, object]:
    row = {
        field: timing[field]
        for field in (
            "wall_seconds",
            "user_seconds",
            "system_seconds",
            "peak_rss_bytes",
            "minor_page_faults",
            "major_page_faults",
            "voluntary_context_switches",
            "involuntary_context_switches",
            "exit_status",
        )
    }
    for field in ("wall_seconds", "user_seconds", "system_seconds"):
        row[field] = format_seconds(float(row[field]))
    return row


def command_record(
    file: object,
    *,
    case: Case,
    profile: str,
    kind: str,
    command: list[str],
    timed: bool,
    phase: str | None = None,
    iteration: int | None = None,
) -> None:
    record: dict[str, object] = {
        "benchmark": case.name,
        "profile": profile,
        "kind": kind,
        "command": command,
        "timed": timed,
    }
    if phase is not None:
        record["phase"] = phase
    if iteration is not None:
        record["iteration"] = iteration
    file.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
    file.flush()


def write_summary(
    cases: list[Case],
    profiles: list[str],
    target: str,
    mode: str,
    execution_kind: str,
    builds: list[BuildInvocation],
    runs: list[RunInvocation],
    stats_records: list[StatsRecord],
    artifacts: list[Artifact],
    output: Path,
) -> None:
    stats_by_key = {
        (record.case.name, record.profile): record.stats for record in stats_records
    }
    artifacts_by_key = {
        (artifact.case.name, artifact.profile, artifact.kind): artifact
        for artifact in artifacts
    }
    with (output / "summary.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(
            file,
            fieldnames=SUMMARY_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        for case in cases:
            for profile in profiles:
                key = (case.name, profile)
                compile_samples = [
                    invocation
                    for invocation in builds
                    if (invocation.case.name, invocation.profile) == key
                    and invocation.stage == "compile"
                    and invocation.phase == "sample"
                ]
                links = [
                    invocation
                    for invocation in builds
                    if (invocation.case.name, invocation.profile) == key
                    and invocation.stage == "link"
                ]
                validations = [
                    invocation
                    for invocation in runs
                    if (invocation.case.name, invocation.profile) == key
                    and invocation.phase == "validation"
                ]
                samples = [
                    invocation
                    for invocation in runs
                    if (invocation.case.name, invocation.profile) == key
                    and invocation.phase == "sample"
                ]
                compile_wall = [
                    float(invocation.timing["wall_seconds"])
                    for invocation in compile_samples
                ]
                compile_rss = [
                    int(invocation.timing["peak_rss_bytes"])
                    for invocation in compile_samples
                ]
                runtime_wall = [
                    float(invocation.timing["wall_seconds"]) for invocation in samples
                ]
                runtime_rss = [
                    int(invocation.timing["peak_rss_bytes"]) for invocation in samples
                ]
                stats = stats_by_key[key]
                final_object = artifacts_by_key[(case.name, profile, "final-object")]
                executable = artifacts_by_key.get((case.name, profile, "executable"))
                row: dict[str, object] = {
                    "benchmark": case.name,
                    "family": case.family,
                    "profile": profile,
                    "target": target,
                    "mode": mode,
                    "execution_kind": execution_kind,
                    "performance_comparable": (
                        "1"
                        if mode == "performance" and execution_kind == "native"
                        else "0"
                    ),
                    "work_unit": case.work_unit,
                    "work_count": case.work_count,
                    "compile_samples": len(compile_samples),
                    "compile_median_wall_seconds": format_seconds(
                        statistics.median(compile_wall)
                    ),
                    "compile_min_wall_seconds": format_seconds(min(compile_wall)),
                    "compile_max_wall_seconds": format_seconds(max(compile_wall)),
                    "compile_median_peak_rss_bytes": round(
                        statistics.median(compile_rss)
                    ),
                    "link_wall_seconds": (
                        format_seconds(float(links[0].timing["wall_seconds"]))
                        if links
                        else ""
                    ),
                    "validation_wall_seconds": (
                        format_seconds(float(validations[0].timing["wall_seconds"]))
                        if validations
                        else ""
                    ),
                    "runtime_samples": len(samples),
                    "runtime_median_wall_seconds": (
                        format_seconds(statistics.median(runtime_wall))
                        if samples
                        else ""
                    ),
                    "runtime_min_wall_seconds": (
                        format_seconds(min(runtime_wall)) if samples else ""
                    ),
                    "runtime_max_wall_seconds": (
                        format_seconds(max(runtime_wall)) if samples else ""
                    ),
                    "runtime_median_peak_rss_bytes": (
                        round(statistics.median(runtime_rss)) if samples else ""
                    ),
                    "runtime_ns_per_work_unit": (
                        f"{statistics.median(runtime_wall) * 1_000_000_000 / case.work_count:.6f}"
                        if samples
                        else ""
                    ),
                    **{metric: stats[metric] for metric in REQUIRED_METRICS},
                    "final_object.file_bytes": final_object.file_bytes,
                    "final_object.sha256": final_object.digest,
                    "executable.file_bytes": (
                        executable.file_bytes if executable is not None else ""
                    ),
                    "executable.sha256": (
                        executable.digest if executable is not None else ""
                    ),
                }
                writer.writerow(row)


def run(arguments: argparse.Namespace, manifest_path: Path) -> Path:
    all_cases = load_manifest(manifest_path)
    selected_cases = select_cases(all_cases, arguments.cases)
    compiler = resolve_executable(arguments.ccc)
    runner = (
        resolve_executable(arguments.runner) if arguments.runner is not None else None
    )
    output = prepare_output(arguments.output)
    shutil.copyfile(manifest_path, output / "input-manifest.toml")
    cases = copy_cases(selected_cases, output)
    write_manifest(cases, output)

    effective_target, target_query = compiler_target(compiler, arguments.target, output)
    execution_kind, runner_prefix = execution_configuration(
        arguments.mode,
        effective_target,
        runner,
        arguments.runner_arg,
    )
    environment = {
        "format_version": FORMAT_VERSION,
        "manifest_schema_version": MANIFEST_SCHEMA_VERSION,
        "codegen_stats_schema_version": CODEGEN_STATS_SCHEMA_VERSION,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "mode": arguments.mode,
        "compiler": os.fspath(compiler),
        "compiler_sha256": sha256(compiler),
        "compiler_version": compiler_version(compiler, output),
        "requested_target": arguments.target,
        "target": effective_target,
        "target_query": target_query,
        "host_target": canonical_host_target(),
        "execution_kind": execution_kind,
        "performance_comparable": (
            arguments.mode == "performance" and execution_kind == "native"
        ),
        "runner": (
            {
                "executable": os.fspath(runner),
                "sha256": sha256(runner),
                "arguments": arguments.runner_arg,
            }
            if runner is not None
            else None
        ),
        "host": {
            "machine": platform.machine(),
            "platform": platform.platform(),
            "python": platform.python_version(),
            "system": platform.system(),
        },
        "profiles": arguments.profiles,
        "cases": arguments.cases,
        "ccc_arguments": arguments.ccc_arg,
        "compile_warmups": arguments.compile_warmups,
        "compile_samples": arguments.compile_samples,
        "run_warmups": (
            arguments.run_warmups if arguments.mode == "performance" else 0
        ),
        "run_samples": (
            arguments.run_samples if arguments.mode == "performance" else 0
        ),
        "effective_configs": effective_configs(
            compiler,
            effective_target,
            arguments.profiles,
            arguments.ccc_arg,
            output,
        ),
    }
    write_json(output / "environment.json", environment)

    raw_directory = output / "raw"
    raw_directory.mkdir()
    builds: list[BuildInvocation] = []
    runs: list[RunInvocation] = []
    stats_records: list[StatsRecord] = []
    artifacts: list[Artifact] = []

    with ExitStack() as stack:
        commands_file = stack.enter_context(
            (output / "commands.jsonl").open("w", encoding="utf-8")
        )
        build_file = stack.enter_context(
            (output / "build-times.tsv").open("w", encoding="utf-8", newline="")
        )
        run_file = stack.enter_context(
            (output / "run-times.tsv").open("w", encoding="utf-8", newline="")
        )
        stats_file = stack.enter_context(
            (output / "codegen-stats.tsv").open("w", encoding="utf-8", newline="")
        )
        artifacts_file = stack.enter_context(
            (output / "artifacts.tsv").open("w", encoding="utf-8", newline="")
        )
        build_writer = csv.DictWriter(
            build_file,
            fieldnames=BUILD_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        run_writer = csv.DictWriter(
            run_file,
            fieldnames=RUN_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        stats_writer = csv.DictWriter(
            stats_file,
            fieldnames=STATS_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        artifact_writer = csv.DictWriter(
            artifacts_file,
            fieldnames=ARTIFACT_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        build_writer.writeheader()
        run_writer.writeheader()
        stats_writer.writeheader()
        artifact_writer.writeheader()

        for case in cases:
            for profile in arguments.profiles:
                invocation_directory = raw_directory / case.name / profile
                invocation_directory.mkdir(parents=True)
                compile_invocations: list[BuildInvocation] = []
                for phase, count in (
                    ("warmup", arguments.compile_warmups),
                    ("sample", arguments.compile_samples),
                ):
                    for iteration in range(1, count + 1):
                        stem = f"compile-{phase}-{iteration:03d}"
                        stdout = invocation_directory / f"{stem}.stdout.txt"
                        stderr = invocation_directory / f"{stem}.stderr.txt"
                        object_path = invocation_directory / f"{stem}.o"
                        timing_path = invocation_directory / f"{stem}.timing.json"
                        command = [
                            os.fspath(compiler),
                            "-std=c11",
                            PROFILE_FLAGS[profile],
                            f"--target={effective_target}",
                            *arguments.ccc_arg,
                            "-c",
                            os.fspath(case.source),
                            "-o",
                            os.fspath(object_path),
                        ]
                        command_record(
                            commands_file,
                            case=case,
                            profile=profile,
                            kind="object-compile",
                            command=command,
                            timed=True,
                            phase=phase,
                            iteration=iteration,
                        )
                        timing = measured_run(command, stdout, stderr)
                        write_json(timing_path, timing)
                        if timing["exit_status"] != 0:
                            raise BenchmarkError(
                                f"{case.name} at -{profile} compile {phase} "
                                f"{iteration} failed with status "
                                f"{timing['exit_status']}; see {stderr}"
                            )
                        if not object_path.is_file():
                            raise BenchmarkError(
                                f"{case.name} at -{profile} did not create "
                                f"{object_path}"
                            )
                        timing["artifact_bytes"] = object_path.stat().st_size
                        timing["artifact_sha256"] = sha256(object_path)
                        write_json(timing_path, timing)
                        invocation = BuildInvocation(
                            case,
                            profile,
                            "compile",
                            phase,
                            iteration,
                            timing,
                            "final-object",
                            object_path,
                        )
                        builds.append(invocation)
                        compile_invocations.append(invocation)
                        build_writer.writerow(
                            {
                                "benchmark": case.name,
                                "profile": profile,
                                "stage": "compile",
                                "phase": phase,
                                "iteration": iteration,
                                **timing_columns(timing),
                                "artifact_kind": "final-object",
                                "artifact_bytes": timing["artifact_bytes"],
                                "artifact_sha256": timing["artifact_sha256"],
                            }
                        )
                        build_file.flush()

                samples = [
                    invocation
                    for invocation in compile_invocations
                    if invocation.phase == "sample"
                ]
                first_sample = samples[0]
                for invocation in samples[1:]:
                    if (
                        invocation.timing["artifact_sha256"]
                        != first_sample.timing["artifact_sha256"]
                    ):
                        raise BenchmarkError(
                            f"{case.name} at -{profile} produced "
                            "nondeterministic final objects"
                        )
                final_object = Artifact(
                    case,
                    profile,
                    "final-object",
                    first_sample.artifact_path,
                    int(first_sample.timing["artifact_bytes"]),
                    str(first_sample.timing["artifact_sha256"]),
                )
                artifacts.append(final_object)
                artifact_writer.writerow(
                    {
                        "benchmark": case.name,
                        "profile": profile,
                        "kind": final_object.kind,
                        "path": final_object.path.relative_to(output),
                        "file_bytes": final_object.file_bytes,
                        "sha256": final_object.digest,
                    }
                )
                artifacts_file.flush()

                stats_stdout = invocation_directory / "codegen-stats.stdout.tsv"
                stats_stderr = invocation_directory / "codegen-stats.stderr.txt"
                stats_result = invocation_directory / "codegen-stats.result.json"
                stats_command = [
                    os.fspath(compiler),
                    "-std=c11",
                    PROFILE_FLAGS[profile],
                    f"--target={effective_target}",
                    *arguments.ccc_arg,
                    "--emit=codegen-stats",
                    os.fspath(case.source),
                ]
                command_record(
                    commands_file,
                    case=case,
                    profile=profile,
                    kind="codegen-stats",
                    command=stats_command,
                    timed=False,
                )
                stats_status = capture_command(
                    stats_command, stats_stdout, stats_stderr
                )
                write_json(
                    stats_result,
                    {
                        "format_version": FORMAT_VERSION,
                        "command": stats_command,
                        "exit_status": stats_status,
                        "stdout": os.fspath(stats_stdout),
                        "stderr": os.fspath(stats_stderr),
                        "timed": False,
                    },
                )
                if stats_status != 0:
                    raise BenchmarkError(
                        f"{case.name} at -{profile} codegen-stats failed with "
                        f"status {stats_status}; see {stats_stderr}"
                    )
                stats, order = parse_stats(stats_stdout)
                if stats["post_inline_ir.functions"] != case.expected_functions:
                    raise BenchmarkError(
                        f"{case.name} at -{profile} emitted "
                        f"{stats['post_inline_ir.functions']} post-inline "
                        f"functions; expected {case.expected_functions}"
                    )
                expected_calls = case.expected_calls.get(profile)
                if (
                    expected_calls is not None
                    and stats["post_inline_ir.call_instructions"] != expected_calls
                ):
                    raise BenchmarkError(
                        f"{case.name} at -{profile} emitted "
                        f"{stats['post_inline_ir.call_instructions']} post-inline "
                        f"calls; expected {expected_calls}"
                    )
                stats_record = StatsRecord(case, profile, stats, order)
                stats_records.append(stats_record)
                for metric in order:
                    stats_writer.writerow(
                        {
                            "benchmark": case.name,
                            "family": case.family,
                            "profile": profile,
                            "metric": metric,
                            "value": stats[metric],
                        }
                    )
                stats_file.flush()

                if arguments.mode == "object":
                    continue

                executable = invocation_directory / "kernel"
                link_stdout = invocation_directory / "link.stdout.txt"
                link_stderr = invocation_directory / "link.stderr.txt"
                link_timing_path = invocation_directory / "link.timing.json"
                link_command = [
                    os.fspath(compiler),
                    "-std=c11",
                    PROFILE_FLAGS[profile],
                    f"--target={effective_target}",
                    *arguments.ccc_arg,
                    os.fspath(final_object.path),
                    "-o",
                    os.fspath(executable),
                ]
                command_record(
                    commands_file,
                    case=case,
                    profile=profile,
                    kind="link",
                    command=link_command,
                    timed=True,
                    phase="canonical",
                    iteration=0,
                )
                link_timing = measured_run(link_command, link_stdout, link_stderr)
                write_json(link_timing_path, link_timing)
                if link_timing["exit_status"] != 0:
                    raise BenchmarkError(
                        f"{case.name} at -{profile} link failed with status "
                        f"{link_timing['exit_status']}; see {link_stderr}"
                    )
                if not executable.is_file() or not os.access(executable, os.X_OK):
                    raise BenchmarkError(
                        f"{case.name} at -{profile} did not create executable "
                        f"{executable}"
                    )
                link_timing["artifact_bytes"] = executable.stat().st_size
                link_timing["artifact_sha256"] = sha256(executable)
                write_json(link_timing_path, link_timing)
                link_invocation = BuildInvocation(
                    case,
                    profile,
                    "link",
                    "canonical",
                    0,
                    link_timing,
                    "executable",
                    executable,
                )
                builds.append(link_invocation)
                build_writer.writerow(
                    {
                        "benchmark": case.name,
                        "profile": profile,
                        "stage": "link",
                        "phase": "canonical",
                        "iteration": 0,
                        **timing_columns(link_timing),
                        "artifact_kind": "executable",
                        "artifact_bytes": link_timing["artifact_bytes"],
                        "artifact_sha256": link_timing["artifact_sha256"],
                    }
                )
                build_file.flush()
                executable_artifact = Artifact(
                    case,
                    profile,
                    "executable",
                    executable,
                    int(link_timing["artifact_bytes"]),
                    str(link_timing["artifact_sha256"]),
                )
                artifacts.append(executable_artifact)
                artifact_writer.writerow(
                    {
                        "benchmark": case.name,
                        "profile": profile,
                        "kind": executable_artifact.kind,
                        "path": executable_artifact.path.relative_to(output),
                        "file_bytes": executable_artifact.file_bytes,
                        "sha256": executable_artifact.digest,
                    }
                )
                artifacts_file.flush()

                execution_command = [*runner_prefix, os.fspath(executable)]
                phases = [("validation", 1)]
                if arguments.mode == "performance":
                    phases.extend(
                        (
                            ("warmup", arguments.run_warmups),
                            ("sample", arguments.run_samples),
                        )
                    )
                for phase, count in phases:
                    for iteration in range(1, count + 1):
                        stem = f"run-{phase}-{iteration:03d}"
                        stdout = invocation_directory / f"{stem}.stdout.txt"
                        stderr = invocation_directory / f"{stem}.stderr.txt"
                        timing_path = invocation_directory / f"{stem}.timing.json"
                        command_record(
                            commands_file,
                            case=case,
                            profile=profile,
                            kind="execute",
                            command=execution_command,
                            timed=phase != "validation",
                            phase=phase,
                            iteration=iteration,
                        )
                        timing = measured_run(execution_command, stdout, stderr)
                        write_json(timing_path, timing)
                        invocation = RunInvocation(
                            case,
                            profile,
                            execution_kind,
                            phase,
                            iteration,
                            timing,
                        )
                        runs.append(invocation)
                        run_writer.writerow(
                            {
                                "benchmark": case.name,
                                "profile": profile,
                                "execution_kind": execution_kind,
                                "phase": phase,
                                "iteration": iteration,
                                **timing_columns(timing),
                            }
                        )
                        run_file.flush()
                        if timing["exit_status"] != 0:
                            raise BenchmarkError(
                                f"{case.name} at -{profile} {phase} failed "
                                f"with status {timing['exit_status']}; see {stderr}"
                            )
                        if stdout.stat().st_size != 0 or stderr.stat().st_size != 0:
                            raise BenchmarkError(
                                f"{case.name} at -{profile} {phase} produced "
                                "unexpected output"
                            )

    write_summary(
        cases,
        arguments.profiles,
        effective_target,
        arguments.mode,
        execution_kind,
        builds,
        runs,
        stats_records,
        artifacts,
        output,
    )
    return output


def main() -> int:
    manifest_path = Path(__file__).resolve().with_name("manifest.toml")
    try:
        cases = load_manifest(manifest_path)
        arguments = parse_arguments(tuple(case.name for case in cases))
        output = run(arguments, manifest_path)
    except BenchmarkError as error:
        print(f"kernel benchmark: {error}", file=sys.stderr)
        return 1
    print(f"kernel benchmark results: {output}")
    print(f"summary: {output / 'summary.tsv'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
