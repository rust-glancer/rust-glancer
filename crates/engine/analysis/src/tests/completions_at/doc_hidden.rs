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
