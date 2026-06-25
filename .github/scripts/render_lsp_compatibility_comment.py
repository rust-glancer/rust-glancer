#!/usr/bin/env python3
"""Render public-LSP compatibility JSON as a pull-request table."""

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


@dataclass
class ScoreRow:
    method: str
    match_score_percent: Optional[float]
    recall_percent: Optional[float]
    precision_percent: Optional[float]


@dataclass
class LspCompatibilityReport:
    fixture: str
    query_count: Optional[int]
    rows: list[ScoreRow]


def main() -> None:
    args = parse_args()
    current = normalize_report(read_json(args.current))
    base = normalize_optional_report(read_optional_json(args.base))
    title = args.section_title
    if title is None and not args.body_only:
        title = "LSP compatibility"
    markdown = render_comment(current, base, title)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(markdown, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--base", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--body-only", action="store_true")
    parser.add_argument("--section-title")
    return parser.parse_args()


def read_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as file:
        value = json.load(file)
    if not isinstance(value, dict):
        return {}
    return value


def read_optional_json(path: Optional[Path]) -> Optional[dict[str, Any]]:
    if path is None or not path.exists():
        return None
    return read_json(path)


def normalize_optional_report(
    report: Optional[dict[str, Any]],
) -> Optional[LspCompatibilityReport]:
    if report is None:
        return None
    return normalize_report(report)


def normalize_report(report: dict[str, Any]) -> LspCompatibilityReport:
    fixture = dict_value(report.get("fixture"))
    rows = aggregate_rows(report)
    total = ScoreRow(
        method="Total",
        match_score_percent=number_value(fixture.get("equivalence_score_percent")),
        recall_percent=average_optional(row.recall_percent for row in rows),
        precision_percent=average_optional(row.precision_percent for row in rows),
    )

    return LspCompatibilityReport(
        fixture=string_value(fixture.get("kind"), "unknown"),
        query_count=int_value(fixture.get("query_count")),
        rows=[total, *rows],
    )


def aggregate_rows(report: dict[str, Any]) -> list[ScoreRow]:
    aggregates = report.get("aggregates", [])
    if not isinstance(aggregates, list):
        return []

    rows = []
    for aggregate in aggregates:
        if not isinstance(aggregate, dict):
            continue
        rows.append(
            ScoreRow(
                method=string_value(aggregate.get("method"), "unknown"),
                match_score_percent=number_value(aggregate.get("match_score_percent")),
                recall_percent=number_value(aggregate.get("recall_percent")),
                precision_percent=number_value(aggregate.get("precision_percent")),
            )
        )
    return rows


def render_comment(
    current: LspCompatibilityReport,
    base: Optional[LspCompatibilityReport],
    title: Optional[str] = "LSP compatibility",
) -> str:
    lines = []
    if title is not None:
        lines.extend([f"## {title}", ""])

    lines.extend(
        [
            render_context(current, base),
            "",
            "Values compare rust-glancer public-LSP responses against rust-analyzer. Deltas are percentage points.",
            "",
            render_score_table(current, base),
            "",
        ]
    )
    return "\n".join(lines)


def render_context(
    current: LspCompatibilityReport,
    base: Optional[LspCompatibilityReport],
) -> str:
    base_note = "available" if base is not None else "unavailable"
    query_count = current.query_count if current.query_count is not None else "?"
    return "\n".join(
        [
            f"- Fixture: `{current.fixture}`",
            f"- Queries: {query_count}",
            f"- Base result: {base_note}",
        ]
    )


def render_score_table(
    current: LspCompatibilityReport,
    base: Optional[LspCompatibilityReport],
) -> str:
    current_rows = {row.method: row for row in current.rows}
    base_rows = {row.method: row for row in base.rows} if base is not None else {}
    methods = list(current_rows)
    for method in base_rows:
        if method not in current_rows:
            methods.append(method)

    rows = [
        "| Method | Match score | Delta | Recall | Delta | Precision | Delta |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for method in methods:
        current_row = current_rows.get(method)
        base_row = base_rows.get(method)
        rows.append(
            "| {method} | {match_score} | {match_delta} | "
            "{recall} | {recall_delta} | "
            "{precision} | {precision_delta} |".format(
                method=method,
                match_score=format_percent_change(
                    row_metric(base_row, "match_score_percent"),
                    row_metric(current_row, "match_score_percent"),
                ),
                match_delta=format_percent_delta(
                    row_metric(current_row, "match_score_percent"),
                    row_metric(base_row, "match_score_percent"),
                ),
                recall=format_percent_change(
                    row_metric(base_row, "recall_percent"),
                    row_metric(current_row, "recall_percent"),
                ),
                recall_delta=format_percent_delta(
                    row_metric(current_row, "recall_percent"),
                    row_metric(base_row, "recall_percent"),
                ),
                precision=format_percent_change(
                    row_metric(base_row, "precision_percent"),
                    row_metric(current_row, "precision_percent"),
                ),
                precision_delta=format_percent_delta(
                    row_metric(current_row, "precision_percent"),
                    row_metric(base_row, "precision_percent"),
                ),
            )
        )
    return "\n".join(rows)


def row_metric(row: Optional[ScoreRow], field: str) -> Optional[float]:
    if row is None:
        return None
    return getattr(row, field)


def average_optional(values: Any) -> Optional[float]:
    numbers = [value for value in values if isinstance(value, (int, float))]
    if not numbers:
        return None
    return sum(numbers) / len(numbers)


def format_optional_percent(value: Optional[float]) -> str:
    if value is None:
        return "-"
    return f"{value:.1f}%"


def format_percent_change(base: Optional[float], current: Optional[float]) -> str:
    return f"{format_optional_percent(base)} -> {format_optional_percent(current)}"


def format_percent_delta(current: Optional[float], base: Optional[float]) -> str:
    if current is None or base is None:
        return "-"
    delta = current - base
    sign = "+" if delta >= 0 else "-"
    return f"{sign}{abs(delta):.1f}pp"


def dict_value(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    return {}


def string_value(value: Any, default: str = "") -> str:
    if isinstance(value, str):
        return value
    return default


def int_value(value: Any) -> Optional[int]:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    return None


def number_value(value: Any) -> Optional[float]:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


if __name__ == "__main__":
    main()
