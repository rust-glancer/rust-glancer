# Benchmark fixtures

This folder is for local benchmark inputs.
Some of the inputs may be too heavy to commit: for them we might have scripts to fetch them.
The helper scripts are tracked, but fetched projects are ignored by git.

## synthetic_body_heavy

The `synthetic_body_heavy/` workspace is generated Rust code designed to stress body
IR resolution. It has many small workflows, method calls, trait methods, and local
bindings, so it is useful as a repeatable worst-case body-heavy target.

## small_app

The `small_app/` workspace is a small quasi-real application fixture. It uses pinned
dependencies such as Tokio, Serde, Clap, Tracing, and Axum, with a checked-in lockfile.
It is meant to represent a modest early-stage service/CLI application rather than a
generated stress test.

## synthetic_parse_heavy

The `synthetic_parse_heavy/` workspace is generated Rust code shaped to stress parser
throughput: large source files, dense expressions, declarative macros, type aliases,
and attributes. It intentionally avoids many crate/module edges so parse cost dominates.

## synthetic_item_tree_heavy

The `synthetic_item_tree_heavy/` workspace is generated Rust code shaped to stress item
tree lowering: many structs, enums, traits, impls, aliases, consts, and statics spread
across modules. Function bodies stay small so body IR does not dominate this target.

## synthetic_def_map_heavy

The `synthetic_def_map_heavy/` workspace is generated Rust code shaped to stress module
and import resolution: multiple crates, nested modules, glob imports, public re-exports,
aliases, and cross-crate use sites.

## rust-analyzer

The `rust-analyzer/` checkout is used as a large, mature real-world benchmark target.
Its source is pinned in `fetch-rust-analyzer.sh` to release `v0.3.3057`,
tag `2026-09-21`, revision
`aaddfb73fd95f2c0bf001b474dca91ae28bcce3a`. This release requires Rust 1.98 or newer.
Change the Cargo git pins, vendored code, and LSP query corpus together when updating it.

Prepare it with:

```sh
./test_targets/bench_fixtures/fetch-rust-analyzer.sh
```

The script checks out the pinned revision and runs `cargo fetch --locked --quiet`.
The benchmark also runs that fetch once per target before measuring, because analysis
needs dependency sources to already exist in Cargo's local registry checkout.

To run only the checked-in synthetic target:

```sh
RUST_GLANCER_BENCH_TARGETS=synthetic_body_heavy cargo bench -p rg_project --bench analysis_pipeline
```

To run one of the phase-focused synthetic targets:

```sh
RUST_GLANCER_BENCH_TARGETS=synthetic_parse_heavy cargo bench -p rg_project --bench analysis_pipeline
RUST_GLANCER_BENCH_TARGETS=synthetic_item_tree_heavy cargo bench -p rg_project --bench analysis_pipeline
RUST_GLANCER_BENCH_TARGETS=synthetic_def_map_heavy cargo bench -p rg_project --bench analysis_pipeline
```

To run only the small app:

```sh
RUST_GLANCER_BENCH_TARGETS=small_app cargo bench -p rg_project --bench analysis_pipeline
```

To run only rust-analyzer:

```sh
RUST_GLANCER_BENCH_TARGETS=rust_analyzer cargo bench -p rg_project --bench analysis_pipeline
```

To run all configured targets:

```sh
cargo bench -p rg_project --bench analysis_pipeline
```

## LSP comparisons

Comparisons use the rust-analyzer available on PATH. Install rust-src before
either server indexes the fixture so both can resolve standard-library types.
To use the Rust toolchain's rust-analyzer:

```sh
rustup component add rust-analyzer rust-src
just agent-debug --isolated-cache compare-clean \
  --rust-analyzer "$(rustup which rust-analyzer)" \
  compare-lsp rust_analyzer --format json
just agent-debug --isolated-cache compare-dirty \
  --rust-analyzer "$(rustup which rust-analyzer)" \
  compare-lsp rust_analyzer_dirty --format json
```

On Windows, use `RUST_GLANCER_COMPARE_LSP_RUST_ANALYZER` with
`rust-glancer compare-lsp`; the managed agent-debug runner is for macOS/Linux.
Linux and Windows CI install rust-analyzer and rust-src through rustup.

CI compares results with the latest available main-branch baseline. A fixture
upgrade can affect that PR's deltas; after merging, the next successful main run
publishes the updated baseline for subsequent PRs.

When updating this fixture, audit query tokens and whole-file/workspace cases in
`crates/rust-glancer/src/compare_lsp/query/mod.rs`. Numeric positions can remain
valid while targeting the wrong symbol. Preserve coverage; comparison scores
still include real gaps in supported behavior.
