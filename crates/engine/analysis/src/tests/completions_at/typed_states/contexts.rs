//! User-typing states owned by general request-local syntax contexts.

use expect_test::expect;

use super::super::super::utils::{AnalysisQuery, check_analysis_queries};

/// Empty paths commonly occur before the parser has seen any closing punctuation.
#[test]
fn completes_empty_paths_at_incomplete_file_end() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_empty_path_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod argument;
pub mod expression;
pub mod generic_argument;
pub mod import;
pub mod qualified_import;
pub mod qualified_type;
pub mod type_position;

pub struct VisibleType;
pub struct Wrapper<T>(T);

pub fn consume(_: u8) {}

//- /src/import.rs
use $empty_import_eof$
//- /src/qualified_import.rs
use crate::$qualified_import_eof$
//- /src/type_position.rs
pub struct SignatureVisibleType;

pub fn run(_: $empty_type_eof$
//- /src/qualified_type.rs
pub fn run(_: crate::$qualified_type_eof$
//- /src/expression.rs
pub fn run(local_value: u8) {
    let _ = $empty_expression_eof$
//- /src/argument.rs
pub fn run(local_value: u8) {
    crate::consume($empty_argument_eof$
//- /src/generic_argument.rs
pub struct GenericVisibleType;

pub fn run() {
    let _: crate::Wrapper<$empty_generic_eof$
"#,
        &[
            AnalysisQuery::complete_with_source("empty import at EOF", "empty_import_eof")
                .matching("crate"),
            AnalysisQuery::complete_with_source(
                "empty qualified import segment at EOF",
                "qualified_import_eof",
            )
            .matching("VisibleType"),
            AnalysisQuery::complete_with_source("empty signature type at EOF", "empty_type_eof")
                .matching("SignatureVisibleType"),
            AnalysisQuery::complete_with_source(
                "empty qualified signature segment at EOF",
                "qualified_type_eof",
            )
            .matching("VisibleType"),
            AnalysisQuery::complete_with_source(
                "empty body expression at EOF",
                "empty_expression_eof",
            )
            .matching("local_value"),
            AnalysisQuery::complete_with_source("empty call argument at EOF", "empty_argument_eof")
                .matching("local_value"),
            AnalysisQuery::complete_with_source(
                "empty generic argument at EOF",
                "empty_generic_eof",
            )
            .matching("GenericVisibleType"),
        ],
        expect![[r#"
            empty import at EOF
            - keyword crate

            empty qualified import segment at EOF
            - struct VisibleType

            empty signature type at EOF
            - struct SignatureVisibleType

            empty qualified signature segment at EOF
            - struct VisibleType

            empty body expression at EOF
            - variable local_value

            empty call argument at EOF
            - variable local_value

            empty generic argument at EOF
            - struct GenericVisibleType
        "#]],
    );
}

/// General syntax contexts must survive the same incomplete owner boundaries as semantic names.
#[test]
fn completes_keywords_and_patterns_while_the_construct_is_being_typed() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_context_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod expression;
pub mod impl_trait;
pub mod pattern;
pub mod statement;
pub mod type_keyword;

//- /src/type_keyword.rs
pub fn run(_: dy$type_keyword_eof$
//- /src/impl_trait.rs
pub fn run(_: im$impl_trait_eof$
//- /src/statement.rs
pub fn run() {
    le$statement_eof$
//- /src/expression.rs
pub fn run() {
    let _ = ma$expression_eof$
//- /src/pattern.rs
pub fn run(value: bool) {
    let re$pattern_eof$
"#,
        &[
            AnalysisQuery::complete_keywords_with_source("type keyword at EOF", "type_keyword_eof")
                .matching("dyn"),
            AnalysisQuery::complete_keywords_with_source(
                "impl Trait keyword at EOF",
                "impl_trait_eof",
            )
            .matching("impl"),
            AnalysisQuery::complete_keywords_with_source(
                "statement keyword at EOF",
                "statement_eof",
            )
            .matching("let"),
            AnalysisQuery::complete_keywords_with_source(
                "expression keyword at EOF",
                "expression_eof",
            )
            .matching("match"),
            AnalysisQuery::complete_keywords_with_source("pattern keyword at EOF", "pattern_eof")
                .matching("ref"),
        ],
        expect![[r#"
            type keyword at EOF
            - keyword dyn

            impl Trait keyword at EOF
            - keyword impl

            statement keyword at EOF
            - keyword let

            expression keyword at EOF
            - keyword match

            pattern keyword at EOF
            - keyword ref
        "#]],
    );
}

/// Every item-list owner has distinct keyword rules and parser recovery.
#[test]
fn completes_each_item_list_context_at_incomplete_file_end() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_item_list_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod extern_block;
pub mod inherent_impl;
pub mod inline_module;
pub mod source_file;
pub mod trait_impl;
pub mod trait_list;

//- /src/source_file.rs
f$source_file_eof$
//- /src/inline_module.rs
pub mod inner {
    f$module_eof$
//- /src/inherent_impl.rs
struct Model;

impl Model {
    f$inherent_impl_eof$
//- /src/trait_list.rs
trait Service {
    f$trait_eof$
//- /src/trait_impl.rs
trait Service {}
struct Model;

impl Service for Model {
    f$trait_impl_eof$
//- /src/extern_block.rs
extern "C" {
    f$extern_block_eof$
"#,
        &[
            keyword_query("source-file item at EOF", "source_file_eof"),
            keyword_query("module item at EOF", "module_eof"),
            keyword_query("inherent impl item at EOF", "inherent_impl_eof"),
            keyword_query("trait item at EOF", "trait_eof"),
            keyword_query("trait impl item at EOF", "trait_impl_eof"),
            keyword_query("extern block item at EOF", "extern_block_eof"),
        ],
        expect![[r#"
            source-file item at EOF
            - keyword fn

            module item at EOF
            - keyword fn

            inherent impl item at EOF
            - keyword fn

            trait item at EOF
            - keyword fn

            trait impl item at EOF
            - keyword fn

            extern block item at EOF
            - keyword fn
        "#]],
    );
}

/// Leading item qualifiers are often typed before any declaration punctuation exists.
#[test]
fn completes_item_keywords_after_qualifiers_at_incomplete_file_end() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_item_qualifier_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod after_async;
pub mod after_const;
pub mod after_extern;
pub mod after_pub;
pub mod after_unsafe;

//- /src/after_pub.rs
pub f$after_pub_eof$
//- /src/after_unsafe.rs
unsafe f$after_unsafe_eof$
//- /src/after_async.rs
async f$after_async_eof$
//- /src/after_extern.rs
extern c$after_extern_eof$
//- /src/after_const.rs
const f$after_const_eof$
"#,
        &[
            keyword_query("after pub at EOF", "after_pub_eof"),
            keyword_query("after unsafe at EOF", "after_unsafe_eof"),
            keyword_query("after async at EOF", "after_async_eof"),
            AnalysisQuery::complete_keywords_with_source("after extern at EOF", "after_extern_eof")
                .matching("crate"),
            keyword_query("after const at EOF", "after_const_eof"),
        ],
        expect![[r#"
            after pub at EOF
            - keyword fn

            after unsafe at EOF
            - keyword fn

            after async at EOF
            - keyword fn

            after extern at EOF
            - keyword crate

            after const at EOF
            - keyword fn
        "#]],
    );
}

/// Pattern completion has separate insertion policies for name, tuple, and record constructors.
#[test]
fn completes_pattern_constructor_families_across_typed_states() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_pattern_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod empty_qualified_eof;
pub mod eof;

pub enum Event {
    Start,
    Data(u8),
    Stop { code: u8 },
}

pub fn qualified_edit(event: Event) {
    let Event::Sta$qualified_name_edit$ = event;
    let Event::Dat$tuple_edit$(_) = event;
    let Event::Sto$record_edit$ { code: _ } = event;
}

pub fn unqualified_unfinished(event: Event) {
    let Sta$unqualified_unfinished$ = event
}

pub fn unqualified_edit(event: Event) {
    let Sta$unqualified_edit$ = event;
}

//- /src/eof.rs
pub fn run(event: crate::Event) {
    match event {
        crate::Event::Sta$qualified_name_eof$
//- /src/empty_qualified_eof.rs
pub fn run(event: crate::Event) {
    match event {
        crate::Event::$empty_qualified_eof$
"#,
        &[
            pattern_query(
                "qualified name pattern with suffix",
                "qualified_name_edit",
                "Start",
            ),
            pattern_query("tuple pattern with suffix", "tuple_edit", "Data"),
            pattern_query("record pattern with suffix", "record_edit", "Stop"),
            pattern_query(
                "unqualified expected pattern without semicolon",
                "unqualified_unfinished",
                "Start",
            ),
            pattern_query(
                "unqualified expected pattern with suffix",
                "unqualified_edit",
                "Start",
            ),
            pattern_query(
                "qualified name pattern at EOF",
                "qualified_name_eof",
                "Start",
            ),
            pattern_query(
                "empty qualified pattern segment at EOF",
                "empty_qualified_eof",
                "Start",
            ),
        ],
        expect![[r#"
            qualified name pattern with suffix
            - variant Start

            tuple pattern with suffix
            - variant Data

            record pattern with suffix
            - variant Stop

            unqualified expected pattern without semicolon
            - variant Start

            unqualified expected pattern with suffix
            - variant Start

            qualified name pattern at EOF
            - variant Start

            empty qualified pattern segment at EOF
            - variant Start
        "#]],
    );
}

fn keyword_query(title: &'static str, marker: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_keywords_with_source(title, marker).matching("fn")
}

fn pattern_query(title: &'static str, marker: &'static str, label: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_with_source(title, marker).matching(label)
}
