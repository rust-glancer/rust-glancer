//! Analysis tests run queries against small Rust fixtures and snapshot the answers. Add a test
//! to the module for the behavior being checked, and keep the fixture focused on that behavior.
//!
//! Start with `check_analysis`. Each `//- /path` starts a fixture file. Put `crate:name` on a
//! crate root to declare its package directly, then include the Rust source needed by the test:
//!
//! ```text
//! use expect_test::expect;
//! use crate::tests::utils::check_analysis;
//!
//! #[test]
//! fn infers_a_byte() {
//!     check_analysis(
//!         r#"
//! //- /src/lib.rs crate:analysis_byte
//! pub fn demo() {
//!     let byte = 1_u8$0; // ty("literal")
//!     let copy$0 = byte; // ty("binding")
//! }
//! "#,
//!         expect![[r#"
//!             u8 // literal
//!             u8 // binding
//!         "#]],
//!     );
//! }
//! ```
//!
//! The default edition is 2024 and the default target kind is `lib`. For several packages, use
//! `root:app` for a package directory and `deps:api` for its dependency. A root with
//! `kind:bin name:tool` declares a binary in that same package. See `rg_workspace::testonly`
//! for the header options. Rust files still live on disk; these fixtures do not run Cargo.
//!
//! Put a marker exactly where an editor's cursor would be. Numeric IDs are local to each line,
//! so `$0` can be reused on the next line or in another file. With several markers, pair queries
//! from left to right: `let x$0 = 1$1; // ty("binding"), ty("literal")`. Keep at most three markers
//! per line. `$11` works too; use a closing `$` before source digits, as in `pair.$0$1`.
//! Write `\$0` when the fixture needs a literal `$0`.
//!
//! Queries run in source order. Their titles explain what is being checked and label the
//! snapshot: short answers use `answer // title`, and multiline answers start with `// title`.
//! Other query names include `hover`, `goto`, `goto_type`, `goto_impl`, `complete`, `references`
//! and `rename`. For example, `rename("field", "new_name")` supplies the new name, while
//! `references("uses", ReferenceQuery::all())` supplies the search scope.
//!
//! Completion queries can use `.matching("prefix")` to keep only relevant candidates; use
//! `complete_verbose` when details or edits matter. The `complete_with_source` variants test
//! completion with captured document text. Use `.in_lib("package")` or `.in_bin("package")`
//! to select a target in a workspace fixture. `check_analysis_with_fake_sysroot` adds the shared
//! small standard-library fixture when the test needs it.
//!
//! Use `check_analysis_queries` with an explicit `AnalysisQuery` list when one marker needs
//! several queries, a line is too crowded, or an unfinished string would swallow the comment.
//! Named markers help here: `AnalysisQuery::hover("type", "cursor")` targets `$cursor$`.
//! Each marker name must identify one position in the fixture. For outlines, workspace symbol
//! searches and inlay hints, use `check_document_symbols`, `check_workspace_symbols` or
//! `check_inlay_hints` instead.
//!
//! Run a focused test with `just agent-debug test -p rg_analysis test_name`. To generate or
//! update snapshots, add `--env UPDATE_EXPECT=1` before `test` and select one test at a time;
//! updating one test can shift the source locations embedded in the other tests' binaries.
//! Inspect the changed expectations, then rerun without that environment override.

mod builtin_derives;
mod utils;

mod code_actions;
mod completions_at;
mod document_symbols;
mod goto_definition;
mod goto_implementation;
mod goto_type_definition;
mod hover;
mod inference;
mod inlay_hints;
mod macro_navigation;
mod raw_identifiers;
mod references;
mod rename;
mod resolve_symbol;
mod symbol_at;
mod type_at;
mod workspace_symbols;
