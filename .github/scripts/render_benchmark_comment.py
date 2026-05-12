#!/usr/bin/env python3
"""Render Gungraun's JSON benchmark summaries as a pull-request table."""

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


METRICS = [
    ("Instructions", {"ir", "instructions"}),
    ("Estimated cycles", {"estimatedcycles", "estimated cycles", "estcycles"}),
    ("Total read+write", {"totalrw", "total rw", "total read+write"}),
]


@dataclass
class MetricResult:
    benchmark: str
    metric: str
    current: Optional[float]
    base: Optional[float]


def main() -> None:
    args = parse_args()
    summaries = read_json_objects(args.input)
    results = BenchmarkSummary.collect(summaries)
    title = args.section_title
    if title is None and not args.body_only:
        title = "Rust Glancer Benchmark"
    markdown = BenchmarkComment(results).render(title)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(markdown, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--body-only", action="store_true")
    parser.add_argument("--section-title")
    return parser.parse_args()


def read_json_objects(path: Path) -> list[dict[str, Any]]:
    objects = []
    with path.open(encoding="utf-8") as file:
        for line in file:
            line = line.strip()
            if not line.startswith("{"):
                continue
            value = json.loads(line)
            if isinstance(value, dict):
                objects.append(value)
    return objects


class BenchmarkSummary:
    def __init__(self, summary: dict[str, Any]) -> None:
        self.summary = summary

    @classmethod
    def collect(cls, summaries: list[dict[str, Any]]) -> list[MetricResult]:
        results = []
        for summary in summaries:
            results.extend(cls(summary).metric_results())
        return results

    def metric_results(self) -> list[MetricResult]:
        benchmark = self.benchmark_name()
        metrics = self.callgrind_metrics()
        results = []
        for label, aliases in METRICS:
            current, base = self.find_metric(metrics, aliases)
            if current is not None or base is not None:
                results.append(MetricResult(benchmark, label, current, base))
        return results

    def benchmark_name(self) -> str:
        function_name = self.summary.get("function_name")
        benchmark_id = self.summary.get("id")
        if isinstance(function_name, str) and isinstance(benchmark_id, str):
            return f"{function_name}/{benchmark_id}"
        if isinstance(benchmark_id, str):
            return benchmark_id
        if isinstance(function_name, str):
            return function_name
        return "ci_analyze"

    def callgrind_metrics(self) -> list[dict[str, Any]]:
        profiles = self.summary.get("profiles", [])
        if not isinstance(profiles, list):
            return []

        # Gungraun summaries are nested by Valgrind tool and benchmark part. The
        # total Callgrind summary is the only part we need for the compact CI
        # signal; the recursive fallback keeps the renderer tolerant of small
        # schema reshuffles.
        for profile in profiles:
            if not isinstance(profile, dict) or not self.is_callgrind_profile(profile):
                continue
            metrics = self.metrics_from_profile(profile)
            if metrics:
                return metrics
        return []

    def is_callgrind_profile(self, profile: dict[str, Any]) -> bool:
        return "callgrind" in json.dumps(profile.get("tool", "")).lower()

    def metrics_from_profile(self, profile: dict[str, Any]) -> list[dict[str, Any]]:
        summaries = profile.get("summaries", {})
        if isinstance(summaries, dict):
            total = summaries.get("total", {})
            if isinstance(total, dict):
                metrics = self.metric_entries(total.get("summary"))
                if metrics:
                    return metrics
        return self.metric_entries(profile)

    def metric_entries(self, value: Any) -> list[dict[str, Any]]:
        entries = []
        if isinstance(value, dict):
            for key, child in value.items():
                current, base = self.metric_values(child)
                if current is not None or base is not None:
                    entries.append({"name": key, "current": current, "base": base})
                entries.extend(self.metric_entries(child))
        elif isinstance(value, list):
            for child in value:
                entries.extend(self.metric_entries(child))
        return entries

    def metric_values(self, value: Any) -> tuple[Optional[float], Optional[float]]:
        if not isinstance(value, dict):
            return None, None
        metrics = value.get("metrics")
        if isinstance(metrics, dict):
            return self.either_or_both(metrics)
        current = self.metric_number(self.first_present(value, "current", "new"))
        base = self.metric_number(self.first_present(value, "base", "old"))
        return current, base

    def first_present(self, value: dict[str, Any], primary: str, fallback: str) -> Any:
        if primary in value:
            return value[primary]
        return value.get(fallback)

    def either_or_both(self, value: dict[str, Any]) -> tuple[Optional[float], Optional[float]]:
        if "Both" in value and isinstance(value["Both"], list):
            values = value["Both"]
            current = self.metric_number(values[0]) if len(values) > 0 else None
            base = self.metric_number(values[1]) if len(values) > 1 else None
            return current, base
        if "Left" in value:
            return self.metric_number(value["Left"]), None
        if "Right" in value:
            return None, self.metric_number(value["Right"])
        return None, None

    def metric_number(self, value: Any) -> Optional[float]:
        if isinstance(value, bool):
            return None
        if isinstance(value, (int, float)):
            return value
        if not isinstance(value, dict):
            return None
        for key in ("Int", "Float"):
            number = value.get(key)
            if isinstance(number, bool):
                return None
            if isinstance(number, (int, float)):
                return number
        return None

    def find_metric(
        self,
        metrics: list[dict[str, Any]],
        aliases: set[str],
    ) -> tuple[Optional[float], Optional[float]]:
        normalized_aliases = {self.normalized_metric_name(alias) for alias in aliases}
        for metric in metrics:
            name = self.normalized_metric_name(metric.get("name"))
            if name in normalized_aliases:
                return metric["current"], metric["base"]
        return None, None

    def normalized_metric_name(self, name: Any) -> str:
        if not isinstance(name, str):
            return ""
        return (
            name.lower()
            .replace("_", "")
            .replace("-", "")
            .replace(" ", "")
            .replace("+", "")
        )


class BenchmarkComment:
    def __init__(self, results: list[MetricResult]) -> None:
        self.results = results

    def render(self, title: Optional[str] = "Rust Glancer Benchmark") -> str:
        lines = []
        if title is not None:
            lines.extend([f"## {title}", ""])

        lines.extend(
            [
                "- Fixture: `test_targets/moderate_workspace`",
                "- Tool: `Gungraun / Callgrind`",
                f"- Base result: {self.base_note()}",
                "",
                "Values are Callgrind instruction-style measurements from one CI runner run. Treat deltas as directional signal rather than a hard threshold.",
                "",
                self.render_table(),
                "",
            ]
        )
        return "\n".join(lines)

    def base_note(self) -> str:
        return "available" if any(result.base is not None for result in self.results) else "unavailable"

    def render_table(self) -> str:
        if not self.results:
            return "_No Gungraun summary metrics were found in the benchmark output._"

        rows = [
            "| Benchmark | Metric | Base | Current | Delta |",
            "| --- | --- | ---: | ---: | ---: |",
        ]
        for result in self.results:
            rows.append(
                "| {benchmark} | {metric} | {base} | {current} | {delta} |".format(
                    benchmark=f"`{result.benchmark}`",
                    metric=result.metric,
                    base=format_optional_count(result.base),
                    current=format_optional_count(result.current),
                    delta=format_delta(result.current, result.base),
                )
            )
        return "\n".join(rows)


def format_optional_count(value: Optional[float]) -> str:
    if value is None:
        return "-"
    return format_count(value)


def format_count(value: float) -> str:
    abs_value = abs(value)
    if abs_value >= 1_000_000_000:
        return f"{value / 1_000_000_000:.2f}B"
    if abs_value >= 1_000_000:
        return f"{value / 1_000_000:.2f}M"
    if abs_value >= 1_000:
        return f"{value / 1_000:.2f}K"
    return f"{value:.0f}"


def format_delta(current: Optional[float], base: Optional[float]) -> str:
    if current is None or base is None:
        return "-"

    delta = current - base
    sign = "+" if delta >= 0 else "-"
    percent = ""
    if base != 0:
        percent = f" ({(delta / base) * 100.0:+.1f}%)"
    return f"{sign}{format_count(abs(delta))}{percent}"


if __name__ == "__main__":
    main()
