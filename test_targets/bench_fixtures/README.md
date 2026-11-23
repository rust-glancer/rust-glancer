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

## LSP benchmarks

From the repository root, run:

```sh
rustup toolchain install 1.100.0 --profile minimal --component rust-src --component rust-analyzer
just compare-lsp
just compare-lsp --mode full --rust-analyzer "$(rustup which --toolchain 1.100.0 rust-analyzer)"
```

Fast mode runs one paired synthetic edit on ripgrep. Full mode runs five pairs per workload on
ripgrep, Helix, rust-analyzer, Bevy, LostInTime, and DataFusion. It includes synthetic edits,
user simulations, first editor hints, isolated requests, and restarts. Bevy uses a
checked-in lockfile. Full mode also runs the standalone library edit and generated-source startup.
Use `--mode full --pairs 1` to cover every workload with one pair. Use `--workload` and `--target`
to focus a run.

Each pair runs both servers back to back. The first server alternates within each workload,
and the starting order rotates across workloads. The report shows medians, sample counts, and
individual observations.

```sh
just compare-lsp --baseline /path/to/old/rust-glancer --candidate /path/to/new/rust-glancer --pairs 6
just compare-lsp --mode full --workload user-simulation
just compare-lsp --mode full --workload restart --target bevy
just compare-lsp --workload isolated-requests --target helix --target datafusion --pairs 3
just compare-lsp --from-run /path/to/observations.json
```

Each run writes `report.html` and `observations.json` under `target/compare-lsp` by default.
`--output-dir` changes that parent directory. The JSON includes the versions, project
revisions, configuration, and individual request attempts. The same JSON can regenerate
the HTML with `--from-run`. Benchmark builds capture the checkout's Git revision, dirty state,
and branch when it differs from `main`. For a prebuilt binary, the report reads its `--version`.
To stamp a standalone build, run `sh tools/with-build-source.sh cargo build --release -p rust-glancer`
from its checkout root.

The first use fetches the pinned checkouts and Cargo dependencies.
The managed template cache keeps checkouts by project and revision. To use a custom
template directory, put each checkout under its project name at the pinned revision
and pass `--templates`. The [target definitions](lsp/targets.json) contain the pins.

Synthetic edits overlap editor hints with navigation, completion, and a type change.
Each user simulation navigates existing code, adds a module to a real crate, and saves an API
with a function, struct, trait implementation, declarative macro, and inline module. Out-of-line
tests call the project’s own API. The replay types at about 350 characters per minute with per-pair
jitter, completes a qualified import and a field, and queries the new API across files. It also
finds references to a local binding in the test. The standalone library edit requests an
auto-import for the same type. The replay records completion outcomes and applies the ordinary
text edit when a completion is unavailable.

Synthetic edits, user simulations, and isolated requests use fresh processes, checkouts, and
Cargo output directories. User simulation sends a hover immediately after opening a file and
retries until the expected result arrives. It waits for the ready signal, which includes
rust-analyzer’s cache priming, before requesting hints and navigating to start the edit.
Synthetic edits and isolated requests also start their measured actions after readiness.
Isolated requests run hover and hints twice each, one at a time on an unchanged file.
Restart seeds its cache in a separate, unedited checkout before reopening that checkout in
a new process. Cargo downloads are shared, and the OS filesystem cache is left alone.
Generated-source runs prepare build outputs with Cargo before launch. During measured
startup, rust-analyzer runs a warm Cargo check to collect build-script messages.
The regular workloads disable diagnostics, and rust-analyzer runs them with build scripts
and procedural macros disabled. The benchmark supplies an empty user-config directory
to the pinned rust-analyzer. The client prefers Markdown for hover. The JSON records the
client capabilities.

Action wait runs from the editor trigger to a usable result, including retries. Final RTT runs
from dispatch to receipt of the last attempt and includes server queueing. First result measures
launch to the first usable hover in startup and user simulation workloads. Server ready measures
launch to the server's ready notification, which may arrive after the first hover. Run time adds
up waits during the measured workflow, counting overlapping waits once. For User simulation,
it covers the editing actions that follow readiness. Startup-only runs end at the first result,
so their run time is zero.

In the HTML, sort and filter Actions and Raw data without splitting server pairs. Sorting uses
the first server's value. Actions compares timings across pairs with a usable result from both
servers. Skipped actions after an earlier failure are marked blocked and stay in the planned
action count. Raw data shows each trial's individual observations and expandable retry
timings. The JSON contains the request parameters and full responses.

The binary comparison uses paired changes. Its timing assessment needs at least six complete
pairs for a 95% sign-test interval. The interval must clear both 5 ms and 5% of the baseline
median to classify a change.

## LSP compatibility sweeps

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
