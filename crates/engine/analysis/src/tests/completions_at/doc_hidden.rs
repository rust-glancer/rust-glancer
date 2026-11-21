use expect_test::expect;

use crate::tests::utils::check_analysis;

#[test]
fn reexports_keep_declaration_and_route_visibility() {
    check_analysis(
        r#"
//- /api/src/lib.rs crate:api root:api
pub mod source {
    #[doc(hidden)]
    pub struct ItemHidden;
    #[unstable(feature = "internal", issue = "none")]
    pub struct ItemUnstable;
    #[doc(cfg(hidden))]
    pub struct ItemVisible;
}

pub mod aliases {
    #[doc(hidden)]
    pub use crate::source::ItemVisible as ItemHiddenAlias;
}

pub mod glob {
    pub use crate::source::*;
}

pub mod nested_glob {
    pub use crate::glob::*;
}

pub mod named {
    pub use crate::source::ItemHidden as ItemNamed;
    pub use crate::aliases::ItemHiddenAlias as ItemDocumented;
    #[doc(inline)]
    pub use crate::source::ItemHidden as ItemInline;
}

pub mod restricted {
    #[doc(hidden)]
    pub use crate::source::*;
    pub(crate) use crate::source::*;
}

fn internal() {
    let _: nested_glob::Item$0; // complete("inside the defining crate").in_lib("api").matching("Item")
}

//- /app/src/lib.rs crate:app root:app deps:api
#[doc(hidden)]
pub use api::source::ItemVisible as ItemLocalAlias;

fn external() {
    let _: api::source::Item$0; // complete("direct items").in_lib("app").matching("Item")
    let _: api::nested_glob::Item$0; // complete("nested glob").in_lib("app").matching("Item")
    let _: api::aliases::Item$0; // complete("hidden import spelling").in_lib("app").matching("Item")
    let _: api::named::Item$0; // complete("named re-exports").in_lib("app").matching("Item")
    let _: api::restricted::Item$0; // complete("private route cannot expose public hidden route").in_lib("app").matching("Item")
    let _: ItemLocalA$0; // complete("local alias of a foreign item").in_lib("app").matching("ItemLocalAlias")
}
"#,
        expect![[r#"
            // inside the defining crate
            st ItemHidden
            st ItemUnstable
            st ItemVisible

            st ItemVisible // direct items
            st ItemVisible // nested glob
            <none> // hidden import spelling

            // named re-exports
            st ItemDocumented
            st ItemInline

            <none> // private route cannot expose public hidden route
            st ItemLocalAlias // local alias of a foreign item
        "#]],
    );
}

#[test]
fn auto_import_uses_the_documented_alias_of_a_hidden_crate() {
    check_analysis(
        r#"
//- /hidden/src/lib.rs crate:hidden root:hidden
#![doc(hidden)]
pub struct RootApi;

//- /app/src/lib.rs crate:app root:app deps:hidden
#[doc(inline)]
pub use hidden as documented;

fn external() {
    let _: RootA$0; // complete_verbose("auto-import the documented alias").in_lib("app").auto_import().matching("RootApi")
}
"#,
        expect![[r#"
            // auto-import the documented alias
            struct RootApi
              detail: struct RootApi (use crate::documented::RootApi)
              sort: 05-auto-import:0003|00|RootApi|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(1), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 73..78
              additional: 44..44 => "\nuse crate::documented::RootApi;"
        "#]],
    );
}
