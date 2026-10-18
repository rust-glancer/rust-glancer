//! LSP engine tests keep editor operations explicit: initialize a fixture, open or change a
//! document, check the answers, then shut the fixture down. Use `//- /path` to separate Cargo
//! fixture files, as in the analysis tests.
//!
//! Put simple cursor queries next to their markers:
//!
//! ```text
//! pub fn demo(user: User) {
//!     user.na$0me; // hover("field type")
//!     user.$0; // complete("members")
//! }
//! ```
//!
//! `fixture.check(expect)` runs saved-fixture annotations in source order. For editor text,
//! pass the annotated source directly to `did_change_full` or `did_open_dirty`, then use
//! `fixture.check_dirty(&dirty, expect)`. Annotations are removed before sending source to the
//! engine, so they do not change query ranges or edit results.
//!
//! Numeric markers are local to a line; reuse `$0` on the next line. Several markers pair with
//! comma-separated queries from left to right, with at most three per line. `$11` works too;
//! use `$0$` before source digits. Query names include `goto`, `goto_type`, `goto_impl`, `hover`,
//! `highlight`, `complete`, `references("title", true)` and `code_actions`. Code actions can
//! use `.only("quickfix")`; `automatic_code_actions` changes the trigger. The references
//! boolean controls whether the declaration is included.
//!
//! Keep `check_queries` / `check_dirty_queries` and named markers such as `$cursor$` for repeated
//! queries at one position, unfinished strings, crowded lines, and file or range queries.
//! Short responses use `answer // title`; multiline responses start with `// title` and retain
//! indentation for symbols, completion details, hover Markdown, and edits.
//!
//! Run these tests with `just agent-debug test -p rg_lsp_engine`.

mod code_action_flow;
mod dirty_flow;
mod document_read_flow;
mod documentation_flow;
mod external_flow;
mod flow;
mod folding_flow;
mod formatting_flow;
mod global_operation_flow;
mod hover_links;
mod macro_navigation;
mod rename_flow;
mod save_flow;
mod utils;
