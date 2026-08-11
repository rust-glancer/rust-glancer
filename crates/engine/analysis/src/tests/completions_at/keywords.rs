use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_keywords_for_each_item_list_context() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_item_list_keyword_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
$source$

pub mod nested {
    $module$
}

pub struct Model;

impl Model {
    $inherent_impl$
}

pub trait Service {
    $trait_list$
}

impl Service for Model {
    $trait_impl$
}

extern "C" {
    $extern_block$
}
"#,
        &[
            AnalysisQuery::complete_keywords_with_source("source-file items", "source"),
            AnalysisQuery::complete_keywords_with_source("module items", "module"),
            AnalysisQuery::complete_keywords_with_source("inherent impl items", "inherent_impl"),
            AnalysisQuery::complete_keywords_with_source("trait items", "trait_list"),
            AnalysisQuery::complete_keywords_with_source("trait impl items", "trait_impl"),
            AnalysisQuery::complete_keywords_with_source("extern block items", "extern_block"),
        ],
        expect![[r#"
            source-file items
            - keyword async
            - keyword const
            - keyword enum
            - keyword extern
            - keyword fn
            - keyword impl
            - keyword impl for
            - keyword mod
            - keyword pub
            - keyword static
            - keyword struct
            - keyword trait
            - keyword type
            - keyword union
            - keyword unsafe
            - keyword use

            module items
            - keyword async
            - keyword const
            - keyword enum
            - keyword extern
            - keyword fn
            - keyword impl
            - keyword impl for
            - keyword mod
            - keyword pub
            - keyword static
            - keyword struct
            - keyword trait
            - keyword type
            - keyword union
            - keyword unsafe
            - keyword use

            inherent impl items
            - keyword async
            - keyword const
            - keyword extern
            - keyword fn
            - keyword pub
            - keyword unsafe

            trait items
            - keyword async
            - keyword const
            - keyword extern
            - keyword fn
            - keyword type
            - keyword unsafe

            trait impl items
            - keyword async
            - keyword const
            - keyword extern
            - keyword fn
            - keyword type
            - keyword unsafe

            extern block items
            - keyword fn
            - keyword pub
            - keyword static
            - keyword unsafe
        "#]],
    );
}

#[test]
fn completes_type_and_qualified_item_keywords_without_cross_context_fallbacks() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_contextual_keyword_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod scope {}
pub trait Service {}

pub struct Holder {
    dyn_ty: dy$dyn_ty$,
    fn_ty: f$fn_ty$,
    for_ty: fo$for_ty$,
    no_impl_trait: im$no_impl_trait$,
}

pub fn inspect(
    value: im$param_impl_trait$,
) -> im$return_impl_trait$ {
    loop {}
}

mod visibility_context { pub(crate) f$after_visibility$ }
mod unsafe_context { unsafe f$after_unsafe$ }
mod async_context { async f$after_async$ }
mod extern_context { extern c$after_extern$ }
mod const_context { const f$after_const$ }

mod extern_crate_context { extern crate dep$extern_crate_name$; }
mod restricted_visibility_context {
    pub(in crate::scope$restricted_visibility$) struct Hidden;
}
"#,
        &[
            AnalysisQuery::complete_keywords_verbose_with_source("dyn type", "dyn_ty"),
            AnalysisQuery::complete_keywords_with_source("fn type", "fn_ty"),
            AnalysisQuery::complete_keywords_with_source("for type", "for_ty"),
            AnalysisQuery::complete_keywords_with_source(
                "impl Trait excluded from fields",
                "no_impl_trait",
            ),
            AnalysisQuery::complete_keywords_with_source(
                "impl Trait function parameter",
                "param_impl_trait",
            ),
            AnalysisQuery::complete_keywords_with_source(
                "impl Trait function return",
                "return_impl_trait",
            ),
            AnalysisQuery::complete_keywords_with_source("after visibility", "after_visibility"),
            AnalysisQuery::complete_keywords_with_source("after unsafe", "after_unsafe"),
            AnalysisQuery::complete_keywords_with_source("after async", "after_async"),
            AnalysisQuery::complete_keywords_with_source("after extern", "after_extern"),
            AnalysisQuery::complete_keywords_with_source("after const", "after_const"),
            AnalysisQuery::complete_keywords_with_source(
                "extern crate name has no item fallback",
                "extern_crate_name",
            ),
            AnalysisQuery::complete_keywords_with_source(
                "restricted visibility has no item fallback",
                "restricted_visibility",
            ),
        ],
        expect![[r#"
            dyn type
            - keyword dyn
              detail: keyword dyn
              sort: ~keyword:00:dyn
              replace: 71..73
              snippet: dyn ${1:Trait}

            fn type
            - keyword fn
            - keyword for

            for type
            - keyword for

            impl Trait excluded from fields
            - <none>

            impl Trait function parameter
            - keyword impl

            impl Trait function return
            - keyword impl

            after visibility
            - keyword fn

            after unsafe
            - keyword fn

            after async
            - keyword fn

            after extern
            - keyword crate

            after const
            - keyword fn

            extern crate name has no item fallback
            - <none>

            restricted visibility has no item fallback
            - <none>
        "#]],
    );
}

#[test]
fn completes_path_root_keywords_from_an_unqualified_body_site() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_path_root_keyword_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn body() {
    let _ = cr$0;
}
"#,
        &[AnalysisQuery::complete(
            "path root keyword completions",
            "0",
        )],
        expect![[r#"
            path root keyword completions
            - fn body
            - keyword crate
        "#]],
    );
}
