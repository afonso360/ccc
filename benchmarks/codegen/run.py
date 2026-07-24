#!/usr/bin/env python3

"""Run small, reproducible CCC code-generation benchmarks."""

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
import re
import resource
import shutil
import statistics
import subprocess
import sys
import time
from typing import Iterable


FORMAT_VERSION = 3
CODEGEN_STATS_SCHEMA_VERSION = 2
PROFILE_FLAGS = {
    "O0": "-O0",
    "O1": "-O1",
    "O2": "-O2",
    "O3": "-O3",
    "Os": "-Os",
    "Oz": "-Oz",
}
CASE_NAMES = (
    "minimal-return",
    "puts-call",
    "printf-variadic",
    "hosted-header",
    "hosted-printf",
    "declaration-heavy",
    "data-declaration-heavy",
    "live-functions",
    "block-count",
    "ssa-values",
    "live-globals",
    "string-literals",
)
REQUIRED_METRICS = (
    "post_inline_ir.functions",
    "post_inline_ir.blocks",
    "post_inline_ir.values",
    "post_inline_ir.instructions",
    "post_inline_ir.call_instructions",
    "post_inline_ir.global_values",
    "primary_object.file_bytes",
    "primary_object.symbols",
    "primary_object.defined_symbols",
    "primary_object.undefined_symbols",
    "primary_object.relocations",
    "primary_object.text_bytes",
    "primary_object.read_only_data_bytes",
    "primary_object.writable_data_bytes",
)
DECLARATION_OBJECT_INVARIANTS = (
    "primary_object.file_bytes",
    "primary_object.symbols",
    "primary_object.undefined_symbols",
    "primary_object.relocations",
    "primary_object.text_bytes",
)
TIMING_FIELDS = (
    "benchmark",
    "family",
    "scale",
    "profile",
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
    "object_bytes",
    "object_sha256",
    "exit_status",
)
NORMALIZED_STATS_FIELDS = (
    "benchmark",
    "family",
    "scale",
    "profile",
    "metric",
    "value",
)
SUMMARY_FIELDS = (
    "benchmark",
    "family",
    "scale",
    "profile",
    "samples",
    "median_wall_seconds",
    "min_wall_seconds",
    "max_wall_seconds",
    "median_user_seconds",
    "median_system_seconds",
    "median_peak_rss_bytes",
    "post_inline_ir.functions",
    "post_inline_ir.blocks",
    "post_inline_ir.values",
    "post_inline_ir.instructions",
    "post_inline_ir.call_instructions",
    "post_inline_ir.global_values",
    "primary_object.file_bytes",
    "primary_object.symbols",
    "primary_object.defined_symbols",
    "primary_object.undefined_symbols",
    "primary_object.relocations",
    "primary_object.text_bytes",
    "primary_object.read_only_data_bytes",
    "primary_object.writable_data_bytes",
)


class BenchmarkError(Exception):
    """An actionable benchmark setup or execution error."""


@dataclass(frozen=True)
class Case:
    name: str
    family: str
    scale: int
    source: Path
    expected_functions: int
    equivalent_to: str | None = None
    primary_object_is_output: bool = True


@dataclass(frozen=True)
class CompileInvocation:
    case: Case
    profile: str
    phase: str
    iteration: int
    timing: dict[str, object]
    object_path: Path


@dataclass(frozen=True)
class StatsRecord:
    case: Case
    profile: str
    stats: dict[str, int]
    stats_order: tuple[str, ...]


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


def scales(value: str, *, label: str) -> list[int]:
    raw_values = comma_separated(value, label=label)
    parsed: list[int] = []
    for raw_value in raw_values:
        if not raw_value.isdecimal():
            raise BenchmarkError(f"{label} values must be nonnegative integers")
        parsed.append(int(raw_value))
    if len(set(parsed)) != len(parsed):
        raise BenchmarkError(f"{label} contains a duplicate value")
    return parsed


def parse_arguments() -> argparse.Namespace:
    benchmark_directory = Path(__file__).resolve().parent
    repository = benchmark_directory.parents[1]
    parser = argparse.ArgumentParser(
        description=(
            "Measure small CCC code-generation workloads and retain raw, "
            "machine-readable evidence."
        )
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
        "--profiles",
        default="O0,O2,Oz",
        help="comma-separated optimization profiles (default: O0,O2,Oz)",
    )
    parser.add_argument(
        "--cases",
        default=",".join(CASE_NAMES),
        help="comma-separated case families (default: all)",
    )
    parser.add_argument(
        "--declaration-scales",
        default="0,32,256,1024",
        help="generated function declaration counts (default: 0,32,256,1024)",
    )
    parser.add_argument(
        "--data-declaration-scales",
        default="0,32,256,1024",
        help="generated data declaration counts (default: 0,32,256,1024)",
    )
    parser.add_argument(
        "--function-scales",
        default="8,32,128",
        help="generated live function counts (default: 8,32,128)",
    )
    parser.add_argument(
        "--block-scales",
        default="0,16,64,256",
        help="generated live conditional counts (default: 0,16,64,256)",
    )
    parser.add_argument(
        "--value-scales",
        default="0,32,256,1024",
        help="generated dependent SSA-operation counts (default: 0,32,256,1024)",
    )
    parser.add_argument(
        "--global-scales",
        default="0,32,256,1024",
        help="generated live global-object counts (default: 0,32,256,1024)",
    )
    parser.add_argument(
        "--string-scales",
        default="0,32,256,1024",
        help="generated distinct live string-literal counts (default: 0,32,256,1024)",
    )
    parser.add_argument(
        "--warmups",
        default=1,
        type=int,
        help="warmups per case/profile, excluded from summary (default: 1)",
    )
    parser.add_argument(
        "--samples",
        default=5,
        type=int,
        help="measured invocations per case/profile (default: 5)",
    )
    parser.add_argument(
        "--target",
        help="optional enabled CCC target triple",
    )
    arguments = parser.parse_args()
    try:
        arguments.profiles = comma_separated(
            arguments.profiles, label="--profiles", allowed=PROFILE_FLAGS
        )
        arguments.cases = comma_separated(
            arguments.cases, label="--cases", allowed=CASE_NAMES
        )
        arguments.declaration_scales = scales(
            arguments.declaration_scales, label="--declaration-scales"
        )
        arguments.data_declaration_scales = scales(
            arguments.data_declaration_scales,
            label="--data-declaration-scales",
        )
        arguments.function_scales = scales(
            arguments.function_scales, label="--function-scales"
        )
        arguments.block_scales = scales(
            arguments.block_scales, label="--block-scales"
        )
        arguments.value_scales = scales(
            arguments.value_scales, label="--value-scales"
        )
        arguments.global_scales = scales(
            arguments.global_scales, label="--global-scales"
        )
        arguments.string_scales = scales(
            arguments.string_scales, label="--string-scales"
        )
        if arguments.warmups < 0:
            raise BenchmarkError("--warmups must be nonnegative")
        if arguments.samples <= 0:
            raise BenchmarkError("--samples must be positive")
    except BenchmarkError as error:
        parser.error(str(error))
    return arguments


def resolve_executable(value: str) -> Path:
    if os.sep not in value:
        found = shutil.which(value)
        if found is None:
            raise BenchmarkError(f"compiler is not available: {value}")
        path = Path(found)
    else:
        path = Path(value)
    try:
        path = path.expanduser().resolve(strict=True)
    except OSError as error:
        raise BenchmarkError(f"compiler does not exist: {value}") from error
    if not path.is_file() or not os.access(path, os.X_OK):
        raise BenchmarkError(f"compiler is not executable: {path}")
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


def declaration_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: declaration-heavy */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
    ]
    for index in range(scale):
        lines.append(f"extern long ccc_decl_{index:06d}(long value);")
    lines.extend(("", "int main(void) {", "    return 0;", "}", ""))
    return "\n".join(lines)


def data_declaration_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: data-declaration-heavy */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
    ]
    for index in range(scale):
        lines.append(f"extern long ccc_data_decl_{index:06d};")
    lines.extend(("", "int main(void) {", "    return 0;", "}", ""))
    return "\n".join(lines)


def live_functions_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: live-functions */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
    ]
    for index in range(scale):
        if index == 0:
            expression = "value + 1"
        else:
            expression = f"ccc_live_{index - 1:06d}(value) + {index + 1}"
        lines.extend(
            (
                f"long ccc_live_{index:06d}(long value) {{",
                f"    return {expression};",
                "}",
                "",
            )
        )
    lines.append("int main(void) {")
    if scale == 0:
        lines.append("    return 0;")
    else:
        lines.append(f"    return (int)(ccc_live_{scale - 1:06d}(7) & 255);")
    lines.extend(("}", ""))
    return "\n".join(lines)


def block_count_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: block-count */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
        "__attribute__((noinline))",
        "unsigned ccc_block_path(unsigned selector) {",
    ]
    for index in range(scale):
        lines.extend(
            (
                f"    if (selector == {index}u)",
                f"        return selector + {index + 17}u;",
            )
        )
    lines.extend(
        (
            "    return selector ^ 0x9e3779b9u;",
            "}",
            "",
            "int main(int argc, char **argv) {",
            "    (void)argv;",
            "    return (int)(ccc_block_path((unsigned)argc) & 255u);",
            "}",
            "",
        )
    )
    return "\n".join(lines)


def ssa_values_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: ssa-values */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
        "__attribute__((noinline))",
        "unsigned ccc_value_chain(unsigned value) {",
    ]
    for index in range(scale):
        addend = 1_013_904_223 ^ index
        lines.append(f"    value = value * 1664525u + {addend}u;")
    lines.extend(
        (
            "    return value;",
            "}",
            "",
            "int main(int argc, char **argv) {",
            "    (void)argv;",
            "    return (int)(ccc_value_chain((unsigned)argc) & 255u);",
            "}",
            "",
        )
    )
    return "\n".join(lines)


def live_globals_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: live-globals */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
    ]
    for index in range(scale):
        initializer = ((index + 1) * 2_654_435_761) & 0xFFFF_FFFF
        lines.append(f"unsigned ccc_global_{index:06d} = {initializer}u;")
    lines.extend(
        (
            "",
            "__attribute__((noinline))",
            "unsigned ccc_read_globals(unsigned value) {",
        )
    )
    for index in range(scale):
        lines.append(f"    value ^= ccc_global_{index:06d} + {index + 1}u;")
    lines.extend(
        (
            "    return value;",
            "}",
            "",
            "int main(int argc, char **argv) {",
            "    (void)argv;",
            "    return (int)(ccc_read_globals((unsigned)argc) & 255u);",
            "}",
            "",
        )
    )
    return "\n".join(lines)


def string_literals_source(scale: int) -> str:
    lines = [
        "/* ccc-benchmark-family: string-literals */",
        f"/* ccc-benchmark-scale: {scale} */",
        "",
        "__attribute__((noinline))",
        "unsigned ccc_read_strings(unsigned index) {",
        "    unsigned value = index;",
    ]
    for literal_index in range(scale):
        lines.append(
            "    value += (unsigned)(unsigned char)"
            f'"ccc-string-{literal_index:06d}"'
            f"[(index + {literal_index}u) % 17u];"
        )
    lines.extend(
        (
            "    return value;",
            "}",
            "",
            "int main(int argc, char **argv) {",
            "    (void)argv;",
            "    return (int)(ccc_read_strings((unsigned)argc) & 255u);",
            "}",
            "",
        )
    )
    return "\n".join(lines)


def copy_and_generate_cases(
    selected: list[str],
    declaration_scales: list[int],
    data_declaration_scales: list[int],
    function_scales: list[int],
    block_scales: list[int],
    value_scales: list[int],
    global_scales: list[int],
    string_scales: list[int],
    output: Path,
) -> list[Case]:
    benchmark_directory = Path(__file__).resolve().parent
    source_directory = output / "sources"
    source_directory.mkdir()
    cases: list[Case] = []

    for name in ("minimal-return", "puts-call", "printf-variadic"):
        if name not in selected:
            continue
        source = source_directory / f"{name}.c"
        shutil.copyfile(benchmark_directory / "cases" / f"{name}.c", source)
        cases.append(
            Case(
                name,
                name,
                1,
                source,
                1,
                primary_object_is_output=name != "printf-variadic",
            )
        )

    if "hosted-header" in selected:
        baseline = "hosted-header-minimal"
        for name, equivalent_to in (
            (baseline, None),
            ("hosted-header-stdio", baseline),
        ):
            source = source_directory / f"{name}.c"
            shutil.copyfile(benchmark_directory / "cases" / f"{name}.c", source)
            cases.append(
                Case(
                    name,
                    "hosted-header",
                    1,
                    source,
                    1,
                    equivalent_to,
                )
            )

    if "hosted-printf" in selected:
        baseline = "hosted-printf-minimal"
        for name, equivalent_to in (
            (baseline, None),
            ("hosted-printf-stdio", baseline),
        ):
            source = source_directory / f"{name}.c"
            shutil.copyfile(benchmark_directory / "cases" / f"{name}.c", source)
            cases.append(
                Case(
                    name,
                    "hosted-printf",
                    1,
                    source,
                    1,
                    equivalent_to,
                    False,
                )
            )

    if "declaration-heavy" in selected:
        for scale in declaration_scales:
            name = f"declaration-heavy-{scale}"
            source = source_directory / f"{name}.c"
            source.write_text(declaration_source(scale), encoding="utf-8")
            cases.append(Case(name, "declaration-heavy", scale, source, 1))

    if "data-declaration-heavy" in selected:
        for scale in data_declaration_scales:
            name = f"data-declaration-heavy-{scale}"
            source = source_directory / f"{name}.c"
            source.write_text(data_declaration_source(scale), encoding="utf-8")
            cases.append(
                Case(name, "data-declaration-heavy", scale, source, 1)
            )

    if "live-functions" in selected:
        for scale in function_scales:
            name = f"live-functions-{scale}"
            source = source_directory / f"{name}.c"
            source.write_text(live_functions_source(scale), encoding="utf-8")
            cases.append(Case(name, "live-functions", scale, source, scale + 1))

    generated_axes = (
        ("block-count", block_scales, block_count_source),
        ("ssa-values", value_scales, ssa_values_source),
        ("live-globals", global_scales, live_globals_source),
        ("string-literals", string_scales, string_literals_source),
    )
    for family, family_scales, source_generator in generated_axes:
        if family not in selected:
            continue
        for scale in family_scales:
            name = f"{family}-{scale}"
            source = source_directory / f"{name}.c"
            source.write_text(source_generator(scale), encoding="utf-8")
            cases.append(Case(name, family, scale, source, 2))

    return cases


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_manifest(cases: list[Case], output: Path) -> None:
    with (output / "manifest.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.writer(file, delimiter="\t", lineterminator="\n")
        writer.writerow(
            (
                "benchmark",
                "family",
                "scale",
                "equivalent_to",
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
                    case.scale,
                    case.equivalent_to or "",
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
) -> dict[str, int | float | str | list[str] | None]:
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
                    f"{path}:{line_number}: expected one tab-separated metric/value row"
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
        raise BenchmarkError(f"{path}: missing required metric(s): {', '.join(missing)}")
    return stats, tuple(order)


def write_json(path: Path, value: object) -> None:
    temporary = path.with_name(f"{path.name}.part.{os.getpid()}")
    temporary.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def timing_row(invocation: CompileInvocation) -> dict[str, object]:
    timing = invocation.timing
    row: dict[str, object] = {
        "benchmark": invocation.case.name,
        "family": invocation.case.family,
        "scale": invocation.case.scale,
        "profile": invocation.profile,
        "phase": invocation.phase,
        "iteration": invocation.iteration,
        **{field: timing[field] for field in TIMING_FIELDS[6:]},
    }
    for field in ("wall_seconds", "user_seconds", "system_seconds"):
        row[field] = f"{float(timing[field]):.9f}"
    return row


def validate_invariants(
    invocations: list[CompileInvocation],
    records: list[StatsRecord],
) -> None:
    stats_by_case = {
        (record.case.name, record.profile): record.stats for record in records
    }
    for record in records:
        if record.case.equivalent_to is None:
            continue
        baseline_key = (record.case.equivalent_to, record.profile)
        if baseline_key not in stats_by_case:
            raise BenchmarkError(
                f"{record.case.name} at -{record.profile} has no "
                f"{record.case.equivalent_to} baseline"
            )
        baseline = stats_by_case[baseline_key]
        for prefix, label in (
            ("post_inline_ir.", "post-inline CLIF"),
            ("primary_object.", "primary-object structure"),
        ):
            baseline_metrics = {
                metric: value
                for metric, value in baseline.items()
                if metric.startswith(prefix)
            }
            current_metrics = {
                metric: value
                for metric, value in record.stats.items()
                if metric.startswith(prefix)
            }
            differences = [
                (
                    metric,
                    baseline_metrics.get(metric),
                    current_metrics.get(metric),
                )
                for metric in sorted(baseline_metrics.keys() | current_metrics.keys())
                if baseline_metrics.get(metric) != current_metrics.get(metric)
            ]
            if differences:
                rendered = ", ".join(
                    f"{metric}={baseline_value} versus {current_value}"
                    for metric, baseline_value, current_value in differences
                )
                raise BenchmarkError(
                    f"{record.case.name} changed {label} relative to "
                    f"{record.case.equivalent_to} at -{record.profile}: "
                    f"{rendered}"
                )

    for record in records:
        functions = record.stats["post_inline_ir.functions"]
        if functions != record.case.expected_functions:
            raise BenchmarkError(
                f"{record.case.name} at -{record.profile} emitted "
                f"{functions} post-inline functions; expected "
                f"{record.case.expected_functions}"
            )

    for invocation in invocations:
        if not invocation.case.primary_object_is_output:
            continue
        expected_bytes = stats_by_case[(invocation.case.name, invocation.profile)][
            "primary_object.file_bytes"
        ]
        if invocation.timing["object_bytes"] != expected_bytes:
            raise BenchmarkError(
                f"{invocation.case.name} at -{invocation.profile} produced "
                f"{invocation.timing['object_bytes']} timed object bytes but "
                f"codegen-stats reported {expected_bytes}"
            )

    sample_groups: dict[tuple[str, str], list[CompileInvocation]] = {}
    for invocation in invocations:
        if invocation.phase == "sample":
            sample_groups.setdefault(
                (invocation.case.name, invocation.profile), []
            ).append(invocation)
    for (case, profile), group in sample_groups.items():
        first = group[0]
        for invocation in group[1:]:
            if invocation.timing["object_sha256"] != first.timing["object_sha256"]:
                raise BenchmarkError(
                    f"{case} at -{profile} produced nondeterministic object files"
                )

    declaration_labels = {
        "declaration-heavy": "function declarations",
        "data-declaration-heavy": "data declarations",
    }
    declarations: dict[tuple[str, str], list[StatsRecord]] = {}
    for record in records:
        if record.case.family in declaration_labels:
            declarations.setdefault(
                (record.case.family, record.profile), []
            ).append(record)
    for (family, profile), group in declarations.items():
        if len(group) < 2:
            continue
        first = group[0]
        first_ir = {
            metric: value
            for metric, value in first.stats.items()
            if metric.startswith("post_inline_ir.")
            or metric in DECLARATION_OBJECT_INVARIANTS
        }
        for record in group[1:]:
            current_ir = {
                metric: value
                for metric, value in record.stats.items()
                if metric.startswith("post_inline_ir.")
                or metric in DECLARATION_OBJECT_INVARIANTS
            }
            if current_ir != first_ir:
                raise BenchmarkError(
                    f"unused {declaration_labels[family]} changed post-inline "
                    "CLIF or primary-object metrics at "
                    f"-{profile}: {first.case.scale} versus "
                    f"{record.case.scale} declarations"
                )

    axis_bounds = {
        "block-count": {
            "post_inline_ir.blocks": (1, 4),
            "post_inline_ir.values": (6, 14),
            "post_inline_ir.instructions": (8, 14),
        },
        "ssa-values": {
            "post_inline_ir.values": (2, 5),
            "post_inline_ir.instructions": (2, 5),
        },
        "live-globals": {
            "post_inline_ir.values": (3, 6),
            "post_inline_ir.instructions": (3, 6),
            "post_inline_ir.global_values": (1, 2),
            "primary_object.defined_symbols": (1, 2),
            "primary_object.writable_data_bytes": (4, 4),
        },
        "string-literals": {
            "post_inline_ir.global_values": (1, 2),
            "primary_object.relocations": (1, 2),
            "primary_object.read_only_data_bytes": (18, 32),
        },
    }
    axis_records: dict[tuple[str, str], list[StatsRecord]] = {}
    for record in records:
        if record.case.family in axis_bounds:
            axis_records.setdefault(
                (record.case.family, record.profile), []
            ).append(record)
    for (family, profile), group in axis_records.items():
        if len(group) < 2:
            raise BenchmarkError(
                f"{family} at -{profile} requires at least two scales for "
                "structural growth validation"
            )
        ordered = sorted(group, key=lambda record: record.case.scale)
        for previous, record in zip(ordered, ordered[1:]):
            scale_growth = record.case.scale - previous.case.scale
            for metric, (minimum_per_item, maximum_per_item) in axis_bounds[
                family
            ].items():
                metric_growth = record.stats[metric] - previous.stats[metric]
                minimum = minimum_per_item * scale_growth
                maximum = maximum_per_item * scale_growth
                if not minimum <= metric_growth <= maximum:
                    raise BenchmarkError(
                        f"{family} at -{profile} produced non-linear structural "
                        f"growth for {metric}: scales {previous.case.scale} to "
                        f"{record.case.scale} changed the metric by "
                        f"{metric_growth}; expected {minimum}..{maximum}"
                    )

        if family == "ssa-values":
            baseline = ordered[0]
            baseline_blocks = baseline.stats["post_inline_ir.blocks"]
            for record in ordered[1:]:
                if record.stats["post_inline_ir.blocks"] != baseline_blocks:
                    raise BenchmarkError(
                        f"ssa-values at -{profile} changed block count across "
                        f"scales {baseline.case.scale} and {record.case.scale}"
                    )


def format_seconds(value: float) -> str:
    return f"{value:.9f}"


def write_summary(
    invocations: list[CompileInvocation],
    records: list[StatsRecord],
    output: Path,
) -> None:
    groups: dict[tuple[str, str], list[CompileInvocation]] = {}
    for invocation in invocations:
        if invocation.phase == "sample":
            groups.setdefault((invocation.case.name, invocation.profile), []).append(
                invocation
            )
    stats_by_case = {
        (record.case.name, record.profile): record.stats for record in records
    }

    with (output / "summary.tsv").open("w", encoding="utf-8", newline="") as file:
        writer = csv.DictWriter(
            file, fieldnames=SUMMARY_FIELDS, delimiter="\t", lineterminator="\n"
        )
        writer.writeheader()
        for key, group in groups.items():
            first = group[0]
            stats = stats_by_case[key]
            wall = [float(item.timing["wall_seconds"]) for item in group]
            user = [float(item.timing["user_seconds"]) for item in group]
            system = [float(item.timing["system_seconds"]) for item in group]
            rss = [int(item.timing["peak_rss_bytes"]) for item in group]
            writer.writerow(
                {
                    "benchmark": first.case.name,
                    "family": first.case.family,
                    "scale": first.case.scale,
                    "profile": first.profile,
                    "samples": len(group),
                    "median_wall_seconds": format_seconds(statistics.median(wall)),
                    "min_wall_seconds": format_seconds(min(wall)),
                    "max_wall_seconds": format_seconds(max(wall)),
                    "median_user_seconds": format_seconds(statistics.median(user)),
                    "median_system_seconds": format_seconds(
                        statistics.median(system)
                    ),
                    "median_peak_rss_bytes": round(statistics.median(rss)),
                    **{
                        metric: stats[metric]
                        for metric in REQUIRED_METRICS
                    },
                }
            )


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


def run(arguments: argparse.Namespace) -> Path:
    compiler = resolve_executable(arguments.ccc)
    output = prepare_output(arguments.output)
    cases = copy_and_generate_cases(
        arguments.cases,
        arguments.declaration_scales,
        arguments.data_declaration_scales,
        arguments.function_scales,
        arguments.block_scales,
        arguments.value_scales,
        arguments.global_scales,
        arguments.string_scales,
        output,
    )
    if not cases:
        raise BenchmarkError("no benchmark cases were selected")
    write_manifest(cases, output)
    effective_target, target_query = compiler_target(
        compiler, arguments.target, output
    )

    environment = {
        "format_version": FORMAT_VERSION,
        "codegen_stats_schema_version": CODEGEN_STATS_SCHEMA_VERSION,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "compiler": os.fspath(compiler),
        "compiler_sha256": sha256(compiler),
        "compiler_version": compiler_version(compiler, output),
        "requested_target": arguments.target,
        "target": effective_target,
        "target_query": target_query,
        "effective_configs": effective_configs(
            compiler, effective_target, arguments.profiles, output
        ),
        "host": {
            "machine": platform.machine(),
            "platform": platform.platform(),
            "python": platform.python_version(),
            "system": platform.system(),
        },
        "profiles": arguments.profiles,
        "warmups": arguments.warmups,
        "samples": arguments.samples,
        "declaration_scales": arguments.declaration_scales,
        "data_declaration_scales": arguments.data_declaration_scales,
        "function_scales": arguments.function_scales,
        "block_scales": arguments.block_scales,
        "value_scales": arguments.value_scales,
        "global_scales": arguments.global_scales,
        "string_scales": arguments.string_scales,
    }
    write_json(output / "environment.json", environment)

    raw_directory = output / "raw"
    raw_directory.mkdir()
    invocations: list[CompileInvocation] = []
    stats_records: list[StatsRecord] = []
    commands_file = (output / "commands.jsonl").open("w", encoding="utf-8")
    timings_file = (output / "compile-times.tsv").open(
        "w", encoding="utf-8", newline=""
    )
    stats_file = (output / "codegen-stats.tsv").open(
        "w", encoding="utf-8", newline=""
    )
    try:
        timing_writer = csv.DictWriter(
            timings_file,
            fieldnames=TIMING_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        timing_writer.writeheader()
        stats_writer = csv.DictWriter(
            stats_file,
            fieldnames=NORMALIZED_STATS_FIELDS,
            delimiter="\t",
            lineterminator="\n",
        )
        stats_writer.writeheader()

        for case in cases:
            for profile in arguments.profiles:
                invocation_directory = raw_directory / case.name / profile
                invocation_directory.mkdir(parents=True, exist_ok=True)
                for phase, count in (
                    ("warmup", arguments.warmups),
                    ("sample", arguments.samples),
                ):
                    for iteration in range(1, count + 1):
                        stem = f"{phase}-{iteration:03d}"
                        raw_stdout = invocation_directory / f"{stem}.stdout.txt"
                        raw_stderr = invocation_directory / f"{stem}.stderr.txt"
                        object_path = invocation_directory / f"{stem}.o"
                        timing_json = invocation_directory / f"{stem}.timing.json"
                        command = [
                            os.fspath(compiler),
                            "-c",
                            PROFILE_FLAGS[profile],
                            f"--target={effective_target}",
                            os.fspath(case.source),
                            "-o",
                            os.fspath(object_path),
                        ]
                        commands_file.write(
                            json.dumps(
                                {
                                    "benchmark": case.name,
                                    "kind": "object-compile",
                                    "profile": profile,
                                    "phase": phase,
                                    "iteration": iteration,
                                    "command": command,
                                    "object": os.fspath(object_path),
                                    "timed": True,
                                },
                                sort_keys=True,
                                separators=(",", ":"),
                            )
                            + "\n"
                        )
                        commands_file.flush()
                        timing = measured_run(command, raw_stdout, raw_stderr)
                        if timing["exit_status"] != 0:
                            write_json(timing_json, timing)
                            raise BenchmarkError(
                                f"{case.name} at -{profile} failed with status "
                                f"{timing['exit_status']}; see {raw_stderr}"
                            )
                        if not object_path.is_file():
                            write_json(timing_json, timing)
                            raise BenchmarkError(
                                f"{case.name} at -{profile} did not create "
                                f"{object_path}"
                            )
                        timing["object_bytes"] = object_path.stat().st_size
                        timing["object_sha256"] = sha256(object_path)
                        write_json(timing_json, timing)
                        invocation = CompileInvocation(
                            case,
                            profile,
                            phase,
                            iteration,
                            timing,
                            object_path,
                        )
                        invocations.append(invocation)
                        timing_writer.writerow(timing_row(invocation))
                        timings_file.flush()

                raw_stats = invocation_directory / "codegen-stats.stdout.tsv"
                stats_stderr = invocation_directory / "codegen-stats.stderr.txt"
                stats_result = invocation_directory / "codegen-stats.result.json"
                stats_command = [
                    os.fspath(compiler),
                    "--emit=codegen-stats",
                    PROFILE_FLAGS[profile],
                    f"--target={effective_target}",
                    os.fspath(case.source),
                ]
                commands_file.write(
                    json.dumps(
                        {
                            "benchmark": case.name,
                            "kind": "codegen-stats",
                            "profile": profile,
                            "command": stats_command,
                            "timed": False,
                        },
                        sort_keys=True,
                        separators=(",", ":"),
                    )
                    + "\n"
                )
                commands_file.flush()
                stats_status = capture_command(
                    stats_command, raw_stats, stats_stderr
                )
                write_json(
                    stats_result,
                    {
                        "format_version": FORMAT_VERSION,
                        "command": stats_command,
                        "exit_status": stats_status,
                        "stdout": os.fspath(raw_stats),
                        "stderr": os.fspath(stats_stderr),
                        "timed": False,
                    },
                )
                if stats_status != 0:
                    raise BenchmarkError(
                        f"{case.name} at -{profile} codegen-stats failed with "
                        f"status {stats_status}; see {stats_stderr}"
                    )
                stats, stats_order = parse_stats(raw_stats)
                record = StatsRecord(case, profile, stats, stats_order)
                stats_records.append(record)
                for metric in stats_order:
                    stats_writer.writerow(
                        {
                            "benchmark": case.name,
                            "family": case.family,
                            "scale": case.scale,
                            "profile": profile,
                            "metric": metric,
                            "value": stats[metric],
                        }
                    )
                stats_file.flush()
    finally:
        commands_file.close()
        timings_file.close()
        stats_file.close()

    validate_invariants(invocations, stats_records)
    write_summary(invocations, stats_records, output)
    return output


def main() -> int:
    arguments = parse_arguments()
    try:
        output = run(arguments)
    except BenchmarkError as error:
        print(f"codegen benchmark: {error}", file=sys.stderr)
        return 1
    print(f"codegen benchmark results: {output}")
    print(f"summary: {output / 'summary.tsv'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
