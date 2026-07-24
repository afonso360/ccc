#!/usr/bin/env python3

"""Create a stable summary from C-Ray's retained raw result tables."""

import argparse
import csv
from pathlib import Path
import statistics
import sys
from typing import Dict, List


def read_rows(path: Path) -> List[dict]:
    with path.open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timings", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--object-sections", required=True, type=Path)
    parser.add_argument("--hashes", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    try:
        timings = read_rows(arguments.timings)
        artifacts = read_rows(arguments.artifacts)
        object_sections = read_rows(arguments.object_sections)
        hashes = read_rows(arguments.hashes)
        if not artifacts:
            raise ValueError("artifact table is empty")

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
    except (KeyError, OSError, ValueError) as error:
        print(f"C-Ray result summary failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
