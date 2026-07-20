#!/usr/bin/env python3
"""Reject compatibility reports where a supported LSP method went entirely silent."""

import argparse
import json
from pathlib import Path
from typing import Any


# The rust-analyzer fixture is pinned and contains positive cases for every method below. Scores
# remain informational, but an empty aggregate means the LSP stopped serving a feature altogether.
EXPECTED_NONEMPTY_METHODS = {
    "textDocument/references",
    "textDocument/definition",
    "textDocument/typeDefinition",
    "textDocument/implementation",
    "textDocument/prepareRename",
    "textDocument/rename",
    "textDocument/documentHighlight",
    "textDocument/documentSymbol",
    "workspace/symbol",
    "textDocument/inlayHint",
    "textDocument/hover",
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()

    with args.report.open(encoding="utf-8") as file:
        report = json.load(file)
    failures = liveness_failures(report)
    if failures:
        details = "\n".join(f"- {failure}" for failure in failures)
        raise SystemExit(f"LSP compatibility liveness check failed:\n{details}")


def liveness_failures(report: Any) -> list[str]:
    if not isinstance(report, dict):
        return ["report root is not an object"]

    aggregates = report.get("aggregates")
    if not isinstance(aggregates, list):
        return ["report does not contain an aggregate list"]

    aggregates_by_method = {
        aggregate.get("method"): aggregate
        for aggregate in aggregates
        if isinstance(aggregate, dict) and isinstance(aggregate.get("method"), str)
    }
    failures: list[str] = []
    for method in sorted(EXPECTED_NONEMPTY_METHODS):
        aggregate = aggregates_by_method.get(method)
        if aggregate is None:
            failures.append(f"{method} is missing from the report")
            continue

        result_count = aggregate.get("rust_glancer_count")
        if not isinstance(result_count, int) or isinstance(result_count, bool):
            failures.append(f"{method} has no integer rust_glancer_count")
        elif result_count <= 0:
            failures.append(f"{method} returned no rust-glancer results")

    return failures


if __name__ == "__main__":
    main()
