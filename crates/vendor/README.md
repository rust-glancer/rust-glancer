# Vendored crates

This folder contains crates vendored from `rust-analyzer` (licensed under MIT/Apache 2.0 licenses).

The vendored crates are changed to serve needs of this project, though, where possible,
we try to preserve the shape, so merging changes from upstream is still at least somewhat
possible.

Rough list of changes and rationale for vendoring:
- `syntax`: removed dependency on `rowan`, instead uses frozen trees for CST
- `macro-expand`: not fully vendored, but heavily based on the `mbe` crate, depends on `syntax`.
- `tt`: because it depends on `syntax` and we need it for `macro-expand`
- `parser`: no real reason, mostly "just in case" so far.
