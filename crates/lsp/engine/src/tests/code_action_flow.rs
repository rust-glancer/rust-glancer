use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn dirty_qualified_path_root_offers_an_import_action() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [workspace]
        members = ["catalog", "app"]
        resolver = "3"

        //- /catalog/Cargo.toml
        [package]
        name = "catalog"
        version = "0.1.0"
        edition = "2024"

        //- /catalog/src/lib.rs
        pub mod collections {
            pub struct HashMap;

            impl HashMap {
                pub fn new() -> Self { Self }
            }
        }

        //- /app/Cargo.toml
        [package]
        name = "app"
        version = "0.1.0"
        edition = "2024"

        [dependencies]
        catalog = { path = "../catalog" }

        //- /app/src/lib.rs
        pub fn demo() {}
        "#,
    )
    .await;
    fixture.did_open_saved("app/src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "app/src/lib.rs",
            8,
            MarkedText::parse(
                r#"pub fn demo() {
    let _ = HashMap$action$::new();
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::code_action_only(
                "import qualified path root",
                "action",
                ls_types::CodeActionKind::QUICKFIX,
            )],
            expect![[r#"
                import qualified path root
                - quickfix Import `catalog::collections::HashMap`
                  preferred: true
                  document: /app/src/lib.rs version 8
                  edit: 0:0-0:0 -> "use catalog::collections::HashMap;\n\n"
                  result:
                    use catalog::collections::HashMap;

                    pub fn demo() {
                        let _ = HashMap::new();
                    }
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_trait_impl_action_subtracts_newly_typed_members() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_trait_code_action"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        trait Service {
            fn first(&self);
            fn second(&self);
        }

        struct Worker;

        impl Service for Worker {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            12,
            MarkedText::parse(
                r#"
// Unsaved text moves the impl and adds one member before analysis rebuilds.
trait Service {
    fn first(&self);
    fn second(&self);
}

struct Worker;

impl Service for Worker {$action$
    fn first(&self) {}
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::code_action_only(
                "implement dirty trait members",
                "action",
                ls_types::CodeActionKind::QUICKFIX,
            )],
            expect![[r#"
                implement dirty trait members
                - quickfix Implement missing trait members
                  preferred: true
                  document: /src/lib.rs version 12
                  edit: 10:22-11:0 -> "\n\n    fn second(&self) {\n        todo!()\n    }\n"
                  result:

                    // Unsaved text moves the impl and adds one member before analysis rebuilds.
                    trait Service {
                        fn first(&self);
                        fn second(&self);
                    }

                    struct Worker;

                    impl Service for Worker {
                        fn first(&self) {}

                        fn second(&self) {
                            todo!()
                        }
                    }
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn new_dirty_generic_trait_impl_supports_completion_and_bulk_implementation() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_new_dirty_trait_impl"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        trait Service<T> {
            fn handle(&self, value: T);
        }

        struct Worker<T>(T);
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"trait Service<T> {
    fn handle(&self, value: T);
}

struct Worker<T>(T);

impl<T> Service<T> for Worker<T> {$action$
    $completion$
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete new dirty trait impl", "completion"),
                LspQuery::code_action_only(
                    "implement new dirty trait impl",
                    "action",
                    ls_types::CodeActionKind::QUICKFIX,
                ),
            ],
            expect![[r#"
                complete new dirty trait impl
                - unsafe Keyword
                  detail: keyword unsafe
                  edit: /src/lib.rs:7:4-7:4 -> unsafe
                - async Keyword
                  detail: keyword async
                  edit: /src/lib.rs:7:4-7:4 -> async
                - extern Keyword
                  detail: keyword extern
                  edit: /src/lib.rs:7:4-7:4 -> extern
                - fn Keyword
                  detail: keyword fn
                  edit: /src/lib.rs:7:4-7:4 -> fn
                - const Keyword
                  detail: keyword const
                  edit: /src/lib.rs:7:4-7:4 -> const
                - type Keyword
                  detail: keyword type
                  edit: /src/lib.rs:7:4-7:4 -> type
                - handle Function
                  detail: required trait member: fn handle(&self, value: T)
                  edit: /src/lib.rs:7:4-7:4 -> "fn handle(&self, value: T) {\n    todo!()\n}"

                implement new dirty trait impl
                - quickfix Implement missing trait members
                  preferred: true
                  document: /src/lib.rs version 2
                  edit: 6:34-8:0 -> "\n    fn handle(&self, value: T) {\n        todo!()\n    }\n"
                  result:
                    trait Service<T> {
                        fn handle(&self, value: T);
                    }

                    struct Worker<T>(T);

                    impl<T> Service<T> for Worker<T> {
                        fn handle(&self, value: T) {
                            todo!()
                        }
                    }
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn qualified_rewrite_uses_utf16_ranges_and_respects_request_context() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_qualified_code_action"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod models { pub struct User; }

        fn load() {
            let _emoji = "😀"; let _: crate::models::User$action$;
        }
        "#,
    )
    .await;
    fixture.did_open_saved("src/lib.rs", 3).await;

    fixture
        .check(
            &[
                LspQuery::code_action_only(
                    "qualified rewrite",
                    "action",
                    ls_types::CodeActionKind::REFACTOR_REWRITE,
                ),
                LspQuery::code_action_only(
                    "quick fixes only",
                    "action",
                    ls_types::CodeActionKind::QUICKFIX,
                ),
                LspQuery::automatic_code_action("automatic discovery", "action"),
            ],
            expect![[r#"
                qualified rewrite
                - refactor.rewrite Replace qualified path with `use`
                  preferred: unset
                  document: /src/lib.rs version 3
                  edit: 0:0-0:0 -> "use crate::models::User;\n\n"
                  edit: 3:30-3:45 -> ""
                  result:
                    use crate::models::User;

                    mod models { pub struct User; }

                    fn load() {
                        let _emoji = "😀"; let _: User;
                    }

                quick fixes only
                - none

                automatic discovery
                - none
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
