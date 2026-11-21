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
fn filters_foreign_members_and_expected_variants() {
    let hidden_extensions = (0..16)
        .map(|index| format!("#[doc(hidden)] fn extension_hidden_{index}(&self) {{}}\n"))
        .collect::<String>();
    let fixture = r#"
//- /api/src/lib.rs crate:api root:api
pub struct Record {
    #[doc(hidden)]
    pub field_hidden: u8,
    pub field_visible: u8,
}
impl Record {
    #[doc(hidden)]
    pub fn member_hidden(&self) -> u16 { 0 }
    pub fn member_visible(&self) {}
    #[doc(hidden)]
    pub const member_hidden_const: u8 = 0;
    pub const member_visible_const: u8 = 0;
}
#[doc(hidden)]
impl Record {
    pub fn member_impl_only(&self) -> u16 { 0 }
    pub const member_impl_const: u8 = 0;
}

pub enum Choice {
    #[doc(hidden)]
    VariantHidden,
    VariantVisible {
        #[doc(hidden)]
        field_hidden: u8,
        field_visible: u8,
    },
}

#[doc(hidden)]
pub enum InternalChoice { VariantInternal }

pub trait Service {
    #[doc(hidden)]
    fn member_hidden(&self) {}
    fn member_visible(&self) {}
}

pub mod extensions {
    pub trait Extensions {
        /* hidden methods */
        fn extension_visible(&self) {}
    }
    impl Extensions for super::Record {}
}

fn internal(record: Record) {
    record.member_$0; // complete("methods inside the defining crate").in_lib("api").matching("member_")
}

//- /app/src/lib.rs crate:app root:app deps:api
use api::{Choice, Record, Service};

fn external(record: Record) {
    let $0written = record.member_impl_only(); // ty("a written hidden method still resolves").in_lib("app")
    record.member_$0; // complete("foreign methods").in_lib("app").matching("member_")
    record.field_$0; // complete("foreign fields").in_lib("app").matching("field_")
    record.extension_$0; // complete("extension auto-import").in_lib("app").auto_import().matching("extension_")
    Record::member_$0; // complete("associated items").in_lib("app").matching("member_")
    let _ = Record { field_$0 }; // complete("record fields").in_lib("app").matching("field_")
    let _ = Choice::VariantVisible { field_$0 }; // complete("variant fields").in_lib("app").matching("field_")
    let _: Choice = Variant$0; // complete("expected enum variants").in_lib("app").matching("Variant")
    let _: api::InternalChoice = Variant$0; // complete("hidden expected enum").in_lib("app").matching("Variant")
}

struct Worker;
impl Service for Worker {
    fn member_$0 // complete("trait member scaffolds").in_lib("app").matching("member_")
}
"#
    .replace("/* hidden methods */", &hidden_extensions);
    check_analysis(
        &fixture,
        expect![[r#"
            // methods inside the defining crate
            im member_hidden
            im member_impl_only
            im member_visible

            u16 // a written hidden method still resolves
            im member_visible // foreign methods
            fd field_visible // foreign fields
            tm extension_visible // extension auto-import

            // associated items
            ct member_visible_const
            fn member_visible

            fd field_visible // record fields
            fd field_visible // variant fields
            ev VariantVisible // expected enum variants
            <none> // hidden expected enum
            fn member_visible // trait member scaffolds
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

#[test]
fn expected_variants_can_use_a_documented_type_alias() {
    check_analysis(
        r#"
//- /src/lib.rs crate:app
mod home {
    mod private {
        pub enum Event<T> { VariantVisible(T) }
    }
    pub type AliasU16 = private::Event<u16>;
    pub type AliasU8 = private::Event<u8>;
}
use home::{AliasU16, AliasU8};

fn external() -> AliasU8 {
    Var$0 // complete_verbose("uses the alias with matching arguments").matching("Variant")
}
"#,
        expect![[r#"
            // uses the alias with matching arguments
            variant VariantVisible
              detail: variant AliasU8::VariantVisible
              sort: !expected|00-expected|VariantVisible|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 236..239
              snippet: AliasU8::VariantVisible($0)
        "#]],
    );
}
