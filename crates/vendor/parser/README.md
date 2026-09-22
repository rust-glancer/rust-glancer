## rg_parser

Rust Glancer fork of [`parser`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/parser) crate.
It is a vendored part, originally a part of the `rust-analyzer` project, licensed under [MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT) and [Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE) license.

## Upstream and regeneration

Synced with rust-analyzer `v0.3.3057` (tag `2026-09-21`, commit
`aaddfb73fd95f2c0bf001b474dca91ae28bcce3a`). Related Cargo git dependencies,
lexer/literal-escaper versions, and benchmark inputs move with this snapshot.

Grammar and inline-test generation live in `crates/tools/codegen`. Run
`just codegen` after changing `crates/vendor/syntax/rust.ungram` or parser inline
test comments; `just codegen-check` verifies checked-in output. Review parser
`.rast` goldens separately, including removed or renamed upstream tests.
