use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries_with_fake_sysroot};

#[test]
fn builtin_derives_expose_methods_and_satisfy_trait_bounds() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "builtin_derives"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key;

pub trait KeyMethods {
    fn usable(&self) -> bool;
}
impl<T: Clone + Copy + core::fmt::Debug + Default + core::hash::Hash + PartialEq + Eq + PartialOrd + Ord> KeyMethods for T {
    fn usable(&self) -> bool { true }
}

pub fn use_it() {
    let key = Key::default()$default$;
    let copy = key.clone()$clone$;
    let same = key.eq(&copy)$eq$;
    let order = key.cmp(&copy)$ord$;
    let usable = key.usable()$usable$;
    key.cl$complete_clone$;
}
"#,
        &[
            AnalysisQuery::ty("derived default", "default"),
            AnalysisQuery::ty("derived clone", "clone"),
            AnalysisQuery::ty("derived comparison", "eq"),
            AnalysisQuery::ty("derived ordering", "ord"),
            AnalysisQuery::ty("all nine bounds", "usable"),
            AnalysisQuery::complete("clone completion", "complete_clone").matching("clone"),
        ]
        .map(|query| query.in_lib("builtin_derives")),
        expect![[r#"
            derived default
            - nominal struct builtin_derives[lib]::crate::Key

            derived clone
            - nominal struct builtin_derives[lib]::crate::Key

            derived comparison
            - bool

            derived ordering
            - nominal enum core[lib]::crate::cmp::Ordering

            all nine bounds
            - bool

            clone completion
            - trait_method clone
            - trait_method clone_from
        "#]],
    );
}

#[test]
fn builtin_derives_handle_aliases_cfg_generics_and_macro_output() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "derive_generics"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#![no_std]
use core::clone::Clone as Duplicate;

#[derive(Clone)]
pub struct Value;
#[cfg_attr(all(), cfg_attr(all(), derive(Duplicate)))]
pub struct Wrapper<T = Value>(T);

pub struct NoDefault;
#[derive(Default)]
pub enum Choice<T> {
    #[cfg_attr(all(), default)]
    Empty,
    Some(T),
}

macro_rules! declare {
    ($name:ident) => {
        #[derive(core::clone::Clone)]
        pub struct $name;
    };
}
declare!(Generated);

pub fn use_it(original: Wrapper<Value>) {
    let wrapped = original.clone()$wrapper$;
    let choice = Choice::<NoDefault>::default()$choice$;
    let generated = Generated.clone()$generated$;
}
"#,
        &[
            AnalysisQuery::ty("generic aliased derive", "wrapper"),
            AnalysisQuery::ty("unit default needs no T bound", "choice"),
            AnalysisQuery::ty("derive in macro output", "generated"),
        ]
        .map(|query| query.in_lib("derive_generics")),
        expect![[r#"
            generic aliased derive
            - nominal struct derive_generics[lib]::crate::Wrapper<nominal struct derive_generics[lib]::crate::Value>

            unit default needs no T bound
            - nominal enum derive_generics[lib]::crate::Choice<nominal struct derive_generics[lib]::crate::NoDefault>

            derive in macro output
            - nominal struct derive_generics[lib]::crate::Generated
        "#]],
    );
}

#[test]
fn builtin_derives_keep_rename_edits_on_written_type_names() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "derive_rename"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[derive(Clone)]
pub struct Fo$type$o;

pub type Alias = Foo;
"#,
        &[AnalysisQuery::rename("rename derived type", "type", "Bar").in_lib("derive_rename")],
        expect![[r#"
            rename derived type
            - target `Foo` @ derive_rename/src/lib.rs:2:12-2:15
            - `Foo` -> `Bar` @ derive_rename/src/lib.rs:2:12-2:15
            - `Foo` -> `Bar` @ derive_rename/src/lib.rs:4:18-4:21
        "#]],
    );
}

#[test]
fn builtin_derives_resolve_dollar_crate_in_exported_macro_output() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["helpers", "app"]
resolver = "3"

//- /helpers/Cargo.toml
[package]
name = "derive_helpers"
version = "0.1.0"
edition = "2024"

//- /helpers/src/lib.rs
pub use core::clone::Clone as Duplicate;

#[macro_export]
macro_rules! declare {
    ($name:ident, $choice:ident) => {
        #[derive($crate::Duplicate)]
        pub struct $name;
        #[cfg_attr(all(), derive($crate::Duplicate))]
        pub enum $choice { Empty }
    };
}

//- /app/Cargo.toml
[package]
name = "derive_app"
version = "0.1.0"
edition = "2024"
[dependencies]
derive_helpers = { path = "../helpers" }

//- /app/src/lib.rs
mod nested {
    derive_helpers::declare!(Generated, Choice);
}

pub fn use_it() {
    let generated = nested::Generated.clone()$struct$;
    let choice = nested::Choice::Empty.clone()$enum$;
}
"#,
        &[
            AnalysisQuery::ty("derived struct clone", "struct"),
            AnalysisQuery::ty("derived enum clone", "enum"),
        ]
        .map(|query| query.in_lib("derive_app")),
        expect![[r#"
            derived struct clone
            - nominal struct derive_app[lib]::crate::nested::Generated

            derived enum clone
            - nominal enum derive_app[lib]::crate::nested::Choice
        "#]],
    );
}

#[test]
fn builtin_derives_allow_clone_with_a_qualified_non_clone_projection() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "derive_projection"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Family { type Item; }
pub struct NoClone;
#[derive(Clone)]
pub struct Owner;
impl Family for Owner { type Item = NoClone; }

// Like PhantomData, this wrapper can be cloned without requiring its T to implement Clone.
pub struct Marker<T>(fn() -> T);
impl<T> Clone for Marker<T> {
    fn clone(&self) -> Self { Self(self.0) }
}

#[derive(Clone)]
pub struct Wrap<T: Family>(Marker<<T as Family>::Item>);

pub fn use_it(value: Wrap<Owner>) {
    let cloned = value.clone()$clone$;
}
"#,
        &[
            AnalysisQuery::ty("clone with a non-Clone associated type", "clone")
                .in_lib("derive_projection"),
        ],
        expect![[r#"
            clone with a non-Clone associated type
            - nominal struct derive_projection[lib]::crate::Wrap<nominal struct derive_projection[lib]::crate::Owner>
        "#]],
    );
}
