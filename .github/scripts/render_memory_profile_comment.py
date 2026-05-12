#!/usr/bin/env python3
"""Render the internal analyze JSON report as a pull-request memory table."""

import argparse
import json
from pathlib import Path
from typing import Any, Optional, Union


def main() -> None:
    args = parse_args()
    current = read_json(args.current)
    base = read_optional_json(args.base)
    title = args.section_title
    if title is None and not args.body_only:
        title = "Rust Glancer Memory Profile"
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
        return json.load(file)


def read_optional_json(path: Optional[Path]) -> Optional[dict[str, Any]]:
    if path is None or not path.exists():
        return None
    return read_json(path)


def render_comment(
    current: dict[str, Any],
    base: Optional[dict[str, Any]],
    title: Optional[str] = "Rust Glancer Memory Profile",
) -> str:
    lines = []
    if title is not None:
        lines.extend([f"## {title}", ""])

    lines.extend(
        [
            render_context(current, base),
            "",
            "Values come from one GitHub runner run, so treat deltas as directional signal rather than a hard threshold.",
            "",
            render_metric_table(current, base),
            "",
            render_checkpoint_table(current, base),
            "",
            render_component_table(current, base),
            "",
        ]
    )
    return "\n".join(lines)


def render_context(current: dict[str, Any], base: Optional[dict[str, Any]]) -> str:
    packages = current.get("project", {}).get("packages", {})
    allocator = current.get("allocator", {})
    base_note = "available" if base is not None else "unavailable"

    return "\n".join(
        [
            f"- Fixture: `{workspace_name(current)}`",
            f"- Packages: {packages.get('total_count', '?')} total, {packages.get('workspace_count', '?')} workspace",
            f"- Residency: `{packages.get('residency_policy', '?')}`",
            f"- Allocator: `{allocator.get('name', '?')}`",
            f"- Base result: {base_note}",
        ]
    )


def render_metric_table(current: dict[str, Any], base: Optional[dict[str, Any]]) -> str:
    metrics = [
        (
            "Build elapsed",
            build_elapsed_ms,
            format_duration_ms,
            format_duration_delta_ms,
        ),
        (
            "Peak allocator resident",
            peak_allocator_resident_bytes,
            format_bytes,
            format_byte_delta,
        ),
        (
            "Final allocator resident",
            final_allocator_resident_bytes,
            format_bytes,
            format_byte_delta,
        ),
        (
            "Post-purge allocator resident",
            post_purge_allocator_resident_bytes,
            format_bytes,
            format_byte_delta,
        ),
        (
            "Retained project memory",
            retained_project_bytes,
            format_bytes,
            format_byte_delta,
        ),
    ]

    rows = ["| Metric | Current | Base | Delta |", "| --- | ---: | ---: | ---: |"]
    for label, getter, formatter, delta_formatter in metrics:
        current_value = getter(current)
        base_value = getter(base) if base is not None else None
        rows.append(
            "| {label} | {current} | {base} | {delta} |".format(
                label=label,
                current=format_optional(current_value, formatter),
                base=format_optional(base_value, formatter),
                delta=delta_formatter(current_value, base_value),
            )
        )
    return "\n".join(rows)


def render_checkpoint_table(current: dict[str, Any], base: Optional[dict[str, Any]]) -> str:
    current_rows = checkpoints_for(current)
    if not current_rows:
        return ""

    base_rows = {
        row.get("label"): row
        for row in checkpoints_for(base)
        if isinstance(row.get("label"), str)
    }

    rows = [
        "### Build Checkpoints",
        "",
        "| Checkpoint | Phase | Delta | RG sampled | Delta | RG total | Delta | Allocator resident | Delta |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in current_rows:
        label = row_label(row)
        base_row = base_rows.get(label)
        table_row = (
            "| {label} | {phase} | {phase_delta} | "
            "{rg_sampled} | {rg_sampled_delta} | "
            "{rg_total} | {rg_total_delta} | "
            "{resident} | {resident_delta} |"
        )
        rows.append(
            table_row.format(
                label=label,
                phase=format_optional(row_number(row, "phase_elapsed_ms"), format_duration_ms),
                phase_delta=format_duration_delta_ms(
                    row_number(row, "phase_elapsed_ms"),
                    row_number(base_row, "phase_elapsed_ms"),
                ),
                rg_sampled=format_optional(row_number(row, "retained_bytes"), format_bytes),
                rg_sampled_delta=format_byte_delta(
                    row_number(row, "retained_bytes"),
                    row_number(base_row, "retained_bytes"),
                ),
                rg_total=format_optional(
                    row_number(row, "active_retained_bytes"),
                    format_bytes,
                ),
                rg_total_delta=format_byte_delta(
                    row_number(row, "active_retained_bytes"),
                    row_number(base_row, "active_retained_bytes"),
                ),
                resident=format_optional(row_number(row, "resident_bytes"), format_bytes),
                resident_delta=format_byte_delta(
                    row_number(row, "resident_bytes"),
                    row_number(base_row, "resident_bytes"),
                ),
            )
        )
    return "\n".join(rows)


def render_component_table(current: dict[str, Any], base: Optional[dict[str, Any]]) -> str:
    current_rows = component_rows_for(current)
    base_rows = {
        row.get("label"): row.get("bytes")
        for row in component_rows_for(base)
    }

    rows = [
        "### Retained Memory By Component",
        "",
        "| Component | Current | Base | Delta |",
        "| --- | ---: | ---: | ---: |",
    ]
    for row in current_rows:
        label = row.get("label", "?")
        current_bytes = row.get("bytes")
        base_bytes = base_rows.get(label)
        rows.append(
            "| {label} | {current} | {base} | {delta} |".format(
                label=label,
                current=format_optional(current_bytes, format_bytes),
                base=format_optional(base_bytes, format_bytes),
                delta=format_byte_delta(current_bytes, base_bytes),
            )
        )
    return "\n".join(rows)


def component_rows_for(report: Optional[dict[str, Any]]) -> list[dict[str, Any]]:
    if report is None:
        return []
    memory = report.get("memory", {})
    if not isinstance(memory, dict):
        return []

    # Older cached CI results used the less precise `by_phase` name for this component grouping.
    rows = memory.get("by_component", memory.get("by_phase", []))
    return [row for row in rows if isinstance(row, dict)]


def workspace_name(report: dict[str, Any]) -> str:
    workspace_root = report.get("workspace_root")
    if not isinstance(workspace_root, str) or not workspace_root:
        return "unknown"
    return Path(workspace_root).name


def build_elapsed_ms(report: Optional[dict[str, Any]]) -> Optional[float]:
    checkpoints = checkpoints_for(report)
    if not checkpoints:
        return None
    return checkpoints[-1].get("elapsed_ms")


def peak_allocator_resident_bytes(report: Optional[dict[str, Any]]) -> Optional[int]:
    values = [
        checkpoint.get("resident_bytes")
        for checkpoint in checkpoints_for(report)
        if isinstance(checkpoint.get("resident_bytes"), int)
    ]
    return max(values) if values else None


def final_allocator_resident_bytes(report: Optional[dict[str, Any]]) -> Optional[int]:
    checkpoints = checkpoints_for(report)
    if not checkpoints:
        return nested_int(report, ["allocator", "stats", "resident_bytes"])
    return checkpoints[-1].get("resident_bytes")


def post_purge_allocator_resident_bytes(report: Optional[dict[str, Any]]) -> Optional[int]:
    return nested_int(report, ["allocator", "purge", "after", "resident_bytes"])


def retained_project_bytes(report: Optional[dict[str, Any]]) -> Optional[int]:
    return nested_int(report, ["memory", "retained_bytes"])


def checkpoints_for(report: Optional[dict[str, Any]]) -> list[dict[str, Any]]:
    if report is None:
        return []
    checkpoints = report.get("build_profile", {}).get("checkpoints", [])
    return [checkpoint for checkpoint in checkpoints if isinstance(checkpoint, dict)]


def row_label(row: dict[str, Any]) -> str:
    label = row.get("label")
    return label if isinstance(label, str) else "?"


def row_number(row: Optional[dict[str, Any]], key: str) -> Optional[float]:
    if row is None:
        return None
    value = row.get(key)
    if isinstance(value, bool):
        return None
    return value if isinstance(value, (int, float)) else None


def nested_int(report: Optional[dict[str, Any]], path: list[str]) -> Optional[int]:
    value: Any = report
    for key in path:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value if isinstance(value, int) else None


def format_optional(value: Optional[float], formatter: Any) -> str:
    if value is None:
        return "-"
    return formatter(value)


def format_bytes(value: Union[int, float]) -> str:
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    size = float(value)
    unit = units[0]
    for next_unit in units[1:]:
        if abs(size) < 1024.0:
            break
        size /= 1024.0
        unit = next_unit
    if unit == "B":
        return f"{int(value)} B"
    return f"{size:.1f} {unit}"


def format_duration_ms(value: Union[int, float]) -> str:
    if value < 1.0:
        return f"{value:.2f} ms"
    if value < 10.0:
        return f"{value:.1f} ms"
    if value < 1000.0:
        return f"{value:.0f} ms"
    return f"{value / 1000.0:.2f} s"


def format_byte_delta(current: Optional[float], base: Optional[float]) -> str:
    return format_delta(current, base, format_bytes)


def format_duration_delta_ms(current: Optional[float], base: Optional[float]) -> str:
    return format_delta(current, base, format_duration_ms)


def format_delta(current: Optional[float], base: Optional[float], formatter: Any) -> str:
    if current is None or base is None:
        return "-"

    delta = current - base
    sign = "+" if delta >= 0 else "-"
    percent = ""
    if base != 0:
        percent_value = (delta / base) * 100.0
        percent = f" ({percent_value:+.1f}%)"
    return f"{sign}{formatter(abs(delta))}{percent}"


if __name__ == "__main__":
    main()
