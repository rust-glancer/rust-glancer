# rg_tt

Shared token-tree primitives adapted from rust-analyzer `v0.3.3057`, tag
`2026-09-21`, commit `aaddfb73fd95f2c0bf001b474dca91ae28bcce3a`, under the
upstream MIT and Apache-2.0 licenses.

`tt/storage.rs` uses upstream's variable-length byte encoding with per-tree symbol
and span tables. Symbols remain locally owned, and spans retain rust-glancer's
file and edition semantics. Memory accounting includes the encoded buffer, span
table, and owned symbols. The syntax bridge uses the custom immutable syntax tree
and desugars doc comments into attributes before macro matching.

The in-memory decoder assumes encoder-produced bytes. Wincode therefore stores
logical tokens and validates nesting, literal suffix boundaries, ranges, and
editions before re-encoding on load. It does not deserialize an unchecked byte
buffer. Development package caches from older representations should be discarded
and rebuilt; the cache schema version intentionally stays unchanged.

For an existing editor workspace, run `Rust Glancer: Reindex Workspace` to rebuild
from source. For automated runs, stop the server and move aside that fixture's
`target/rust_glancer` directory, or use a fresh isolated cache as below. Keep Cargo
build outputs and source files intact.

Use `just agent-debug --isolated-cache ra-upgrade analyze <fixture>` for a fresh
local cache, and `just agent-debug test -p rg_tt -p rg_macro_expand` for focused
validation. Avoid copying upstream's interner or span database into this crate.
