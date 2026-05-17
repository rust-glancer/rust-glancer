## rg_parser

Rust Glancer fork of [`parser`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/parser) crate.
It is a vendored part, originally a part of the `rust-analyzer` project, licensed under [MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT) and [Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE) license.

## On codegeneration

Relevant codegen parts are not (yet) vendored.
The expectation, I guess, is that we'll be able to copy generated files from rust-analyzer
when needed; right now no changes to `rg-parser` are planned. For now it's not worth it.
