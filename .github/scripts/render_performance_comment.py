#!/usr/bin/env python3
"""Combine CI performance report sections into one compact pull-request comment."""

import argparse
from pathlib import Path


def main() -> None:
    args = parse_args()
    markdown = "\n\n".join(
        [
            render_section(args.memory, "Memory usage analysis"),
            render_section(args.benchmark, "Benchmark results"),
            "",
        ]
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(markdown, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--memory", type=Path, required=True)
    parser.add_argument("--benchmark", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def render_section(path: Path, fallback_title: str) -> str:
    title, body = read_section(path, fallback_title)
    return "\n".join(
        [
            f"## {title}",
            "",
            "<details>",
            "<summary>Click to see</summary>",
            "",
            body,
            "",
            "</details>",
        ]
    )


def read_section(path: Path, fallback_title: str) -> tuple[str, str]:
    lines = path.read_text(encoding="utf-8").strip().splitlines()
    if lines and lines[0].startswith("## "):
        title = lines[0].removeprefix("## ").strip()
        body = "\n".join(lines[1:]).strip()
        return title, body
    return fallback_title, "\n".join(lines).strip()


if __name__ == "__main__":
    main()
