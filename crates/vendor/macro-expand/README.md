## rg_macro_expand

Rust Glancer declarative macro expansion crate.

The matcher, transcriber, and token-tree internals are adapted from
[`tt`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/tt) and
[`mbe`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/mbe) from
the `rust-analyzer` project, licensed under
[MIT](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-MIT) and
[Apache 2.0](https://github.com/rust-lang/rust-analyzer/blob/master/LICENSE-APACHE).

The MBE implementation follows rust-analyzer `v0.3.3057`, tag `2026-09-21`,
commit `aaddfb73fd95f2c0bf001b474dca91ae28bcce3a`.

This crate exposes a small expansion facade. Shared token trees, syntax bridging,
local spans, and owned symbols live in `rg_tt`. Keep these boundaries when merging
upstream changes; the expander does not use Salsa or upstream global interning.
The local facade also avoids transcribing partial bindings after a failed match.

Run `just agent-debug test -p rg_tt -p rg_macro_expand` for token round trips,
macro parsing/expansion, and source mapping. Project tests cover re-expansion from
an offloaded dependency's cached macro definition.
