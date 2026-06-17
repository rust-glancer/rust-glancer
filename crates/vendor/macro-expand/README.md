## rg_macro_expand

Rust Glancer declarative macro expansion crate.

The matcher, transcriber, and token-tree internals are adapted from
[`tt`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/tt) and
[`mbe`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/mbe) from
the `rust-analyzer` project, licensed under
[MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT) and
[Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE).

Unlike `rg_parser` and `rg_syntax`, this crate is not intended to preserve
upstream crate boundaries. Rust Glancer only exposes a small macro-expansion
facade; the adapted `tt`, `mbe`, span, and symbol support modules are private
implementation details.
