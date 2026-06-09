use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn queries_use_dirty_full_text_overlay() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct SavedUser {
            pub saved_field: SavedName,
        }

        pub struct SavedName;

        pub fn demo(user: SavedUser) {
            let _ = user.saved_field;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub struct DirtyUser {
    /// Field that exists only in the unsaved buffer.
    pub dirty_field: DirtyName,
}

pub struct DirtyName;

pub fn demo(user: DirtyUser) {
    let _completion = user.$complete$;
    let _hover = user.dirty_$hover$field;
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete dirty field", "complete"),
                LspQuery::hover("hover dirty field", "hover"),
                LspQuery::document_symbol("dirty document symbols", "src/lib.rs"),
            ],
            expect![[r#"
                complete dirty field
                - dirty_field Field
                  detail: pub dirty_field: DirtyName
                  edit: /src/lib.rs:9:27-9:27 -> dirty_field

                hover dirty field
                - range: /src/lib.rs:10:22-10:33
                - markdown:
                  ```rust
                  lsp_dirty_flow::DirtyUser
                  ```

                  ```rust
                  pub dirty_field: DirtyName
                  ```

                  Field that exists only in the unsaved buffer.

                dirty document symbols
                - Struct DirtyUser 1:11-1:20
                  - Field dirty_field 3:8-3:19
                - Struct DirtyName 6:11-6:20
                - Function demo 8:7-8:11
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
