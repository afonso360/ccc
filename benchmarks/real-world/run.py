#!/usr/bin/env python3

"""Measure large deterministic workloads using already-built real programs.

This runner deliberately does not build a corpus or run its upstream test suite.
Use the separately maintained corpus adapters to build and validate CCC programs,
then pass their executable paths here for controlled native timing.
"""

from __future__ import annotations

import argparse
import csv
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import statistics
import sys
import time
import tomllib
from typing import Iterable


FORMAT_VERSION = 1
MANIFEST_SCHEMA_VERSION = 1
WORKLOAD_KINDS = ("compression", "lua")
RUN_FIELDS = (
    "benchmark",
    "family",
    "operation",
    "phase",
    "iteration",
    "wall_seconds",
    "user_seconds",
    "system_seconds",
    "peak_rss_bytes",
    "exit_status",
)
SUMMARY_FIELDS = (
    "benchmark",
    "family",
    "operation",
    "work_unit",
    "work_count",
    "input_bytes",
    "warmups",
    "samples",
    "median_wall_seconds",
    "minimum_wall_seconds",
    "maximum_wall_seconds",
    "median_user_seconds",
    "median_system_seconds",
    "median_peak_rss_bytes",
    "throughput_mib_per_second",
    "validation_sha256",
)
ARTIFACT_FIELDS = (
    "benchmark",
    "kind",
    "path",
    "file_bytes",
    "sha256",
)


class BenchmarkError(Exception):
    """An actionable benchmark setup, validation, or measurement failure."""


@dataclass(frozen=True)
class Case:
    name: str
    family: str
    workload: str
    work_unit: str
    fixed_work_count: int | None
    compression_arguments: tuple[str, ...] = ()
    decompression_arguments: tuple[str, ...] = ()


@dataclass(frozen=True)
class Measurement:
    case: Case
    operation: str
    phase: str
    iteration: int
    timing: dict[str, object]


def positive(value: str) -> int:
    if not value.isdecimal() or int(value) <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return int(value)


def nonnegative(value: str) -> int:
    if not value.isdecimal():
        raise argparse.ArgumentTypeError("must be a nonnegative integer")
    return int(value)


def comma_separated(
    value: str,
    *,
    label: str,
    allowed: Iterable[str],
) -> list[str]:
    values = value.split(",")
    if not values or any(not item for item in values):
        raise BenchmarkError(f"{label} must be a nonempty comma-separated list")
    if len(values) != len(set(values)):
        raise BenchmarkError(f"{label} contains a duplicate value")
    allowed_values = set(allowed)
    unknown = [item for item in values if item not in allowed_values]
    if unknown:
        raise BenchmarkError(f"{label} contains unknown value(s): {', '.join(unknown)}")
    return values


def string_list(value: object, *, label: str, manifest: Path) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise BenchmarkError(f"{manifest}: {label} must be a nonempty array")
    if any(not isinstance(item, str) or not item for item in value):
        raise BenchmarkError(f"{manifest}: {label} must contain nonempty strings")
    return tuple(value)


def load_manifest(path: Path) -> list[Case]:
    try:
        with path.open("rb") as source:
            manifest = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise BenchmarkError(f"could not read manifest {path}: {error}") from error
    if manifest.get("schema_version") != MANIFEST_SCHEMA_VERSION:
        raise BenchmarkError(
            f"{path}: unsupported schema_version {manifest.get('schema_version')!r}"
        )
    raw_cases = manifest.get("case")
    if not isinstance(raw_cases, list) or not raw_cases:
        raise BenchmarkError(f"{path}: at least one [[case]] is required")

    cases: list[Case] = []
    names: set[str] = set()
    for index, raw in enumerate(raw_cases, 1):
        if not isinstance(raw, dict):
            raise BenchmarkError(f"{path}: case {index} is not a table")
        try:
            name = raw["name"]
            family = raw["family"]
            workload = raw["workload"]
            work_unit = raw["work_unit"]
            work_count = raw["work_count"]
        except KeyError as error:
            raise BenchmarkError(
                f"{path}: case {index} is missing {error.args[0]!r}"
            ) from error
        for label, value in (
            ("name", name),
            ("family", family),
            ("workload", workload),
            ("work_unit", work_unit),
        ):
            if not isinstance(value, str) or not value:
                raise BenchmarkError(f"{path}: case {index} {label} must be nonempty")
        if name in names:
            raise BenchmarkError(f"{path}: duplicate case name {name!r}")
        names.add(name)
        if workload not in WORKLOAD_KINDS:
            raise BenchmarkError(f"{path}: {name} has unsupported workload {workload!r}")

        fixed_work_count: int | None
        if workload == "compression":
            if work_count != "input_bytes":
                raise BenchmarkError(
                    f"{path}: {name} compression work_count must be 'input_bytes'"
                )
            fixed_work_count = None
        elif isinstance(work_count, int) and not isinstance(work_count, bool) and work_count > 0:
            fixed_work_count = work_count
        else:
            raise BenchmarkError(f"{path}: {name} work_count must be positive")

        compression_arguments: tuple[str, ...] = ()
        decompression_arguments: tuple[str, ...] = ()
        if workload == "compression":
            compression_arguments = string_list(
                raw.get("compression_arguments"),
                label=f"{name} compression_arguments",
                manifest=path,
            )
            decompression_arguments = string_list(
                raw.get("decompression_arguments"),
                label=f"{name} decompression_arguments",
                manifest=path,
            )
        cases.append(
            Case(
                name=name,
                family=family,
                workload=workload,
                work_unit=work_unit,
                fixed_work_count=fixed_work_count,
                compression_arguments=compression_arguments,
                decompression_arguments=decompression_arguments,
            )
        )
    return cases


def parse_programs(
    values: list[str],
    *,
    selected: set[str],
) -> dict[str, Path]:
    programs: dict[str, Path] = {}
    for value in values:
        name, separator, raw_path = value.partition("=")
        if not separator or not name or not raw_path:
            raise BenchmarkError("--program entries must have the form CASE=PATH")
        if name not in selected:
            raise BenchmarkError(f"--program names an unselected or unknown case {name!r}")
        if name in programs:
            raise BenchmarkError(f"--program names {name!r} more than once")
        programs[name] = Path(raw_path)
    missing = selected.difference(programs)
    if missing:
        raise BenchmarkError(
            "--program is required for every selected case; missing "
            + ", ".join(sorted(missing))
        )
    return programs


def parse_arguments(case_names: tuple[str, ...]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Measure deterministic large workloads with already-built "
            "real-program executables. It never builds or tests those programs."
        )
    )
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="new or empty result directory",
    )
    parser.add_argument(
        "--cases",
        default=",".join(case_names),
        help="comma-separated cases (default: all)",
    )
    parser.add_argument(
        "--program",
        action="append",
        default=[],
        metavar="CASE=PATH",
        help=(
            "CCC-built executable for a selected case; repeat for every case "
            "(use --program=CASE=PATH for paths beginning with '-')"
        ),
    )
    parser.add_argument(
        "--input-mebibytes",
        type=positive,
        default=32,
        help="deterministic compression-input size in MiB (default: 32)",
    )
    parser.add_argument(
        "--warmups",
        type=nonnegative,
        default=1,
        help="untimed workload warmups (default: 1)",
    )
    parser.add_argument(
        "--samples",
        type=positive,
        default=5,
        help="measured workload samples (default: 5)",
    )
    arguments = parser.parse_args()
    try:
        arguments.cases = comma_separated(
            arguments.cases,
            label="--cases",
            allowed=case_names,
        )
        arguments.programs = parse_programs(
            arguments.program,
            selected=set(arguments.cases),
        )
    except BenchmarkError as error:
        parser.error(str(error))
    return arguments


def resolve_executable(path: Path, *, label: str) -> Path:
    try:
        resolved = path.expanduser().resolve(strict=True)
    except OSError as error:
        raise BenchmarkError(f"{label} does not exist: {path}") from error
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise BenchmarkError(f"{label} is not executable: {resolved}")
    return resolved


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
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def peak_rss_bytes(usage: resource.struct_rusage) -> int:
    value = int(usage.ru_maxrss)
    return value if platform.system() == "Darwin" else value * 1024


def status_from_wait(wait_status: int) -> tuple[int, int | None]:
    if os.WIFEXITED(wait_status):
        return os.WEXITSTATUS(wait_status), None
    if os.WIFSIGNALED(wait_status):
        signal = os.WTERMSIG(wait_status)
        return 128 + signal, signal
    return 255, None


def measured_run(
    command: list[str],
    *,
    stdout: Path,
    stderr: Path,
    environment: dict[str, str],
) -> dict[str, object]:
    with stdout.open("wb") as standard_output, stderr.open("wb") as standard_error:
        started = time.monotonic_ns()
        process = os.fork()
        if process == 0:
            try:
                os.dup2(standard_output.fileno(), 1)
                os.dup2(standard_error.fileno(), 2)
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
    status, signal = status_from_wait(wait_status)
    return {
        "wall_seconds": (time.monotonic_ns() - started) / 1_000_000_000,
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "peak_rss_bytes": peak_rss_bytes(usage),
        "exit_status": status,
        "signal": signal,
    }


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )


def timing_row(timing: dict[str, object]) -> dict[str, object]:
    row = dict(timing)
    row.pop("signal", None)
    for key in ("wall_seconds", "user_seconds", "system_seconds"):
        row[key] = f"{float(row[key]):.9f}"
    return row


def record_command(
    output: object,
    *,
    case: Case,
    kind: str,
    command: list[str],
    phase: str | None = None,
    iteration: int | None = None,
) -> None:
    record: dict[str, object] = {
        "benchmark": case.name,
        "kind": kind,
        "command": command,
    }
    if phase is not None:
        record["phase"] = phase
    if iteration is not None:
        record["iteration"] = iteration
    output.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
    output.flush()


def write_large_input(path: Path, mebibytes: int) -> tuple[int, str]:
    target_bytes = mebibytes * 1024 * 1024
    state = 0x7f4a7c15
    pattern = b"CCC real-program benchmark record: predictable data and noise.\n"
    block_size = 64 * 1024
    written = 0
    with path.open("wb") as output:
        while written < target_bytes:
            size = min(block_size, target_bytes - written)
            block = bytearray(size)
            block_index = written // block_size
            if block_index % 4 == 0:
                for index in range(size):
                    state ^= (state << 13) & 0xffffffff
                    state ^= state >> 17
                    state ^= (state << 5) & 0xffffffff
                    state &= 0xffffffff
                    block[index] = state & 0xff
            else:
                offset = (block_index * 37) % len(pattern)
                for index in range(size):
                    block[index] = pattern[(offset + index) % len(pattern)]
            output.write(block)
            written += size
    return target_bytes, sha256(path)


def write_lua_workload(path: Path) -> tuple[int, str]:
    source = """-- Generated by benchmarks/real-world/run.py; deterministic by design.
local modulus = 4294967296
local slot_count = 2048
local rounds = 512
local inner_count = 4096
local state = 0x51f15e5d
local checksum = 0x243f6a88
local slots = {}

for index = 1, slot_count do
  slots[index] = 0
end

for round_number = 1, rounds do
  for index = 1, inner_count do
    state = (state * 1664525 + 1013904223) % modulus
    local slot = (state % slot_count) + 1
    local value = (slots[slot] + (state % 65536) + index + round_number) % modulus
    slots[slot] = value
    checksum = (checksum + value) % modulus
  end

  for index = 1, slot_count do
    local value = slots[index]
    checksum = (checksum ~ (value + index * 0x9e3779b9)) % modulus
    checksum = ((checksum << 7) | (checksum >> 25)) % modulus
    slots[index] = (value * 33 + round_number + index) % modulus
  end
end

if (checksum ~ state) ~= 0x9bbfd5c9 then
  error("Lua workload checksum mismatch")
end
"""
    path.write_text(source, encoding="utf-8")
    return 512 * (4096 + 2048), sha256(path)


def ensure_success(
    timing: dict[str, object],
    *,
    case: Case,
    operation: str,
    stderr: Path,
) -> None:
    if timing["exit_status"] != 0:
        raise BenchmarkError(
            f"{case.name} {operation} failed with status {timing['exit_status']}; "
            f"see {stderr}"
        )


def validate_compression(
    case: Case,
    executable: Path,
    input_path: Path,
    input_digest: str,
    directory: Path,
    environment: dict[str, str],
    commands: object,
) -> tuple[Path, str]:
    compressed = directory / "validation.compressed"
    decompressed = directory / "validation.decompressed"
    compress_command = [
        os.fspath(executable),
        *case.compression_arguments,
        os.fspath(input_path),
    ]
    compress_stderr = directory / "validation-compress.stderr.txt"
    record_command(
        commands,
        case=case,
        kind="validate-compression",
        command=compress_command,
    )
    timing = measured_run(
        compress_command,
        stdout=compressed,
        stderr=compress_stderr,
        environment=environment,
    )
    write_json(directory / "validation-compress.json", timing)
    ensure_success(
        timing,
        case=case,
        operation="validation compression",
        stderr=compress_stderr,
    )
    if compressed.stat().st_size == 0:
        raise BenchmarkError(f"{case.name} validation did not produce compressed output")

    decompress_command = [
        os.fspath(executable),
        *case.decompression_arguments,
        os.fspath(compressed),
    ]
    decompress_stderr = directory / "validation-decompress.stderr.txt"
    record_command(
        commands,
        case=case,
        kind="validate-decompression",
        command=decompress_command,
    )
    timing = measured_run(
        decompress_command,
        stdout=decompressed,
        stderr=decompress_stderr,
        environment=environment,
    )
    write_json(directory / "validation-decompress.json", timing)
    ensure_success(
        timing,
        case=case,
        operation="validation decompression",
        stderr=decompress_stderr,
    )
    if decompressed.stat().st_size != input_path.stat().st_size:
        raise BenchmarkError(
            f"{case.name} decompression produced {decompressed.stat().st_size} bytes; "
            f"expected {input_path.stat().st_size}"
        )
    digest = sha256(decompressed)
    if digest != input_digest:
        raise BenchmarkError(
            f"{case.name} validation checksum mismatch: {digest} != {input_digest}"
        )
    return compressed, sha256(compressed)


def run_samples(
    case: Case,
    operation: str,
    command: list[str],
    *,
    warmups: int,
    samples: int,
    directory: Path,
    environment: dict[str, str],
    commands: object,
) -> list[Measurement]:
    measurements: list[Measurement] = []
    devnull = Path(os.devnull)
    for phase, count in (("warmup", warmups), ("sample", samples)):
        for iteration in range(1, count + 1):
            stderr = directory / f"{operation}-{phase}-{iteration:03d}.stderr.txt"
            record_command(
                commands,
                case=case,
                kind=operation,
                command=command,
                phase=phase,
                iteration=iteration,
            )
            timing = measured_run(
                command,
                stdout=devnull,
                stderr=stderr,
                environment=environment,
            )
            write_json(
                directory / f"{operation}-{phase}-{iteration:03d}.json",
                timing,
            )
            ensure_success(
                timing,
                case=case,
                operation=f"{operation} {phase}",
                stderr=stderr,
            )
            measurements.append(Measurement(case, operation, phase, iteration, timing))
    return measurements


def artifact(path: Path, *, benchmark: str, kind: str, output: Path) -> dict[str, object]:
    return {
        "benchmark": benchmark,
        "kind": kind,
        "path": os.fspath(path.relative_to(output)),
        "file_bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def run(arguments: argparse.Namespace, cases: list[Case]) -> Path:
    output = prepare_output(arguments.output)
    selected = [case for case in cases if case.name in arguments.cases]
    programs = {
        case.name: resolve_executable(
            arguments.programs[case.name], label=f"{case.name} program"
        )
        for case in selected
    }
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"

    inputs = output / "inputs"
    workloads = output / "workloads"
    inputs.mkdir()
    workloads.mkdir()
    large_input = inputs / "mixed-input.bin"
    input_bytes, input_digest = write_large_input(
        large_input, arguments.input_mebibytes
    )
    lua_workload = inputs / "interpreter-workload.lua"
    lua_work_units, lua_digest = write_lua_workload(lua_workload)

    artifacts = [
        artifact(large_input, benchmark="shared", kind="compression-input", output=output),
        artifact(lua_workload, benchmark="lua", kind="interpreter-workload", output=output),
    ]
    for case in selected:
        artifacts.append(
            {
                "benchmark": case.name,
                "kind": "program",
                "path": os.fspath(programs[case.name]),
                "file_bytes": programs[case.name].stat().st_size,
                "sha256": sha256(programs[case.name]),
            }
        )

    measurements: list[Measurement] = []
    validation_hashes: dict[tuple[str, str], str] = {}
    with (
        (output / "commands.jsonl").open("w", encoding="utf-8") as commands,
        (output / "run-times.tsv").open("w", encoding="utf-8", newline="")
        as runs_file,
        (output / "artifacts.tsv").open("w", encoding="utf-8", newline="")
        as artifacts_file,
    ):
        run_writer = csv.DictWriter(
            runs_file, fieldnames=RUN_FIELDS, delimiter="\t", lineterminator="\n"
        )
        artifact_writer = csv.DictWriter(
            artifacts_file,
            fieldnames=ARTIFACT_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        run_writer.writeheader()
        artifact_writer.writeheader()
        for value in artifacts:
            artifact_writer.writerow(value)

        for case in selected:
            executable = programs[case.name]
            directory = workloads / case.name
            directory.mkdir()
            if case.workload == "compression":
                compressed, compressed_digest = validate_compression(
                    case,
                    executable,
                    large_input,
                    input_digest,
                    directory,
                    environment,
                    commands,
                )
                decompressed = directory / "validation.decompressed"
                compression_artifact = artifact(
                    compressed,
                    benchmark=case.name,
                    kind="validation-compressed",
                    output=output,
                )
                decompression_artifact = artifact(
                    decompressed,
                    benchmark=case.name,
                    kind="validation-decompressed",
                    output=output,
                )
                artifact_writer.writerow(compression_artifact)
                artifact_writer.writerow(decompression_artifact)
                validation_hashes[(case.name, "compression")] = compressed_digest
                validation_hashes[(case.name, "decompression")] = input_digest
                measurements.extend(
                    run_samples(
                        case,
                        "compression",
                        [
                            os.fspath(executable),
                            *case.compression_arguments,
                            os.fspath(large_input),
                        ],
                        warmups=arguments.warmups,
                        samples=arguments.samples,
                        directory=directory,
                        environment=environment,
                        commands=commands,
                    )
                )
                measurements.extend(
                    run_samples(
                        case,
                        "decompression",
                        [
                            os.fspath(executable),
                            *case.decompression_arguments,
                            os.fspath(compressed),
                        ],
                        warmups=arguments.warmups,
                        samples=arguments.samples,
                        directory=directory,
                        environment=environment,
                        commands=commands,
                    )
                )
            else:
                command = [os.fspath(executable), os.fspath(lua_workload)]
                validation_stderr = directory / "validation.stderr.txt"
                record_command(
                    commands, case=case, kind="validate-interpreter", command=command
                )
                timing = measured_run(
                    command,
                    stdout=Path(os.devnull),
                    stderr=validation_stderr,
                    environment=environment,
                )
                write_json(directory / "validation.json", timing)
                ensure_success(
                    timing,
                    case=case,
                    operation="interpreter validation",
                    stderr=validation_stderr,
                )
                validation_hashes[(case.name, "interpreter")] = lua_digest
                measurements.extend(
                    run_samples(
                        case,
                        "interpreter",
                        command,
                        warmups=arguments.warmups,
                        samples=arguments.samples,
                        directory=directory,
                        environment=environment,
                        commands=commands,
                    )
                )

        for measurement in measurements:
            run_writer.writerow(
                {
                    "benchmark": measurement.case.name,
                    "family": measurement.case.family,
                    "operation": measurement.operation,
                    "phase": measurement.phase,
                    "iteration": measurement.iteration,
                    **timing_row(measurement.timing),
                }
            )

    with (output / "summary.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(
            file,
            fieldnames=SUMMARY_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        for case in selected:
            operations = (
                ("compression", "decompression")
                if case.workload == "compression"
                else ("interpreter",)
            )
            for operation in operations:
                samples = [
                    measurement
                    for measurement in measurements
                    if measurement.case.name == case.name
                    and measurement.operation == operation
                    and measurement.phase == "sample"
                ]
                if len(samples) != arguments.samples:
                    raise BenchmarkError(
                        f"{case.name} {operation} retained {len(samples)} samples; "
                        f"expected {arguments.samples}"
                    )
                wall = [float(sample.timing["wall_seconds"]) for sample in samples]
                user = [float(sample.timing["user_seconds"]) for sample in samples]
                system = [float(sample.timing["system_seconds"]) for sample in samples]
                rss = [int(sample.timing["peak_rss_bytes"]) for sample in samples]
                byte_workload = case.workload == "compression"
                work_count = input_bytes if byte_workload else lua_work_units
                bytes_per_second = (
                    input_bytes / statistics.median(wall) if byte_workload else None
                )
                writer.writerow(
                    {
                        "benchmark": case.name,
                        "family": case.family,
                        "operation": operation,
                        "work_unit": case.work_unit,
                        "work_count": work_count,
                        "input_bytes": input_bytes if byte_workload else 0,
                        "warmups": arguments.warmups,
                        "samples": arguments.samples,
                        "median_wall_seconds": f"{statistics.median(wall):.9f}",
                        "minimum_wall_seconds": f"{min(wall):.9f}",
                        "maximum_wall_seconds": f"{max(wall):.9f}",
                        "median_user_seconds": f"{statistics.median(user):.9f}",
                        "median_system_seconds": f"{statistics.median(system):.9f}",
                        "median_peak_rss_bytes": int(statistics.median(rss)),
                        "throughput_mib_per_second": (
                            f"{bytes_per_second / (1024 * 1024):.6f}"
                            if bytes_per_second is not None
                            else ""
                        ),
                        "validation_sha256": validation_hashes[(case.name, operation)],
                    }
                )

    write_json(
        output / "environment.json",
        {
            "format_version": FORMAT_VERSION,
            "created_at": datetime.now(timezone.utc).isoformat(),
            "host": {
                "system": platform.system(),
                "machine": platform.machine(),
                "release": platform.release(),
            },
            "input": {
                "path": os.fspath(large_input.relative_to(output)),
                "bytes": input_bytes,
                "sha256": input_digest,
            },
            "lua_workload": {
                "path": os.fspath(lua_workload.relative_to(output)),
                "work_units": lua_work_units,
                "sha256": lua_digest,
            },
            "warmups": arguments.warmups,
            "samples": arguments.samples,
            "programs": {
                case.name: {
                    "path": os.fspath(programs[case.name]),
                    "sha256": sha256(programs[case.name]),
                }
                for case in selected
            },
        },
    )
    return output


def main() -> int:
    manifest_path = Path(__file__).resolve().with_name("manifest.toml")
    try:
        cases = load_manifest(manifest_path)
        arguments = parse_arguments(tuple(case.name for case in cases))
        output = run(arguments, cases)
    except BenchmarkError as error:
        print(f"real-world benchmark: {error}", file=sys.stderr)
        return 1
    print(f"real-world benchmark results: {output}")
    print(f"summary: {output / 'summary.tsv'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
