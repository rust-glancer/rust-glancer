## rg_codegen

Developer-maintenance tool for generated parser and syntax files.

This crate vendors the small parts of `rust-analyzer` codegen that Rust Glancer
needs: grammar codegen and parser inline test extraction. It is not part of
normal builds; generated files stay checked in, and this tool exists so changes
to `rust.ungram` or parser test comments can be checked and regenerated locally.

It is based on [`xtask` codegen](https://github.com/rust-lang/rust-analyzer/tree/master/xtask/src/codegen)
from the `rust-analyzer` project, licensed under [MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT)
and [Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE) license.

The vendored generator follows rust-analyzer `v0.3.3057`, tag `2026-09-21`,
commit `aaddfb73fd95f2c0bf001b474dca91ae28bcce3a`. It retains the local AST
API and pinned rustfmt configuration. Parser golden output is maintained by the
parser tests, separately from generated fixture sources and the test runner.

## Usage

```text
cargo run -p rg_codegen -- all
cargo run -p rg_codegen -- all --check
```
