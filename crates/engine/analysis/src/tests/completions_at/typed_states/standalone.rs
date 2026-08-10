//! User-typing states for standalone semantic completion sites.

use expect_test::expect;

use super::super::super::utils::{AnalysisQuery, check_analysis_queries};

/// Standalone sites are discovered directly from incomplete item syntax rather than Body IR.
///
/// Each family is exercised once while the item is still being typed at file end and once with
/// the surrounding item-list syntax already present. Trait-member stubs cover both a bare recovery
/// prefix and the ordinary `fn req` typing state; the latter replaces the whole incomplete
/// declaration prefix with the generated signature.
#[test]
fn completes_standalone_sites_while_items_are_being_typed() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_standalone_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod macro_edit;
pub mod macro_forward;
pub mod module_edit;
pub mod module_empty;
pub mod module_forward;
pub mod trait_closed;
pub mod trait_empty;
pub mod trait_eof;
pub mod trait_fn_closed;
pub mod trait_fn_eof;

//- /src/trait_closed.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    req$trait_closed$
}

//- /src/trait_eof.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    req$trait_eof$
//- /src/trait_fn_closed.rs
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn req$trait_fn_closed$
}

//- /src/trait_fn_eof.rs
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn req$trait_fn_eof$
//- /src/trait_empty.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    $trait_empty_eof$
//- /src/macro_edit.rs
macro_rules! local_item {
    () => { struct Generated; };
}

local_i$macro_edit$!();

//- /src/macro_forward.rs
macro_rules! local_item {
    () => { struct Generated; };
}

local_i$macro_forward$
//- /src/module_edit.rs
mod pars$module_edit$;

//- /src/module_edit/parser.rs
pub struct Parser;

//- /src/module_forward.rs
mod pars$module_forward$
//- /src/module_forward/parser.rs
pub struct Parser;

//- /src/module_empty.rs
mod $module_empty_eof$
//- /src/module_empty/parser.rs
pub struct Parser;
"#,
        &[
            AnalysisQuery::complete_with_source(
                "trait member before closing brace",
                "trait_closed",
            )
            .matching("required"),
            AnalysisQuery::complete_with_source("trait member at EOF", "trait_eof")
                .matching("required"),
            AnalysisQuery::complete_with_source(
                "function trait member before closing brace",
                "trait_fn_closed",
            )
            .matching("required"),
            AnalysisQuery::complete_with_source("function trait member at EOF", "trait_fn_eof")
                .matching("required"),
            AnalysisQuery::complete_with_source(
                "trait member before its first prefix at EOF",
                "trait_empty_eof",
            )
            .matching("required"),
            AnalysisQuery::complete_with_source(
                "module macro before invocation suffix",
                "macro_edit",
            )
            .matching("local_item"),
            AnalysisQuery::complete_with_source("module macro at EOF", "macro_forward")
                .matching("local_item"),
            AnalysisQuery::complete_with_source(
                "module declaration before semicolon",
                "module_edit",
            )
            .matching("parser"),
            AnalysisQuery::complete_with_source("module declaration at EOF", "module_forward")
                .matching("parser"),
            AnalysisQuery::complete_with_source(
                "module declaration before its first prefix at EOF",
                "module_empty_eof",
            )
            .matching("parser"),
        ],
        expect![[r#"
            trait member before closing brace
            - fn required

            trait member at EOF
            - fn required

            function trait member before closing brace
            - fn required

            function trait member at EOF
            - fn required

            trait member before its first prefix at EOF
            - fn required

            module macro before invocation suffix
            - macro local_item

            module macro at EOF
            - macro local_item

            module declaration before semicolon
            - module parser

            module declaration at EOF
            - module parser

            module declaration before its first prefix at EOF
            - module parser
        "#]],
    );
}
