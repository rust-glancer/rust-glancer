use crate::tests::utils;

#[test]
fn parent_textual_macro_rules_respects_child_module_source_order() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:textual_parent_macro_fixture
mod earlier {
    make_item!();
}

macro_rules! make_item {
    () => {
        pub struct Item;
    };
}

mod later {
    make_item!();
}

pub use earlier::Item as BeforeDefinition;
pub use later::Item as AfterDefinition;
"#,
    );
    let target = project.lib("textual_parent_macro_fixture");

    target
        .entry("BeforeDefinition")
        .assert_missing("parent textual macro_rules should not be visible before its definition");
    target.entry("AfterDefinition").assert_type_exists(
        "parent textual macro_rules should be visible in child modules after its definition",
    );
}

#[test]
fn same_module_textual_macro_rules_uses_latest_prior_definition() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:textual_macro_shadow_fixture
macro_rules! make {
    () => {
        pub struct A;
    };
}

make!();

macro_rules! make {
    () => {
        pub struct B;
    };
}

make!();
"#,
    );
    let target = project.lib("textual_macro_shadow_fixture");

    target
        .entry("A")
        .assert_type_exists("the first call should use the first textual definition");
    target
        .entry("B")
        .assert_type_exists("the second call should use the later textual definition");
}

#[test]
fn inner_textual_macro_rules_shadows_parent_textual_macro() {
    let project = utils::DefMapFixtureDb::build(
        r#"
//- /src/lib.rs crate:inner_textual_macro_shadow_fixture
macro_rules! make {
    () => {
        pub struct Parent;
    };
}

mod child {
    macro_rules! make {
        () => {
            pub struct Child;
        };
    }

    make!();
}

pub use child::Child;
pub use child::Parent;
"#,
    );
    let target = project.lib("inner_textual_macro_shadow_fixture");

    target
        .entry("Child")
        .assert_type_exists("child textual macro_rules should shadow the parent definition");
    target
        .entry("Parent")
        .assert_missing("the parent textual macro should not be used when child has a match");
}
