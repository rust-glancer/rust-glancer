use expect_test::expect;

use super::utils;
use crate::Namespace;

#[test]
fn explicit_bindings_shadow_globs_independent_of_import_order() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/named-first/src/lib.rs crate:named-first root:crates/named-first
mod explicit;
mod globbed;

use explicit::Thing;
use globbed::*;

//- /crates/named-first/src/explicit.rs
pub struct Thing;

//- /crates/named-first/src/globbed.rs
pub struct Thing;

//- /crates/glob-first/src/lib.rs crate:glob-first root:crates/glob-first
mod explicit;
mod globbed;

use globbed::*;
use explicit::Thing;

//- /crates/glob-first/src/explicit.rs
pub struct Thing;

//- /crates/glob-first/src/globbed.rs
pub struct Thing;
"#,
    );

    for package in ["named-first", "glob-first"] {
        project.lib(package).entry("Thing").assert_type_source_file(
            "explicit.rs",
            "a named import should replace a glob binding in either source order",
        );
    }
}

#[test]
fn imports_preserve_constructor_namespace_occupancy_and_value_precedence() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:constructor_import_fixture
mod named {
    pub struct Unit;
    pub struct Record { pub value: u8 }
    pub struct RestrictedTuple(u8);
}

use named::{
    Record as NamedRecord,
    RestrictedTuple as NamedRestrictedTuple,
    Unit as NamedUnit,
};

mod glob {
    pub struct Tuple(pub u8);
}

use glob::*;

mod shadowed {
    pub struct Choice;
}

use shadowed::*;

#[allow(non_snake_case)]
fn Choice() {}
"#,
    );
    let target = project.lib("constructor_import_fixture");

    target
        .entry("NamedUnit")
        .assert_type_exists("named imports should retain the unit struct type")
        .assert_value_exists("named imports should retain the unit constructor");
    target
        .entry("NamedRecord")
        .assert_type_exists("named imports should retain record struct types")
        .assert_value_missing("record structs should not gain a value binding through imports");
    target
        .entry("NamedRestrictedTuple")
        .assert_type_exists("the visibility of a tuple struct type follows its declaration")
        .assert_value_missing(
            "a tuple constructor should not be imported beyond its positional fields' visibility",
        );
    target
        .entry("Tuple")
        .assert_type_exists("glob imports should retain tuple struct types")
        .assert_value_exists("glob imports should retain tuple constructors");
    target
        .entry("Choice")
        .assert_type_exists("the glob constructor's type slot should remain visible")
        .assert_value_kind(
            crate::LocalDefKind::Function,
            "a direct function should outrank a glob-imported constructor only in value space",
        );
}

#[test]
fn enum_variant_imports_preserve_shape_namespace_occupancy() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:variant_import_fixture
mod source;

use source::Choice::{
    Record as NamedRecord,
    Tuple as NamedTuple,
    Unit as NamedUnit,
};
use source::Choice::*;

//- /src/source.rs
pub enum Choice {
    Record { value: u8 },
    Tuple(u8),
    Unit,
}
"#,
    );
    let target = project.lib("variant_import_fixture");

    for name in ["NamedRecord", "Record"] {
        target
            .entry(name)
            .assert_type_exists("record variant imports should occupy the type namespace")
            .assert_value_missing("record variant imports should not create bare values");
    }
    for name in ["NamedTuple", "Tuple", "NamedUnit", "Unit"] {
        target
            .entry(name)
            .assert_type_exists("tuple and unit variant imports should retain their type binding")
            .assert_value_exists(
                "tuple and unit variant imports should retain their value constructor",
            );
    }
    target.entry("NamedTuple").assert_type_source_file(
        "source.rs",
        "an imported variant should retain the source of its enum declaration",
    );
}

#[test]
fn equal_priority_imports_keep_the_first_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:ambiguous_import_fixture
mod first;
mod second;

use first::Thing;
use second::Thing;

//- /src/first.rs
pub struct Thing;

//- /src/second.rs
pub struct Thing;
"#,
    );

    project
        .lib("ambiguous_import_fixture")
        .entry("Thing")
        .assert_type_source_file(
            "first.rs",
            "conflicting imports should retain the first useful target",
        );
}

#[test]
fn duplicate_glob_imports_select_one_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:duplicate_glob_route_fixture
mod source {
    pub struct Thing;
}

mod first {
    pub use crate::source::Thing;
}

mod second {
    pub use crate::source::Thing;
}

use first::*;
use second::*;
"#,
    );

    project
        .lib("duplicate_glob_route_fixture")
        .entry("Thing")
        .assert_type_exists("two glob imports of one definition should keep one useful target");
}

#[test]
fn repeated_globs_keep_the_broadest_visibility_in_either_order() {
    for broad_first in [false, true] {
        let imports = |visibility: &str| {
            let private = "use crate::source::*;";
            let broad = format!("{visibility} use crate::source::*;");
            if broad_first {
                format!("{broad}\n{private}")
            } else {
                format!("{private}\n{broad}")
            }
        };
        let fixture = r#"
//- /dep/src/lib.rs crate:dep root:dep
mod source {
    pub struct Shared {}
}

mod crate_facade {
    $crate_imports
}

pub mod public_facade {
    $public_imports
}

use crate_facade::Shared as CrateWide;

//- /app/src/lib.rs crate:app root:app deps:dep
use dep::public_facade::Shared as Public;
"#
        .replace("$crate_imports", &imports("pub(crate)"))
        .replace("$public_imports", &imports("pub"));
        let project = utils::DefMapFixtureDb::build(&fixture);
        project
            .lib("dep")
            .entry("CrateWide")
            .assert_type_exists("the broader module visibility should survive either glob order");
        project
            .lib("app")
            .entry("Public")
            .assert_type_exists("public visibility should survive either glob order across crates");
    }
}

#[test]
fn resolves_nested_self_imports_without_binding_literal_self() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:self_import_fixture
mod bar {
    pub mod foo {
        pub fn work() {}
    }
}

use bar::foo::{self, self as imported_foo, work};
"#,
    );
    let target = project.lib("self_import_fixture");

    target.entry("foo").assert_module_named(
        "foo",
        "nested self imports should bind the referenced module under its own name",
    );
    target.entry("imported_foo").assert_module_named(
        "foo",
        "aliased nested self imports should keep the referenced module under the alias",
    );
    target
        .entry("work")
        .assert_value_exists("nested self imports should not interfere with sibling imports");
    target
        .entry("self")
        .assert_missing("nested self imports should not leak a literal `self` binding");
}

#[test]
fn ignores_hidden_renames() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub fn work() {}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
mod bar {
    pub fn work() {}
}

extern crate dep as _;
use bar::work as _;
"#,
    );
    let target = project.lib("app");

    target
        .entry("bar")
        .assert_type_exists("hidden renames should not remove unrelated local bindings");
    target
        .entry("dep")
        .assert_missing("hidden extern crate renames should not bind the dependency name");
    target
        .entry("work")
        .assert_missing("hidden use renames should not bind the imported item name");
}

#[test]
fn records_unresolved_named_and_glob_imports() {
    utils::check_project_def_map(
        r#"
//- /src/lib.rs crate:unresolved_import_fixture
mod existing {}

use missing::Thing;
use existing::Missing as Renamed;
pub use existing::missing::*;
"#,
        expect![[r#"
            package unresolved_import_fixture

            unresolved_import_fixture [lib]
            crate
            - existing : type [module unresolved_import_fixture[lib]::crate::existing]
            unresolved imports
            - use missing::Thing
            - use existing::Missing as Renamed
            - pub use existing::missing::*

            crate::existing
        "#]],
    );
}

#[test]
fn records_unresolved_hidden_imports_without_flagging_resolved_hidden_imports() {
    utils::check_project_def_map(
        r#"
//- /src/lib.rs crate:hidden_unresolved_import_fixture
mod existing {
    pub fn work() {}
}

use existing::work as _;
use missing::Thing as _;
"#,
        expect![[r#"
            package hidden_unresolved_import_fixture

            hidden_unresolved_import_fixture [lib]
            crate
            - existing : type [module hidden_unresolved_import_fixture[lib]::crate::existing]
            unresolved imports
            - use missing::Thing as _

            crate::existing
            - work : value [pub fn hidden_unresolved_import_fixture[lib]::crate::existing::work]
        "#]],
    );
}

#[test]
fn cyclic_imports_keep_a_stable_declaration_backed_target() {
    for (a_import, b_import, expected) in [
        (
            "pub use crate::b::*;",
            "pub use crate::a::*;",
            Some("DirectLeft"),
        ),
        ("pub use crate::b::Thing;", "pub use crate::a::*;", None),
        ("pub use crate::b::Thing;", "pub use crate::a::Thing;", None),
    ] {
        let fixture = format!(
            r#"
//- /src/lib.rs crate:import_cycle
mod a;
mod b;
mod left;
mod right;
pub use a::Thing as FromA;
pub use b::Thing as FromB;
pub use left::Thing as DirectLeft;
pub use right::Thing as DirectRight;
//- /src/a.rs
{a_import}
pub use crate::left::*;
//- /src/b.rs
{b_import}
pub use crate::right::*;
//- /src/left.rs
pub struct Thing;
//- /src/right.rs
pub struct Thing;
"#
        );
        let project = utils::DefMapFixtureDb::build(&fixture);
        let target = project.lib("import_cycle");
        let scope = &target.def_map().modules()[0].scope;
        let binding = |name: &str| {
            scope
                .entry(name)
                .and_then(|entry| entry.binding(Namespace::Types))
                .expect("cycle should have a useful type target")
                .def
        };
        assert!(
            [binding("DirectLeft"), binding("DirectRight")].contains(&binding("FromB")),
            "a conflicting cycle should retain a real declaration"
        );
        assert!(
            [binding("DirectLeft"), binding("DirectRight")].contains(&binding("FromA")),
            "a cycle must select a real declaration"
        );
        if let Some(expected) = expected {
            assert_eq!(
                binding("FromA"),
                binding(expected),
                "{a_import} / {b_import}"
            );
        }
    }
}

#[test]
fn unseeded_import_cycles_finish_without_inventing_a_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:unseeded_import_cycle
mod a { pub use crate::b::Thing; pub use crate::b::*; }
mod b { pub use crate::a::Thing; pub use crate::a::*; }
use a::Thing;
"#,
    );
    project
        .lib("unseeded_import_cycle")
        .entry("Thing")
        .assert_missing("a cycle without a declaration should finish as unresolved");
}
