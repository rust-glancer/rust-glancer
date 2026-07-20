use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn resolves_and_renders_raw_identifiers_across_semantic_surfaces() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_raw_identifiers"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct r#type<r#match> {
            pub r#struct: r#match,
        }

        impl<r#match> r#type<r#match> {
            pub fn r#fn(&self) -> &r#match {
                &self.r#struct
            }
        }

        pub mod r#mod {
            pub struct r#enum;
        }

        pub struct r#gen;

        use r#mod::r#enum as Imported;

        macro_rules! r#macro {
            () => {
                pub struct r#async;
            };
        }
        r#macro!();

        pub fn inspect(value: r#ty$type_hover$pe<u8>, imported: Imported, generated: r#as$generated_hover$ync) {
            let _field = value.r#str$field_hover$uct;
            let _method = value.r#f$method_hover$n();
            let _: ty$type_completion$ = value;
            let _ = value.str$field_completion$;
            let _ = value.f$method_completion$;
            let _ = imported;
            let _ = generated;
        }
        "#,
        &[
            AnalysisQuery::hover("hover raw type", "type_hover"),
            AnalysisQuery::goto("goto raw type", "type_hover"),
            AnalysisQuery::hover("hover raw field", "field_hover"),
            AnalysisQuery::hover("hover raw method", "method_hover"),
            AnalysisQuery::hover("hover macro-generated raw type", "generated_hover"),
            AnalysisQuery::complete("complete raw type", "type_completion"),
            AnalysisQuery::complete("complete raw field", "field_completion"),
            AnalysisQuery::complete("complete raw method", "method_completion"),
        ],
        expect![[r#"
            hover raw type
            - range: 26:23-26:29
            - block:
              kind: struct
              path: analysis_raw_identifiers::r#type
              signature:
                pub struct r#type<r#match> {
                    pub r#struct: r#match,
                }

            goto raw type
            - struct r#type @ 1:12-1:18

            hover raw field
            - range: 27:24-27:32
            - block:
              kind: field
              path: analysis_raw_identifiers::r#type
              signature:
                pub r#struct: r#match

            hover raw method
            - range: 28:25-28:29
            - block:
              kind: method
              path: analysis_raw_identifiers::r#type::r#fn
              signature:
                pub fn r#fn(&self) -> &r#match

            hover macro-generated raw type
            - range: 26:66-26:73
            - block:
              kind: struct
              path: analysis_raw_identifiers::r#async
              signature:
                pub struct r#async

            complete raw type
            - struct Imported
            - struct r#async
            - struct r#gen
            - module r#mod
            - struct r#type

            complete raw field
            - inherent_method r#fn
            - field r#struct

            complete raw method
            - inherent_method r#fn
            - field r#struct
        "#]],
    );
}

#[test]
fn presentation_uses_the_package_edition_instead_of_a_global_keyword_set() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_2021_identifiers"
        version = "0.1.0"
        edition = "2021"

        //- /src/lib.rs
        pub struct gen;

        pub fn inspect(value: g$gen_hover$en) {
            let _: ge$gen_completion$ = value;
        }
        "#,
        &[
            AnalysisQuery::hover("hover 2021 identifier", "gen_hover"),
            AnalysisQuery::complete("complete 2021 identifier", "gen_completion"),
        ],
        expect![[r#"
            hover 2021 identifier
            - range: 3:23-3:26
            - block:
              kind: struct
              path: analysis_2021_identifiers::gen
              signature:
                pub struct gen

            complete 2021 identifier
            - struct gen
        "#]],
    );
}

#[test]
fn prepare_rename_preserves_the_selected_occurrence_edition() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [workspace]
        members = ["crates/dep", "crates/app"]
        resolver = "3"

        //- /crates/dep/Cargo.toml
        [package]
        name = "dep"
        version = "0.1.0"
        edition = "2021"

        //- /crates/dep/src/lib.rs
        pub struct gen;

        //- /crates/app/Cargo.toml
        [package]
        name = "app"
        version = "0.1.0"
        edition = "2024"

        [dependencies]
        dep = { path = "../dep" }

        //- /crates/app/src/lib.rs
        pub fn inspect(_: dep::r#g$cross_edition_use$en) {}
        "#,
        &[
            AnalysisQuery::prepare_rename("prepare cross-edition raw use", "cross_edition_use")
                .in_lib("app"),
        ],
        expect![[r#"
            prepare cross-edition raw use
            - `r#gen` @ app/src/lib.rs:1:24-1:29
        "#]],
    );
}

#[test]
fn canonical_raw_lifetimes_are_escaped_again_in_signatures() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_raw_lifetimes"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Borrowed<'r#fn> {
            pub value: &'r#fn str,
        }

        pub fn borrow<'r#fn>(value: &'r#fn str) -> Borrowed<'r#fn> {
            Borrowed { value }
        }

        pub fn inspect(value: Borr$raw_lifetime_hover$owed<'static>) {}
        "#,
        &[AnalysisQuery::hover(
            "hover signature with raw lifetime",
            "raw_lifetime_hover",
        )],
        expect![[r#"
            hover signature with raw lifetime
            - range: 9:23-9:31
            - block:
              kind: struct
              path: analysis_raw_lifetimes::Borrowed
              signature:
                pub struct Borrowed<'r#fn> {
                    pub value: &'r#fn str,
                }
        "#]],
    );
}

#[test]
fn rename_treats_raw_syntax_as_presentation_of_one_semantic_name() {
    check_analysis_queries(
        r#"
        //- /Cargo.toml
        [package]
        name = "analysis_raw_rename"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct r#ty$raw_decl$pe;

        pub fn identity(value: r#ty$raw_use$pe) -> r#type {
            value
        }
        "#,
        &[
            AnalysisQuery::prepare_rename("prepare raw rename", "raw_use"),
            AnalysisQuery::rename("rename raw identifier to keyword", "raw_decl", "match"),
        ],
        expect![[r#"
            prepare raw rename
            - `r#type` @ src/lib.rs:3:24-3:30

            rename raw identifier to keyword
            - target `r#type` @ src/lib.rs:1:12-1:18
            - `r#type` -> `r#match` @ src/lib.rs:1:12-1:18
            - `r#type` -> `r#match` @ src/lib.rs:3:24-3:30
            - `r#type` -> `r#match` @ src/lib.rs:3:35-3:41
        "#]],
    );
}
