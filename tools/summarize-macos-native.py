#!/usr/bin/env python3
"""Aggregate alternating macOS native probe logs without third-party packages."""

from __future__ import annotations

import csv
import re
import statistics
import sys
from pathlib import Path
from typing import Optional


TIMING = re.compile(
    r"^(?P<metric>native_[a-z0-9_]+).*?"
    r"p50=(?P<p50>\d+)ns p95=(?P<p95>\d+)ns p99=(?P<p99>\d+)ns$"
)
SINGLE = re.compile(r"^(?P<metric>native_[a-z0-9_]+) elapsed_ns=(?P<value>\d+)$")
METRICS = re.compile(
    r'^native_metrics stage=(?P<stage>[a-z0-9_]+) ps="(?P<rss>\d+)\s+(?P<threads>\d+)" '
    r"Physical footprint:\s*(?P<footprint>[0-9.]+)(?P<unit>[KMG])"
)
TARGET = re.compile(
    r"^native_(?P<source>ax|vision|hybrid)_target "
    r"name_hex=(?P<name>[0-9a-f]+) role_hex=(?P<role>[0-9a-f]+) "
    r"x=(?P<x>-?[0-9.]+) y=(?P<y>-?[0-9.]+) "
    r"width=(?P<width>[0-9.]+) height=(?P<height>[0-9.]+)$"
)
EXPECTED_FIXTURE_NAMES = {f"Fixture Button {index:02}" for index in range(24)}


def bytes_for(value: str, unit: str) -> int:
    multiplier = {"K": 1024, "M": 1024**2, "G": 1024**3}[unit]
    return round(float(value) * multiplier)


def percentile(values: list[int], percent: int) -> int:
    ordered = sorted(values)
    index = ((len(ordered) - 1) * percent + 99) // 100
    return ordered[index]


def collect(path: Path) -> dict[tuple[str, str], list[int]]:
    values: dict[tuple[str, str], list[int]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if match := TIMING.match(line):
            for percentile in ("p50", "p95", "p99"):
                values.setdefault((match["metric"], percentile), []).append(
                    int(match[percentile])
                )
        elif match := SINGLE.match(line):
            values.setdefault((match["metric"], "elapsed"), []).append(
                int(match["value"])
            )
        elif match := METRICS.match(line):
            stage = f'native_metrics_{match["stage"]}'
            values.setdefault((stage, "rss_bytes"), []).append(int(match["rss"]) * 1024)
            values.setdefault((stage, "threads"), []).append(int(match["threads"]))
            values.setdefault((stage, "footprint_bytes"), []).append(
                bytes_for(match["footprint"], match["unit"])
            )
    return values


def collect_targets(path: Path) -> dict[str, list[tuple[str, str, tuple[float, ...]]]]:
    targets: dict[str, list[tuple[str, str, tuple[float, ...]]]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not (match := TARGET.match(line)):
            continue
        rect = tuple(
            float(match[field]) for field in ("x", "y", "width", "height")
        )
        targets.setdefault(match["source"], []).append(
            (
                bytes.fromhex(match["name"]).decode("utf-8"),
                bytes.fromhex(match["role"]).decode("utf-8"),
                rect,
            )
        )
    return targets


def validate_fixture_targets(
    baseline_path: Path, optimized_path: Path
) -> list[str]:
    errors: list[str] = []
    baseline = collect_targets(baseline_path)
    optimized = collect_targets(optimized_path)
    for source in ("ax", "vision", "hybrid"):
        left = sorted(baseline.get(source, []))
        right = sorted(optimized.get(source, []))
        left_names = {target[0] for target in left}
        right_names = {target[0] for target in right}
        if left_names != EXPECTED_FIXTURE_NAMES:
            missing = sorted(EXPECTED_FIXTURE_NAMES - left_names)
            errors.append(f"baseline {source} fixture targets missing {missing}")
        if right_names != EXPECTED_FIXTURE_NAMES:
            missing = sorted(EXPECTED_FIXTURE_NAMES - right_names)
            errors.append(f"optimized {source} fixture targets missing {missing}")
        if len(left) != len(right):
            errors.append(
                f"{source} fixture target count changed: {len(left)} != {len(right)}"
            )
            continue
        tolerance = 0.000001 if source == "ax" else 1.0
        for baseline_target, optimized_target in zip(left, right):
            if baseline_target[:2] != optimized_target[:2]:
                errors.append(
                    f"{source} target identity changed: "
                    f"{baseline_target[:2]} != {optimized_target[:2]}"
                )
                continue
            difference = max(
                abs(before - after)
                for before, after in zip(baseline_target[2], optimized_target[2])
            )
            if difference > tolerance:
                errors.append(
                    f"{source} target {baseline_target[0]!r} moved by "
                    f"{difference:.6f} logical pixels"
                )
    return errors


def aggregate_value(
    grouped: dict[str, dict[tuple[str, str], list[int]]],
    variant: str,
    metric: str,
    stat: str,
) -> Optional[int]:
    samples = grouped[variant].get((metric, stat))
    if samples:
        return round(statistics.median(samples))
    elapsed = grouped[variant].get((metric, "elapsed"))
    if elapsed and stat in {"p50", "p95", "p99"}:
        return percentile(elapsed, int(stat[1:]))
    return None


def percent_change(baseline: int, optimized: int) -> float:
    return (optimized - baseline) * 100.0 / baseline if baseline else 0.0


def validate_performance(
    grouped: dict[str, dict[tuple[str, str], list[int]]],
) -> list[str]:
    errors: list[str] = []

    def change(metric: str, stat: str) -> Optional[float]:
        baseline = aggregate_value(grouped, "baseline", metric, stat)
        optimized = aggregate_value(grouped, "optimized", metric, stat)
        if baseline is None or optimized is None:
            errors.append(f"missing {metric} {stat}")
            return None
        return percent_change(baseline, optimized)

    for metric in (
        "native_cursor_move",
        "native_indicator_move",
        "native_key_batch",
        "native_ax_first_partial",
        "native_ax_terminal",
    ):
        improvement = change(metric, "p95")
        if improvement is not None and improvement > -5.0:
            errors.append(
                f"{metric} p95 change was {improvement:.2f}% (requires at most -5%)"
            )

    vision_latency = change("native_vision_terminal", "p95")
    vision_footprint = change("native_metrics_vision_steady", "footprint_bytes")
    if (
        vision_latency is not None
        and vision_footprint is not None
        and min(vision_latency, vision_footprint) > -5.0
    ):
        errors.append(
            "Vision needs at least 5% lower terminal p95 or steady physical footprint"
        )

    for metric in (
        "native_backend_ready",
        "native_first_present",
        "native_cursor_move",
        "native_indicator_move",
        "native_key_batch",
        "native_ax_first_partial",
        "native_ax_terminal",
        "native_vision_first_partial",
        "native_vision_terminal",
        "native_hybrid_first_partial",
        "native_hybrid_terminal",
    ):
        regression = change(metric, "p99")
        if regression is not None and regression > 3.0:
            errors.append(f"{metric} p99 regressed {regression:.2f}% (limit 3%)")

    dismissed = change("native_metrics_dismissed", "footprint_bytes")
    if dismissed is not None and dismissed > 2.0:
        errors.append(f"dismissed physical footprint grew {dismissed:.2f}% (limit 2%)")

    steady_footprint = aggregate_value(
        grouped, "optimized", "native_metrics_hybrid_steady", "footprint_bytes"
    )
    after_footprint = aggregate_value(
        grouped, "optimized", "native_metrics_hybrid_after_50", "footprint_bytes"
    )
    if steady_footprint is None or after_footprint is None:
        errors.append("missing optimized Hybrid steady/after-50 footprint")
    elif percent_change(steady_footprint, after_footprint) > 2.0:
        errors.append("physical footprint kept growing after 50 Hybrid scans")

    steady_threads = aggregate_value(
        grouped, "optimized", "native_metrics_hybrid_steady", "threads"
    )
    after_threads = aggregate_value(
        grouped, "optimized", "native_metrics_hybrid_after_50", "threads"
    )
    if steady_threads is None or after_threads is None:
        errors.append("missing optimized Hybrid steady/after-50 thread count")
    elif after_threads > steady_threads:
        errors.append(
            f"Hybrid thread count did not return: {after_threads} > {steady_threads}"
        )
    return errors


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: summarize-macos-native.py OUTPUT_DIR ROUNDS", file=sys.stderr)
        return 2
    directory = Path(sys.argv[1])
    rounds = int(sys.argv[2])
    grouped: dict[str, dict[tuple[str, str], list[int]]] = {}
    validation_errors: list[str] = []
    for variant in ("baseline", "optimized"):
        aggregate: dict[tuple[str, str], list[int]] = {}
        for round_number in range(1, rounds + 1):
            for key, samples in collect(directory / f"{variant}-{round_number}.log").items():
                aggregate.setdefault(key, []).extend(samples)
        grouped[variant] = aggregate
    for round_number in range(1, rounds + 1):
        validation_errors.extend(
            validate_fixture_targets(
                directory / f"baseline-{round_number}.log",
                directory / f"optimized-{round_number}.log",
            )
        )
    validation_errors.extend(validate_performance(grouped))

    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(["metric", "stat", "baseline_median", "optimized_median", "change_percent"])
    keys = sorted(set(grouped["baseline"]) | set(grouped["optimized"]))
    for key in keys:
        baseline = grouped["baseline"].get(key, [])
        optimized = grouped["optimized"].get(key, [])
        if not baseline or not optimized:
            continue
        if key[1] == "elapsed":
            for stat, percent in (("p50", 50), ("p95", 95), ("p99", 99)):
                baseline_value = percentile(baseline, percent)
                optimized_value = percentile(optimized, percent)
                writer.writerow(
                    [
                        key[0],
                        stat,
                        baseline_value,
                        optimized_value,
                        f"{percent_change(baseline_value, optimized_value):.2f}",
                    ]
                )
            continue
        baseline_median = round(statistics.median(baseline))
        optimized_median = round(statistics.median(optimized))
        change = percent_change(baseline_median, optimized_median)
        writer.writerow([key[0], key[1], baseline_median, optimized_median, f"{change:.2f}"])
    if validation_errors:
        for error in validation_errors:
            print(f"fixture validation failed: {error}", file=sys.stderr)
        return 1
    for source in ("ax", "vision", "hybrid"):
        writer.writerow(
            [f"native_{source}_fixture_controls", "validated", 24, 24, "0.00"]
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
