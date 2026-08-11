//! User-typing states for narrow syntax families such as attributes and strings.

use expect_test::expect;

use super::super::super::utils::{AnalysisQuery, check_analysis_queries};

/// Delimited and transform-like completions need both sides of the editor-state split.
#[test]
fn completes_strings_fragments_and_postfix_before_and_after_closing_syntax_exists() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_specialized_typed_states"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod abi_edit;
pub mod abi_eof;
pub mod environment_edit;
pub mod environment_eof;
pub mod format_edit;
pub mod format_eof;
pub mod fragment_edit;
pub mod fragment_eof;
pub mod postfix_edit;
pub mod postfix_eof;
pub mod raw_environment_eof;
pub mod raw_format_eof;

//- /src/format_edit.rs
pub fn run(local_capture: usize) {
    let _ = format!("{loc$format_edit$}");
}

//- /src/format_eof.rs
pub fn run(local_capture: usize) {
    let _ = format!("{loc$format_eof$
//- /src/environment_edit.rs
pub fn run() {
    let _ = env!("CARGO_MAN$environment_edit$");
}

//- /src/environment_eof.rs
pub fn run() {
    let _ = env!("CARGO_MAN$environment_eof$
//- /src/raw_format_eof.rs
pub fn run(local_capture: usize) {
    let _ = format!(r#"{loc$raw_format_eof$
//- /src/raw_environment_eof.rs
pub fn run() {
    let _ = env!(r#"CARGO_MAN$raw_environment_eof$
//- /src/abi_edit.rs
extern "C-un$abi_edit$" fn foreign();

//- /src/abi_eof.rs
extern "C-un$abi_eof$
//- /src/fragment_edit.rs
macro_rules! capture {
    ($value: ex$fragment_edit$) => { $value };
}

//- /src/fragment_eof.rs
macro_rules! capture {
    ($value: ex$fragment_eof$
//- /src/postfix_edit.rs
pub fn run(condition: bool) {
    let _ = (condition.i$postfix_edit$);
}

//- /src/postfix_eof.rs
pub fn run(condition: bool) {
    let _ = condition.i$postfix_eof$
"#,
        &[
            query(
                "format capture with closing syntax",
                "format_edit",
                "local_capture",
            ),
            query(
                "format capture in unterminated string",
                "format_eof",
                "local_capture",
            ),
            query(
                "environment with closing syntax",
                "environment_edit",
                "CARGO_MANIFEST_DIR",
            ),
            query(
                "environment in unterminated string",
                "environment_eof",
                "CARGO_MANIFEST_DIR",
            ),
            query(
                "format capture in unterminated raw string",
                "raw_format_eof",
                "local_capture",
            ),
            query(
                "environment in unterminated raw string",
                "raw_environment_eof",
                "CARGO_MANIFEST_DIR",
            ),
            query("ABI with closing quote", "abi_edit", "C-unwind"),
            query("ABI before closing quote", "abi_eof", "C-unwind"),
            query(
                "macro fragment with closing syntax",
                "fragment_edit",
                "expr",
            ),
            query("macro fragment at EOF", "fragment_eof", "expr"),
            query("postfix inside complete expression", "postfix_edit", "if"),
            query("postfix at EOF", "postfix_eof", "if"),
        ],
        expect![[r#"
            format capture with closing syntax
            - variable local_capture

            format capture in unterminated string
            - variable local_capture

            environment with closing syntax
            - value CARGO_MANIFEST_DIR

            environment in unterminated string
            - value CARGO_MANIFEST_DIR

            format capture in unterminated raw string
            - variable local_capture

            environment in unterminated raw string
            - value CARGO_MANIFEST_DIR

            ABI with closing quote
            - value C-unwind

            ABI before closing quote
            - value C-unwind

            macro fragment with closing syntax
            - value expr
            - value expr_2021

            macro fragment at EOF
            - value expr
            - value expr_2021

            postfix inside complete expression
            - postfix if

            postfix at EOF
            - postfix if
        "#]],
    );
}

/// Trigger punctuation is itself a realistic pause point before any identifier prefix exists.
#[test]
fn completes_immediately_after_specialized_trigger_characters() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_empty_specialized_triggers"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod array;
pub mod attribute;
pub mod const_default;
pub mod format;
pub mod fragment;
pub mod label;
pub mod lifetime;
pub mod visibility;

//- /src/format.rs
pub fn run(local_capture: usize) {
    let _ = format!("{$empty_format$
//- /src/attribute.rs
#[$empty_attribute$
//- /src/fragment.rs
macro_rules! capture {
    ($value: $empty_fragment$
//- /src/lifetime.rs
pub fn run<'scope>(value: &'$empty_lifetime$
//- /src/label.rs
pub fn run() {
    'inner: loop {
        break '$empty_label$
//- /src/visibility.rs
pub(in crate::$empty_visibility$
//- /src/const_default.rs
const LIMIT: usize = 8;
struct Buffer<const N: usize = $empty_const_default$
//- /src/array.rs
const LIMIT: usize = 8;
struct Array([u8; $empty_array$
"#,
        &[
            query(
                "empty format capture at EOF",
                "empty_format",
                "local_capture",
            ),
            query("empty attribute path at EOF", "empty_attribute", "derive"),
            query("empty macro fragment at EOF", "empty_fragment", "expr"),
            query("empty lifetime name at EOF", "empty_lifetime", "'scope"),
            query("empty loop label at EOF", "empty_label", "'inner"),
            query(
                "empty restricted visibility segment at EOF",
                "empty_visibility",
                "visibility",
            ),
            query(
                "empty const default expression at EOF",
                "empty_const_default",
                "LIMIT",
            ),
            query(
                "empty array length expression at EOF",
                "empty_array",
                "LIMIT",
            ),
        ],
        expect![[r#"
            empty format capture at EOF
            - variable local_capture

            empty attribute path at EOF
            - attribute derive

            empty macro fragment at EOF
            - value expr
            - value expr_2021

            empty lifetime name at EOF
            - lifetime 'scope

            empty loop label at EOF
            - label 'inner

            empty restricted visibility segment at EOF
            - module visibility

            empty const default expression at EOF
            - const LIMIT

            empty array length expression at EOF
            - const LIMIT
        "#]],
    );
}

/// Every attribute subdomain must work before the user closes its delimiters.
#[test]
fn completes_attribute_families_in_incomplete_attributes_at_file_end() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["macros", "app"]
resolver = "3"

//- /macros/Cargo.toml
[package]
name = "macros"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true

//- /macros/src/lib.rs
extern crate proc_macro;

#[proc_macro_attribute]
pub fn traced(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    item
}

#[proc_macro_derive(Stored)]
pub fn stored(_item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[features]
default = []
serde-support = []

[dependencies]
macros = { path = "../macros" }

//- /app/src/lib.rs
pub mod attribute_macro;
pub mod attribute_path;
pub mod cfg_feature;
pub mod cfg_key;
pub mod compatibility;
pub mod derive_builtin;
pub mod derive_macro;
pub mod diagnostic;
pub mod lint;
pub mod repr;

//- /app/src/attribute_path.rs
#[der$attribute_path_eof$
//- /app/src/attribute_macro.rs
#[macros::tra$attribute_macro_eof$
//- /app/src/derive_builtin.rs
#[derive(Cl$derive_builtin_eof$
//- /app/src/derive_macro.rs
#[derive(macros::Sto$derive_macro_eof$
//- /app/src/lint.rs
#[allow(dead$lint_eof$
//- /app/src/repr.rs
#[repr(tra$repr_eof$
//- /app/src/cfg_key.rs
#[cfg(tar$cfg_key_eof$
//- /app/src/cfg_feature.rs
#[cfg(feature = "serde-$cfg_feature_eof$
//- /app/src/diagnostic.rs
#[diagnostic::on_unimplemented(mes$diagnostic_eof$
//- /app/src/compatibility.rs
#[deprecated(si$compatibility_eof$
"#,
        &[
            app_query("attribute path at EOF", "attribute_path_eof", "derive"),
            app_query(
                "attribute proc macro at EOF",
                "attribute_macro_eof",
                "traced",
            ),
            app_query("builtin derive at EOF", "derive_builtin_eof", "Clone"),
            app_query("derive proc macro at EOF", "derive_macro_eof", "Stored"),
            app_query("lint input at EOF", "lint_eof", "dead_code"),
            app_query("repr input at EOF", "repr_eof", "transparent"),
            app_query("cfg key at EOF", "cfg_key_eof", "target_arch"),
            app_query(
                "cfg feature in unterminated attribute string",
                "cfg_feature_eof",
                "serde-support",
            ),
            app_query("diagnostic input at EOF", "diagnostic_eof", "message"),
            app_query("compatibility input at EOF", "compatibility_eof", "since"),
        ],
        expect![[r#"
            attribute path at EOF
            - attribute derive

            attribute proc macro at EOF
            - macro traced

            builtin derive at EOF
            - value Clone

            derive proc macro at EOF
            - macro Stored

            lint input at EOF
            - value dead_code

            repr input at EOF
            - value transparent

            cfg key at EOF
            - value target_arch

            cfg feature in unterminated attribute string
            - value serde-support

            diagnostic input at EOF
            - value message

            compatibility input at EOF
            - value since
        "#]],
    );
}

/// Apostrophe, visibility, dependency, and const-expression classifiers each own syntax that can
/// end at the cursor before its closing token is written.
#[test]
fn completes_remaining_specialized_families_across_typed_states() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["dependency", "app"]
resolver = "3"

//- /dependency/Cargo.toml
[package]
name = "dependency"
version = "0.1.0"
edition = "2024"

//- /dependency/src/lib.rs
pub struct Dependency;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dependency = { path = "../dependency" }

//- /app/src/lib.rs
pub mod array_edit;
pub mod array_eof;
pub mod const_arg_edit;
pub mod const_arg_eof;
pub mod const_default_edit;
pub mod const_default_eof;
pub mod const_param_edit;
pub mod const_param_eof;
pub mod extern_edit;
pub mod extern_eof;
pub mod label_edit;
pub mod label_eof;
pub mod lifetime_edit;
pub mod lifetime_eof;
pub mod visibility_edit;
pub mod visibility_eof;

pub mod outer {
    pub mod inner {}
}

//- /app/src/visibility_edit.rs
pub(in crate::visib$visibility_edit$) struct Restricted;

//- /app/src/visibility_eof.rs
pub(in crate::visib$visibility_eof$
//- /app/src/extern_edit.rs
extern crate dep$extern_edit$;

//- /app/src/extern_eof.rs
extern crate dep$extern_eof$
//- /app/src/const_default_edit.rs
const LIMIT: usize = 8;
struct Buffer<const N: usize = LIM$const_default_edit$>([u8; N]);

//- /app/src/const_default_eof.rs
const LIMIT: usize = 8;
struct Buffer<const N: usize = LIM$const_default_eof$
//- /app/src/array_edit.rs
const LIMIT: usize = 8;
struct Array([u8; LIM$array_edit$]);

//- /app/src/array_eof.rs
const LIMIT: usize = 8;
struct Array([u8; LIM$array_eof$
//- /app/src/const_param_edit.rs
pub fn run<const N: usize>() {
    let _: [u8; N$const_param_edit$];
}

//- /app/src/const_param_eof.rs
pub fn run<const N: usize>() {
    let _: [u8; N$const_param_eof$
//- /app/src/const_arg_edit.rs
const LIMIT: usize = 8;
struct Buffer<const N: usize>;

pub fn run() {
    let _: Buffer<{ LIM$const_arg_edit$ }>;
}

//- /app/src/const_arg_eof.rs
const LIMIT: usize = 8;
struct Buffer<const N: usize>;

pub fn run() {
    let _: Buffer<{ LIM$const_arg_eof$
//- /app/src/lifetime_edit.rs
pub fn run<'outer>(value: &'outer u8) -> &'out$lifetime_edit$ u8 {
    value
}

//- /app/src/lifetime_eof.rs
pub fn run<'outer>(value: &'outer u8) -> &'out$lifetime_eof$
//- /app/src/label_edit.rs
pub fn run() {
    'inner: loop {
        break 'inn$label_edit$ 1;
    }
}

//- /app/src/label_eof.rs
pub fn run() {
    'inner: loop {
        break 'inn$label_eof$
"#,
        &[
            app_query(
                "restricted visibility with closing syntax",
                "visibility_edit",
                "visibility_edit",
            ),
            app_query(
                "restricted visibility at EOF",
                "visibility_eof",
                "visibility_eof",
            ),
            app_query("extern crate before semicolon", "extern_edit", "dependency"),
            app_query("extern crate at EOF", "extern_eof", "dependency"),
            app_query(
                "const default with closing syntax",
                "const_default_edit",
                "LIMIT",
            ),
            app_query("const default at EOF", "const_default_eof", "LIMIT"),
            app_query("array length with closing syntax", "array_edit", "LIMIT"),
            app_query("array length at EOF", "array_eof", "LIMIT"),
            app_query(
                "const parameter with closing syntax",
                "const_param_edit",
                "N",
            ),
            app_query("const parameter at EOF", "const_param_eof", "N"),
            app_query(
                "braced const argument with closing syntax",
                "const_arg_edit",
                "LIMIT",
            ),
            app_query("braced const argument at EOF", "const_arg_eof", "LIMIT"),
            app_query("lifetime with following type", "lifetime_edit", "'outer"),
            app_query("lifetime at EOF", "lifetime_eof", "'outer"),
            app_query("label with following break value", "label_edit", "'inner"),
            app_query("label at EOF", "label_eof", "'inner"),
        ],
        expect![[r#"
            restricted visibility with closing syntax
            - module visibility_edit

            restricted visibility at EOF
            - module visibility_eof

            extern crate before semicolon
            - module dependency

            extern crate at EOF
            - module dependency

            const default with closing syntax
            - const LIMIT

            const default at EOF
            - const LIMIT

            array length with closing syntax
            - const LIMIT

            array length at EOF
            - const LIMIT

            const parameter with closing syntax
            - const N

            const parameter at EOF
            - const N

            braced const argument with closing syntax
            - const LIMIT

            braced const argument at EOF
            - const LIMIT

            lifetime with following type
            - lifetime 'outer

            lifetime at EOF
            - lifetime 'outer

            label with following break value
            - label 'inner

            label at EOF
            - label 'inner
        "#]],
    );
}

fn query(title: &'static str, marker: &'static str, label: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_with_source(title, marker).matching(label)
}

fn app_query(title: &'static str, marker: &'static str, label: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_with_source(title, marker)
        .in_lib("app")
        .matching(label)
}
