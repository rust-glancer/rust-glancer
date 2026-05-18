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

analyze *args:
    cargo run --release -p rust-glancer -- analyze {{args}}

deny:
    cargo deny check

build:
    cargo build --workspace --release

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
