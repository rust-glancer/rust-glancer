# Development guidelines

## Prerequisites

`just` and `cargo-nextest`.

## Development commands

The recommended way to work with the project is via the [project Justfile](../Justfile) and
the [extension Justfile](../editors/code/Justfile).

Extension `just` commands can be invoked via submodules, e.g. `just client build`.

## Comments philosophy

I don't believe in "code must be self-documenting"; if your code is at least somewhat
non-trivial, annotate it. The documentation should be concise; it's mostly a hint to the reader
on what they should anticipate, and should focus on the context rather than on actions.

## Testing philosophy

Snapshot tests are preferred when possible. Snapshot tests tend to both be more declarative,
allow testing more behavior, and survive refactoring much better than usual tests.

We use `expect-test` for snapshot tests in most cases.

## Benchmarking, profiling and measuring memory

This project has commands to measure memory usage, profile, and benchmark the project.

For profiling and measuring memory, check out:

```sh
cargo run --release -- analyze --help
```

Note that measuring memory affects profiling, so don't interpret the phase timings
as a source of truth when also measuring memory. Treat these as two different modes.

Benchmarks can be run with

```sh
just bench
```
