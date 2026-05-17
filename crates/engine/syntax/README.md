## rg_syntax

Rust Glancer fork of [`syntax`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/syntax) crate.
It is a vendored part, originally a part of the `rust-analyzer` project, licensed under [MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT) and [Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE) license.

## On codegeneration

Relevant codegen parts are not (yet) vendored.
The expectation, I guess, is that we'll be able to copy generated files from rust-analyzer
when needed; if `rust.ungram` will diverge for any reason or API shape will change --
then it might make sense to vendor codegen logic. For now it's not worth it.
