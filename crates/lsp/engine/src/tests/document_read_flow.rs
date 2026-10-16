use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn document_reads_combine_current_locals_with_saved_global_semantics() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_current_document_reads"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Name;

        pub struct User {
            pub name: Name,
        }

        pub fn make_user() -> User {
            User { name: Name }
        }

        pub fn inspect() {}

        pub fn untouched() {
            let saved = make_user();
        }

        pub fn keep(user: &User, count: u32) {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let current = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub struct Name;

pub struct User {
    pub name: Name,
}

pub fn make_user() -> User {
    User { name: Name }
}

$range_start$pub fn inspect() {
    let user = make_user();
    let alias = user;
    let _hover = alias.na$hover$me;
    let _copy = ali$definition$as;
    let _highlight = al$highlight$ias;
    let callback = |closure_user| -> User { closure_user };
    keep(&user, 1);
}$range_end$

pub fn untouched(_changed: usize) {
    let unrelated = make_user();
}

pub fn keep(user: &User, count: u32) {}
"#,
        )
        .await;

    fixture
        .check_dirty_queries(
            &current,
            &[
                LspQuery::hover("hover through a current local", "hover"),
                LspQuery::goto_definition("navigate to a current local", "definition"),
                LspQuery::document_highlight("highlight a current local", "highlight"),
                LspQuery::inlay_hint(
                    "hints for the selected current body",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
            ],
            expect![[r#"
                // hover through a current local
                range: /src/lib.rs:14:23-14:27
                markdown:
                  ```rust
                  lsp_current_document_reads::User

                  pub name: Name
                  ```

                /src/lib.rs:13:8-13:13 // navigate to a current local

                // highlight a current local
                read 13:8-13:13
                read 14:17-14:22
                read 15:16-15:21
                read 16:21-16:26

                // hints for the selected current body
                `: User` type @ 12:12
                `: User` type @ 13:13
                `: Name` type @ 14:14
                `: User` type @ 15:13
                `: User` type @ 16:18
                `: |User| -> User` type @ 17:16
                `: User` type @ 17:32
                `count:` parameter @ 18:16
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn document_highlights_saved_items_when_editor_source_is_exact() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_exact_document_highlights"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait Serv$0ice {} // highlight("highlight an exact trait item")

        pub fn insp$0ect() {} // highlight("highlight an exact function item")

        pub fn use_items<T: Service>() {
            inspect();
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .check(expect![[r#"
            // highlight an exact trait item
            read 0:10-0:17
            read 4:20-4:27

            // highlight an exact function item
            read 2:7-2:14
            read 5:4-5:11
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn inlay_hints_skip_bodyless_declarations_and_leave_engine_available() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_bodyless_inlay"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait Service {
            fn required(&self);
        }

        pub struct Val$hover$ue;

        $range_start$pub fn inspect() {
            let value = Value;
        }$range_end$
        "#,
    )
    .await;

    fixture
        .check_queries(
            &[
                LspQuery::inlay_hint(
                    "saved inlay hints skip a trait method",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
                LspQuery::hover("engine remains available after saved inlay hints", "hover"),
            ],
            expect![[r#"
                `: Value` type @ 7:13 // saved inlay hints skip a trait method

                // engine remains available after saved inlay hints
                range: /src/lib.rs:4:11-4:16
                markdown:
                  ```rust
                  lsp_bodyless_inlay::Value

                  pub struct Value
                  ```
            "#]],
        )
        .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub trait Service {
    fn required(&self);
}

pub struct Val$hover$ue;

$range_start$pub fn inspect() {
    let value = Value;
}$range_end$

fn unfinished
"#,
        )
        .await;

    fixture
        .check_dirty_queries(
            &dirty,
            &[
                LspQuery::inlay_hint(
                    "dirty inlay hints skip bodyless and unfinished items",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
                LspQuery::hover("engine remains available after dirty inlay hints", "hover"),
            ],
            expect![[r#"
                `: Value` type @ 8:13 // dirty inlay hints skip bodyless and unfinished items

                // engine remains available after dirty inlay hints
                range: /src/lib.rs:5:11-5:16
                markdown:
                  ```rust
                  lsp_bodyless_inlay::Value

                  pub struct Value
                  ```
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
