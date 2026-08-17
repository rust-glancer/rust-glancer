mod client "editors/code"

test:
    cargo nextest run --workspace

lint:
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings

codegen:
    cargo run -p rg_codegen -- all

codegen-check:
    cargo run -p rg_codegen -- all --check

[positional-arguments]
agent-debug *args:
    exec python3 tools/agent-debug.py "$@"

analyze *args:
    cargo run --release -p rust-glancer -- analyze {{args}}

compare-lsp fixture="rust_analyzer" *args:
    cargo run --release -p rust-glancer -- compare-lsp {{fixture}} {{args}}

lsp-query query_file *args:
    python3 tools/lsp-query.py --query-file '{{query_file}}' {{args}}

lsp-query-help:
    python3 tools/lsp-query.py --help

deny:
    cargo deny check

build:
    cargo build --workspace --release

package-vsix:
    just client::install
    just client::build
    just client::package-vsix

bench:
    cargo bench -p rg_project --bench analysis_pipeline

check-test-targets:
    cargo check --manifest-path test_targets/simple_crate/Cargo.toml --locked
    cargo check --manifest-path test_targets/moderate_crate/Cargo.toml --locked
    cargo check --manifest-path test_targets/complex_crate/Cargo.toml --locked
    cargo check --manifest-path test_targets/moderate_workspace/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/complex_workspace/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/bench_fixtures/small_app/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/bench_fixtures/synthetic_parse_heavy/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/bench_fixtures/synthetic_item_tree_heavy/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/bench_fixtures/synthetic_def_map_heavy/Cargo.toml --workspace --locked
    cargo check --manifest-path test_targets/bench_fixtures/synthetic_body_heavy/Cargo.toml --workspace --locked

pr-ready: test lint deny codegen-check check-test-targets client::pr-ready
