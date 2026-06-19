# Profiling rust-glancer

This project has pretty good infrastructure for profiling the indexing pipeline.
It comes in three flavors:
- Indexing checkpoints
- Memory profiling
- Stats gathering.

Profiling is done primarily with [rg_profile](../crates/lib/profile/) crate.
It provides a way to declare profiling descriptors of different kinds, e.g.
counters, gauges, named metrics families, and checkpoints.

Checkpoints can be seen as a series of measurements, where measurement is a row
with fixed set of columns.

Each profiling metric has a scope in form of `foo.bar.baz`, which enables reverse
filtering: `foo.bar` enables [`foo.bar`, `foo`], but not `foo.bar.baz`, which is
convenient for enabling profiling up to a certain level.

The best way to learn the syntax is to look for examples, primarily in the 
[rg_project](../crates/engine/project/) crate, and to see doc-comment on the
[`declare_metrics` macro](../crates/lib/profile/src/macros.rs).

## Memory

Memory profiling is explained in greater detail in [MEMORY.md](./MEMORY.md).

## Reporting

We support several options for profile report generation:

- text (default), e.g. `just analyze . --profile --memory`
  Prints the report data as text output.
- HTML, e.g. `just analyze . --profile --memory --format html`
  Generates a timestamped HTML report in `target/rust_glancer/report`
  Works best for full analysis, e.g. `just analyze . --profile all --memory --format html`
- JSON, e.g. `just analyze . --profile --memory --format json`

Important: profiling is not without overhead. If you collect more data (e.g. retained memory
for each phase, or unresolved macros), the run will become slower, but it is not indicative
of slower indexing. For the most accurate measurements, use `just analyze . --profile`, as
it mostly just records timings per phase, without doing any expensive measurements.

