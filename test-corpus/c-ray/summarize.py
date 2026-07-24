#!/usr/bin/env python3

"""Create a stable summary from C-Ray's retained raw result tables."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path
import re
import statistics
import sys
from typing import Dict, List


CODEGEN_STATS_SCHEMA = (
    "schema_version",
    "post_inline_ir.functions",
    "post_inline_ir.blocks",
    "post_inline_ir.values",
    "post_inline_ir.instructions",
    "post_inline_ir.call_instructions",
    "post_inline_ir.fixed_stack_slots",
    "post_inline_ir.fixed_stack_bytes",
    "post_inline_ir.dynamic_stack_slots",
    "post_inline_ir.signatures",
    "post_inline_ir.unused_signatures",
    "post_inline_ir.external_functions",
    "post_inline_ir.unused_external_functions",
    "post_inline_ir.global_values",
    "post_inline_ir.unused_global_values",
    "post_inline_ir.constants",
    "post_inline_ir.jump_tables",
    "primary_object.file_bytes",
    "primary_object.sections",
    "primary_object.symbols",
    "primary_object.defined_symbols",
    "primary_object.undefined_symbols",
    "primary_object.relocations",
    "primary_object.text_bytes",
    "primary_object.read_only_data_bytes",
    "primary_object.writable_data_bytes",
    "primary_object.bss_bytes",
    "primary_object.tls_data_bytes",
    "primary_object.tls_bss_bytes",
    "primary_object.unwind_bytes",
    "primary_object.debug_bytes",
    "primary_object.metadata_bytes",
    "primary_object.other_section_bytes",
)
PHASE_TIMING_SCHEMA_VERSION = "1"
PHASE_TIMING_METRICS = (
    "preprocessing",
    "parsing",
    "semantic_analysis",
    "ccc_ir_lowering",
    "ccc_ir_optimization",
    "codegen.total",
    "object_packaging",
    "pipeline",
)
PHASE_TIMING_SUMMARY_FIELDS = (
    ("compile_phase_preprocessing_nanoseconds", "preprocessing"),
    ("compile_phase_parsing_nanoseconds", "parsing"),
    ("compile_phase_semantic_analysis_nanoseconds", "semantic_analysis"),
    ("compile_phase_ccc_ir_lowering_nanoseconds", "ccc_ir_lowering"),
    ("compile_phase_ccc_ir_optimization_nanoseconds", "ccc_ir_optimization"),
    ("compile_phase_codegen_total_nanoseconds", "codegen.total"),
    ("compile_phase_object_packaging_nanoseconds", "object_packaging"),
    ("compile_phase_pipeline_nanoseconds", "pipeline"),
)
PHASE_TIMING_FIELDS = ("label", "metric", "value")
PHASE_ARTIFACT_FIELDS = (
    "label",
    "canonical_object_bytes",
    "canonical_object_sha256",
    "instrumented_object_bytes",
    "instrumented_object_sha256",
    "objects_match",
)
RESULT_LABELS = ("ccc-o0", "ccc-o2", "ccc-oz", "reference-o2")
CCC_LABELS = RESULT_LABELS[:3]


def read_rows(path: Path) -> List[dict]:
    with path.open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def read_exact_rows(path: Path, fields: tuple[str, ...]) -> List[dict]:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if tuple(reader.fieldnames or ()) != fields:
            raise ValueError(f"{path}: expected header {'/'.join(fields)}")
        rows = list(reader)
    for line_number, row in enumerate(rows, 2):
        if None in row or any(row[field] is None for field in fields):
            raise ValueError(f"{path}:{line_number}: malformed normalized row")
    return rows


def one_timing(timings: List[dict], label: str, stage: str) -> dict:
    matches = [
        row for row in timings if row["label"] == label and row["stage"] == stage
    ]
    if len(matches) != 1:
        raise ValueError(f"{label}: expected one {stage} timing, found {len(matches)}")
    return matches[0]


def one_object_sections(section_totals: List[dict], label: str) -> dict:
    matches = [row for row in section_totals if row["label"] == label]
    if len(matches) != 1:
        raise ValueError(
            f"{label}: expected one object-section total, found {len(matches)}"
        )
    return matches[0]


def codegen_stats_by_label(rows: List[dict]) -> Dict[str, Dict[str, str]]:
    result: Dict[str, Dict[str, str]] = {}
    for row in rows:
        label = row["label"]
        metric = row["metric"]
        value = row["value"]
        if not label or not metric:
            raise ValueError("codegen-stat labels and metrics must not be empty")
        try:
            parsed = int(value)
        except ValueError as error:
            raise ValueError(
                f"{label}: codegen stat {metric!r} is not an integer"
            ) from error
        if parsed < 0 or str(parsed) != value:
            raise ValueError(
                f"{label}: codegen stat {metric!r} is not canonical unsigned decimal"
            )
        metrics = result.setdefault(label, {})
        if metric in metrics:
            raise ValueError(f"{label}: duplicate codegen stat {metric!r}")
        metrics[metric] = value

    for label, metrics in result.items():
        if metrics.get("schema_version") != "3":
            raise ValueError(f"{label}: unsupported codegen statistics schema")
        if tuple(metrics) != CODEGEN_STATS_SCHEMA:
            missing = [metric for metric in CODEGEN_STATS_SCHEMA if metric not in metrics]
            unexpected = [metric for metric in metrics if metric not in CODEGEN_STATS_SCHEMA]
            if missing:
                raise ValueError(
                    f"{label}: missing codegen statistics {missing!r}"
                )
            if unexpected:
                raise ValueError(
                    f"{label}: unexpected codegen statistics {unexpected!r}"
                )
            raise ValueError(f"{label}: codegen statistics are out of schema order")
    return result


def phase_timings_by_label(rows: List[dict]) -> Dict[str, Dict[str, str]]:
    result: Dict[str, Dict[str, str]] = {}
    orders: Dict[str, List[str]] = {}
    for row in rows:
        label = row["label"]
        metric = row["metric"]
        value = row["value"]
        if not label or not metric:
            raise ValueError("phase-timing labels and metrics must not be empty")
        if re.fullmatch(r"(0|[1-9][0-9]*)", value) is None:
            raise ValueError(
                f"{label}: phase-timing metric {metric!r} "
                "is not canonical unsigned decimal"
            )
        metrics = result.setdefault(label, {})
        if metric in metrics:
            raise ValueError(f"{label}: duplicate phase-timing metric {metric!r}")
        metrics[metric] = value
        orders.setdefault(label, []).append(metric)

    expected_order = ("schema_version", *PHASE_TIMING_METRICS)
    for label, metrics in result.items():
        if metrics.get("schema_version") != PHASE_TIMING_SCHEMA_VERSION:
            found = metrics.get("schema_version", "missing")
            raise ValueError(f"{label}: unsupported phase-timing schema {found}")
        order = orders[label]
        if tuple(order) != expected_order:
            missing = [metric for metric in expected_order if metric not in metrics]
            unexpected = [metric for metric in order if metric not in expected_order]
            if missing:
                raise ValueError(f"{label}: missing phase timings {missing!r}")
            if unexpected:
                raise ValueError(f"{label}: unexpected phase timings {unexpected!r}")
            raise ValueError(f"{label}: phase timings are out of schema order")
    return result


def phase_artifacts_by_label(rows: List[dict]) -> Dict[str, dict]:
    result: Dict[str, dict] = {}
    for row in rows:
        label = row["label"]
        if not label:
            raise ValueError("phase-artifact labels must not be empty")
        if label in result:
            raise ValueError(f"{label}: duplicate phase-artifact record")
        for field in ("canonical_object_bytes", "instrumented_object_bytes"):
            value = row[field]
            if re.fullmatch(r"(0|[1-9][0-9]*)", value) is None:
                raise ValueError(
                    f"{label}: phase artifact {field!r} "
                    "is not canonical unsigned decimal"
                )
        for field in ("canonical_object_sha256", "instrumented_object_sha256"):
            if re.fullmatch(r"[0-9a-f]{64}", row[field]) is None:
                raise ValueError(f"{label}: phase artifact {field!r} is not SHA-256")
        if row["objects_match"] != "1":
            raise ValueError(
                f"{label}: phase-timing instrumentation did not match "
                "the measured object"
            )
        if (
            row["canonical_object_bytes"] != row["instrumented_object_bytes"]
            or row["canonical_object_sha256"]
            != row["instrumented_object_sha256"]
        ):
            raise ValueError(
                f"{label}: phase artifact claims a match for different objects"
            )
        result[label] = row
    return result


SUMMARY_CODEGEN_METRICS = {
    "clif_functions": "post_inline_ir.functions",
    "clif_blocks": "post_inline_ir.blocks",
    "clif_values": "post_inline_ir.values",
    "clif_instructions": "post_inline_ir.instructions",
    "clif_call_instructions": "post_inline_ir.call_instructions",
    "clif_fixed_stack_slots": "post_inline_ir.fixed_stack_slots",
    "clif_fixed_stack_bytes": "post_inline_ir.fixed_stack_bytes",
    "clif_dynamic_stack_slots": "post_inline_ir.dynamic_stack_slots",
    "clif_signatures": "post_inline_ir.signatures",
    "clif_unused_signatures": "post_inline_ir.unused_signatures",
    "clif_external_functions": "post_inline_ir.external_functions",
    "clif_unused_external_functions": "post_inline_ir.unused_external_functions",
    "clif_global_values": "post_inline_ir.global_values",
    "clif_unused_global_values": "post_inline_ir.unused_global_values",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timings", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--codegen-stats", required=True, type=Path)
    parser.add_argument("--phase-timings", required=True, type=Path)
    parser.add_argument("--phase-artifacts", required=True, type=Path)
    parser.add_argument("--object-sections", required=True, type=Path)
    parser.add_argument("--hashes", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    try:
        timings = read_rows(arguments.timings)
        artifacts = read_rows(arguments.artifacts)
        codegen_stats = codegen_stats_by_label(read_rows(arguments.codegen_stats))
        phase_timings = phase_timings_by_label(
            read_exact_rows(arguments.phase_timings, PHASE_TIMING_FIELDS)
        )
        phase_artifacts = phase_artifacts_by_label(
            read_exact_rows(arguments.phase_artifacts, PHASE_ARTIFACT_FIELDS)
        )
        object_sections = read_rows(arguments.object_sections)
        hashes = read_rows(arguments.hashes)
        if not artifacts:
            raise ValueError("artifact table is empty")
        artifact_labels = [artifact["label"] for artifact in artifacts]
        if tuple(artifact_labels) != RESULT_LABELS:
            missing = [label for label in RESULT_LABELS if label not in artifact_labels]
            unexpected = [
                label for label in artifact_labels if label not in RESULT_LABELS
            ]
            if missing or unexpected:
                raise ValueError(
                    "artifact labels do not match the result schema: "
                    f"missing {missing!r}; unexpected {unexpected!r}"
                )
            raise ValueError("artifact labels are out of result-schema order")
        ccc_labels = set(CCC_LABELS)
        phase_labels = set(phase_timings)
        phase_artifact_labels = set(phase_artifacts)
        if phase_labels != ccc_labels:
            missing = sorted(ccc_labels - phase_labels)
            unexpected = sorted(phase_labels - ccc_labels)
            raise ValueError(
                "phase-timing records do not match CCC artifacts: "
                f"missing {missing!r}; unexpected {unexpected!r}"
            )
        if phase_artifact_labels != ccc_labels:
            missing = sorted(ccc_labels - phase_artifact_labels)
            unexpected = sorted(phase_artifact_labels - ccc_labels)
            raise ValueError(
                "phase-artifact records do not match CCC artifacts: "
                f"missing {missing!r}; unexpected {unexpected!r}"
            )
        artifacts_by_label = {artifact["label"]: artifact for artifact in artifacts}
        for label, phase_artifact in phase_artifacts.items():
            if (
                phase_artifact["canonical_object_bytes"]
                != artifacts_by_label[label]["object_bytes"]
            ):
                raise ValueError(
                    f"{label}: phase artifact does not describe the measured object"
                )

        output_fields = (
            "label",
            "compile_wall_seconds",
            "link_wall_seconds",
            "render_samples",
            "render_median_seconds",
            "render_min_seconds",
            "render_max_seconds",
            "compile_peak_rss_bytes",
            "render_peak_rss_median_bytes",
            *(field for field, _metric in PHASE_TIMING_SUMMARY_FIELDS),
            *SUMMARY_CODEGEN_METRICS,
            "object_bytes",
            "object_text_bytes",
            "object_read_only_data_bytes",
            "object_writable_data_bytes",
            "object_bss_bytes",
            "object_unwind_bytes",
            "object_debug_bytes",
            "object_other_section_bytes",
            "object_total_section_bytes",
            "executable_bytes",
            "image_sha256",
        )
        result_rows = []
        for artifact in artifacts:
            label = artifact["label"]
            compile_timing = one_timing(timings, label, "compile")
            link_timing = one_timing(timings, label, "link")
            section_totals = one_object_sections(object_sections, label)
            label_codegen_stats = codegen_stats.get(label)
            label_phase_timings = phase_timings.get(label)
            if label in CCC_LABELS and label_codegen_stats is None:
                raise ValueError(f"{label}: codegen statistics are missing")
            if label_codegen_stats is not None:
                missing = [
                    metric
                    for metric in SUMMARY_CODEGEN_METRICS.values()
                    if metric not in label_codegen_stats
                ]
                if missing:
                    raise ValueError(
                        f"{label}: missing codegen statistics {missing!r}"
                    )
            render_timings = [
                row
                for row in timings
                if row["label"] == label and row["stage"] == "render-sample"
            ]
            if not render_timings:
                raise ValueError(f"{label}: no render samples")
            image_hashes = {
                row["sha256"] for row in hashes if row["label"] == label
            }
            if len(image_hashes) != 1:
                raise ValueError(
                    f"{label}: expected one stable image hash, found {len(image_hashes)}"
                )
            render_seconds = [float(row["wall_seconds"]) for row in render_timings]
            render_rss = [int(row["peak_rss_bytes"]) for row in render_timings]
            result_rows.append(
                {
                    "label": label,
                    "compile_wall_seconds": f"{float(compile_timing['wall_seconds']):.9f}",
                    "link_wall_seconds": f"{float(link_timing['wall_seconds']):.9f}",
                    "render_samples": str(len(render_timings)),
                    "render_median_seconds": f"{statistics.median(render_seconds):.9f}",
                    "render_min_seconds": f"{min(render_seconds):.9f}",
                    "render_max_seconds": f"{max(render_seconds):.9f}",
                    "compile_peak_rss_bytes": compile_timing["peak_rss_bytes"],
                    "render_peak_rss_median_bytes": str(
                        int(statistics.median(render_rss))
                    ),
                    **{
                        output_name: (
                            label_phase_timings[metric]
                            if label_phase_timings is not None
                            else ""
                        )
                        for output_name, metric in PHASE_TIMING_SUMMARY_FIELDS
                    },
                    **{
                        output_name: (
                            label_codegen_stats[metric]
                            if label_codegen_stats is not None
                            else ""
                        )
                        for output_name, metric in SUMMARY_CODEGEN_METRICS.items()
                    },
                    "object_bytes": artifact["object_bytes"],
                    "object_text_bytes": section_totals["text_bytes"],
                    "object_read_only_data_bytes": section_totals[
                        "read_only_data_bytes"
                    ],
                    "object_writable_data_bytes": section_totals[
                        "writable_data_bytes"
                    ],
                    "object_bss_bytes": section_totals["bss_bytes"],
                    "object_unwind_bytes": section_totals["unwind_bytes"],
                    "object_debug_bytes": section_totals["debug_bytes"],
                    "object_other_section_bytes": section_totals["other_bytes"],
                    "object_total_section_bytes": section_totals[
                        "total_section_bytes"
                    ],
                    "executable_bytes": artifact["executable_bytes"],
                    "image_sha256": next(iter(image_hashes)),
                }
            )

        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        with arguments.output.open("w", encoding="utf-8", newline="") as output:
            writer = csv.DictWriter(
                output, fieldnames=output_fields, delimiter="\t", lineterminator="\n"
            )
            writer.writeheader()
            writer.writerows(result_rows)
    except (csv.Error, KeyError, OSError, UnicodeError, ValueError) as error:
        print(f"C-Ray result summary failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
