//! User-typing states owned by general request-local syntax contexts.

use expect_test::expect;

use crate::tests::utils::check_analysis;

/// Empty paths commonly occur before the parser has seen any closing punctuation.
#[test]
fn completes_empty_paths_at_incomplete_file_end() {
    check_analysis(
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
use $0 // complete_with_source("empty import at EOF").matching("crate")
//- /src/qualified_import.rs
use crate::$0 // complete_with_source("empty qualified import segment at EOF").matching("VisibleType")
//- /src/type_position.rs
pub struct SignatureVisibleType;

pub fn run(_: $0 // complete_with_source("empty signature type at EOF").matching("SignatureVisibleType")
//- /src/qualified_type.rs
pub fn run(_: crate::$0 // complete_with_source("empty qualified signature segment at EOF").matching("VisibleType")
//- /src/expression.rs
pub fn run(local_value: u8) {
    let _ = $0 // complete_with_source("empty body expression at EOF").matching("local_value")
//- /src/argument.rs
pub fn run(local_value: u8) {
    crate::consume($0 // complete_with_source("empty call argument at EOF").matching("local_value")
//- /src/generic_argument.rs
pub struct GenericVisibleType;

pub fn run() {
    let _: crate::Wrapper<$0 // complete_with_source("empty generic argument at EOF").matching("GenericVisibleType")
"#,
        expect![[r#"
            keyword crate // empty import at EOF
            struct VisibleType // empty qualified import segment at EOF
            struct SignatureVisibleType // empty signature type at EOF
            struct VisibleType // empty qualified signature segment at EOF
            variable local_value // empty body expression at EOF
            variable local_value // empty call argument at EOF
            struct GenericVisibleType // empty generic argument at EOF
        "#]],
    );
}

/// General syntax contexts must survive the same incomplete owner boundaries as semantic names.
#[test]
fn completes_keywords_and_patterns_while_the_construct_is_being_typed() {
    check_analysis(
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
pub fn run(_: dy$0 // complete_keywords_with_source("type keyword at EOF").matching("dyn")
//- /src/impl_trait.rs
pub fn run(_: im$0 // complete_keywords_with_source("impl Trait keyword at EOF").matching("impl")
//- /src/statement.rs
pub fn run() {
    le$0 // complete_keywords_with_source("statement keyword at EOF").matching("let")
//- /src/expression.rs
pub fn run() {
    let _ = ma$0 // complete_keywords_with_source("expression keyword at EOF").matching("match")
//- /src/pattern.rs
pub fn run(value: bool) {
    let re$0 // complete_keywords_with_source("pattern keyword at EOF").matching("ref")
"#,
        expect![[r#"
            keyword dyn // type keyword at EOF
            keyword impl // impl Trait keyword at EOF
            keyword let // statement keyword at EOF
            keyword match // expression keyword at EOF
            keyword ref // pattern keyword at EOF
        "#]],
    );
}

/// Every item-list owner has distinct keyword rules and parser recovery.
#[test]
fn completes_each_item_list_context_at_incomplete_file_end() {
    check_analysis(
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
f$0 // complete_keywords_with_source("source-file item at EOF").matching("fn")
//- /src/inline_module.rs
pub mod inner {
    f$0 // complete_keywords_with_source("module item at EOF").matching("fn")
//- /src/inherent_impl.rs
struct Model;

impl Model {
    f$0 // complete_keywords_with_source("inherent impl item at EOF").matching("fn")
//- /src/trait_list.rs
trait Service {
    f$0 // complete_keywords_with_source("trait item at EOF").matching("fn")
//- /src/trait_impl.rs
trait Service {}
struct Model;

impl Service for Model {
    f$0 // complete_keywords_with_source("trait impl item at EOF").matching("fn")
//- /src/extern_block.rs
extern "C" {
    f$0 // complete_keywords_with_source("extern block item at EOF").matching("fn")
"#,
        expect![[r#"
            keyword fn // source-file item at EOF
            keyword fn // module item at EOF
            keyword fn // inherent impl item at EOF
            keyword fn // trait item at EOF
            keyword fn // trait impl item at EOF
            keyword fn // extern block item at EOF
        "#]],
    );
}

/// Leading item qualifiers are often typed before any declaration punctuation exists.
#[test]
fn completes_item_keywords_after_qualifiers_at_incomplete_file_end() {
    check_analysis(
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
pub f$0 // complete_keywords_with_source("after pub at EOF").matching("fn")
//- /src/after_unsafe.rs
unsafe f$0 // complete_keywords_with_source("after unsafe at EOF").matching("fn")
//- /src/after_async.rs
async f$0 // complete_keywords_with_source("after async at EOF").matching("fn")
//- /src/after_extern.rs
extern c$0 // complete_keywords_with_source("after extern at EOF").matching("crate")
//- /src/after_const.rs
const f$0 // complete_keywords_with_source("after const at EOF").matching("fn")
"#,
        expect![[r#"
            keyword fn // after pub at EOF
            keyword fn // after unsafe at EOF
            keyword fn // after async at EOF
            keyword crate // after extern at EOF
            keyword fn // after const at EOF
        "#]],
    );
}

/// Pattern completion has separate insertion policies for name, tuple, and record constructors.
#[test]
fn completes_pattern_constructor_families_across_typed_states() {
    check_analysis(
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
    let Event::Sta$0 = event; // complete_with_source("qualified name pattern with suffix").matching("Start")
    let Event::Dat$0(_) = event; // complete_with_source("tuple pattern with suffix").matching("Data")
    let Event::Sto$0 { code: _ } = event; // complete_with_source("record pattern with suffix").matching("Stop")
}

pub fn unqualified_unfinished(event: Event) {
    let Sta$0 = event // complete_with_source("unqualified expected pattern without semicolon").matching("Start")
}

pub fn unqualified_edit(event: Event) {
    let Sta$0 = event; // complete_with_source("unqualified expected pattern with suffix").matching("Start")
}

//- /src/eof.rs
pub fn run(event: crate::Event) {
    match event {
        crate::Event::Sta$0 // complete_with_source("qualified name pattern at EOF").matching("Start")
//- /src/empty_qualified_eof.rs
pub fn run(event: crate::Event) {
    match event {
        crate::Event::$0 // complete_with_source("empty qualified pattern segment at EOF").matching("Start")
"#,
        expect![[r#"
            variant Start // qualified name pattern with suffix
            variant Data // tuple pattern with suffix
            variant Stop // record pattern with suffix
            variant Start // unqualified expected pattern without semicolon
            variant Start // unqualified expected pattern with suffix
            variant Start // qualified name pattern at EOF
            variant Start // empty qualified pattern segment at EOF
        "#]],
    );
}
