use expect_test::expect;

use crate::tests::utils::check_analysis;
#[test]
fn completes_keywords_for_each_item_list_context() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_item_list_keyword_completions
$0 // complete_keywords_with_source("source-file items")

pub mod nested {
    $0 // complete_keywords_with_source("module items")
}

pub struct Model;

impl Model {
    $0 // complete_keywords_with_source("inherent impl items")
}

pub trait Service {
    $0 // complete_keywords_with_source("trait items")
}

impl Service for Model {
    $0 // complete_keywords_with_source("trait impl items")
}

extern "C" {
    $0 // complete_keywords_with_source("extern block items")
}
"#,
        expect![[r#"
            // source-file items
            kw async
            kw const
            kw enum
            kw extern
            kw fn
            kw impl
            kw impl for
            kw mod
            kw pub
            kw static
            kw struct
            kw trait
            kw type
            kw union
            kw unsafe
            kw use

            // module items
            kw async
            kw const
            kw enum
            kw extern
            kw fn
            kw impl
            kw impl for
            kw mod
            kw pub
            kw static
            kw struct
            kw trait
            kw type
            kw union
            kw unsafe
            kw use

            // inherent impl items
            kw async
            kw const
            kw extern
            kw fn
            kw pub
            kw unsafe

            // trait items
            kw async
            kw const
            kw extern
            kw fn
            kw type
            kw unsafe

            // trait impl items
            kw async
            kw const
            kw extern
            kw fn
            kw type
            kw unsafe

            // extern block items
            kw fn
            kw pub
            kw static
            kw unsafe
        "#]],
    );
}

#[test]
fn completes_type_and_qualified_item_keywords_without_cross_context_fallbacks() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_contextual_keyword_completions
pub mod scope {}
pub trait Service {}

pub struct Holder {
    dyn_ty: dy$0, // complete_keywords_verbose_with_source("dyn type")
    fn_ty: f$0, // complete_keywords_with_source("fn type")
    for_ty: fo$0, // complete_keywords_with_source("for type")
    no_impl_trait: im$0, // complete_keywords_with_source("impl Trait excluded from fields")
}

pub fn inspect(
    value: im$0, // complete_keywords_with_source("impl Trait function parameter")
) -> im$0 { // complete_keywords_with_source("impl Trait function return")
    loop {}
}

mod visibility_context { pub(crate) f$0 } // complete_keywords_with_source("after visibility")
mod unsafe_context { unsafe f$0 } // complete_keywords_with_source("after unsafe")
mod async_context { async f$0 } // complete_keywords_with_source("after async")
mod extern_context { extern c$0 } // complete_keywords_with_source("after extern")
mod const_context { const f$0 } // complete_keywords_with_source("after const")

mod extern_crate_context { extern crate dep$0; } // complete_keywords_with_source("extern crate name has no item fallback")
mod restricted_visibility_context {
    pub(in crate::scope$0) struct Hidden; // complete_keywords_with_source("restricted visibility has no item fallback")
}
"#,
        expect![[r#"
            // dyn type
            keyword dyn
              detail: keyword dyn
              sort: ~keyword:00:dyn
              replace: 71..73
              snippet: dyn ${1:Trait}

            // fn type
            kw fn
            kw for

            kw for // for type
            <none> // impl Trait excluded from fields
            kw impl // impl Trait function parameter
            kw impl // impl Trait function return
            kw fn // after visibility
            kw fn // after unsafe
            kw fn // after async
            kw crate // after extern
            kw fn // after const
            <none> // extern crate name has no item fallback
            <none> // restricted visibility has no item fallback
        "#]],
    );
}

#[test]
fn completes_path_root_keywords_from_an_unqualified_body_site() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_path_root_keyword_completions
pub fn body() {
    let _ = cr$0; // complete("path root keyword completions")
}
"#,
        expect![[r#"
            // path root keyword completions
            fn body
            kw crate
        "#]],
    );
}
