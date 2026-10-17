use expect_test::expect;

use super::utils::check_analysis_with_fake_sysroot;

#[test]
fn builtin_derives_expose_methods_and_satisfy_trait_bounds() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:builtin_derives
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key;

pub trait KeyMethods {
    fn usable(&self) -> bool;
}
impl<T: Clone + Copy + core::fmt::Debug + Default + core::hash::Hash + PartialEq + Eq + PartialOrd + Ord> KeyMethods for T {
    fn usable(&self) -> bool { true }
}

pub fn use_it() {
    let key = Key::default()$0; // ty("derived default").in_lib("builtin_derives")
    let copy = key.clone()$0; // ty("derived clone").in_lib("builtin_derives")
    let same = key.eq(&copy)$0; // ty("derived comparison").in_lib("builtin_derives")
    let order = key.cmp(&copy)$0; // ty("derived ordering").in_lib("builtin_derives")
    let usable = key.usable()$0; // ty("all nine bounds").in_lib("builtin_derives")
    key.cl$0; // complete("clone completion").matching("clone").in_lib("builtin_derives")
}
"#,
        expect![[r#"
            nominal struct builtin_derives[lib]::crate::Key // derived default
            nominal struct builtin_derives[lib]::crate::Key // derived clone
            bool // derived comparison
            nominal enum core[lib]::crate::cmp::Ordering // derived ordering
            bool // all nine bounds

            // clone completion
            trait_method clone
            trait_method clone_from
        "#]],
    );
}

#[test]
fn builtin_derives_handle_aliases_cfg_generics_and_macro_output() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:derive_generics
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
    let wrapped = original.clone()$0; // ty("generic aliased derive").in_lib("derive_generics")
    let choice = Choice::<NoDefault>::default()$0; // ty("unit default needs no T bound").in_lib("derive_generics")
    let generated = Generated.clone()$0; // ty("derive in macro output").in_lib("derive_generics")
}
"#,
        expect![[r#"
            nominal struct derive_generics[lib]::crate::Wrapper<nominal struct derive_generics[lib]::crate::Value> // generic aliased derive
            nominal enum derive_generics[lib]::crate::Choice<nominal struct derive_generics[lib]::crate::NoDefault> // unit default needs no T bound
            nominal struct derive_generics[lib]::crate::Generated // derive in macro output
        "#]],
    );
}

#[test]
fn builtin_derives_keep_rename_edits_on_written_type_names() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:derive_rename
#[derive(Clone)]
pub struct Fo$0o; // rename("rename derived type", "Bar").in_lib("derive_rename")

pub type Alias = Foo;
"#,
        expect![[r#"
            // rename derived type
            target `Foo` @ derive_rename/src/lib.rs:2:12-2:15
            `Foo` -> `Bar` @ derive_rename/src/lib.rs:2:12-2:15
            `Foo` -> `Bar` @ derive_rename/src/lib.rs:4:18-4:21
        "#]],
    );
}

#[test]
fn builtin_derives_resolve_dollar_crate_in_exported_macro_output() {
    check_analysis_with_fake_sysroot(
        r#"
//- /helpers/src/lib.rs crate:derive_helpers root:helpers
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

//- /app/src/lib.rs crate:derive_app root:app deps:derive_helpers
mod nested {
    derive_helpers::declare!(Generated, Choice);
}

pub fn use_it() {
    let generated = nested::Generated.clone()$0; // ty("derived struct clone").in_lib("derive_app")
    let choice = nested::Choice::Empty.clone()$0; // ty("derived enum clone").in_lib("derive_app")
}
"#,
        expect![[r#"
            nominal struct derive_app[lib]::crate::nested::Generated // derived struct clone
            nominal enum derive_app[lib]::crate::nested::Choice // derived enum clone
        "#]],
    );
}

#[test]
fn builtin_derives_allow_clone_with_a_qualified_non_clone_projection() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:derive_projection
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
    let cloned = value.clone()$0; // ty("clone with a non-Clone associated type").in_lib("derive_projection")
}
"#,
        expect![[r#"
            nominal struct derive_projection[lib]::crate::Wrap<nominal struct derive_projection[lib]::crate::Owner> // clone with a non-Clone associated type
        "#]],
    );
}
