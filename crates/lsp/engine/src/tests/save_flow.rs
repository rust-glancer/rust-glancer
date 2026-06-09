use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn save_promotes_dirty_text_to_saved_project() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_save_flow"
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
pub struct SavedUser {
    pub renamed_field: SavedName,
}

pub struct SavedName;

pub fn demo(user: SavedUser) {
    let _completion = user.$complete$;
    let _hover = user.renamed_$hover$field;
}
"#,
            ),
        )
        .await;

    fixture.did_save_dirty(&dirty).await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete saved field after save", "complete"),
                LspQuery::hover("hover saved field after save", "hover"),
                LspQuery::document_symbol("saved document symbols after save", "src/lib.rs"),
            ],
            expect![[r#"
                complete saved field after save
                - renamed_field Field
                  detail: pub renamed_field: SavedName
                  edit: /src/lib.rs:8:27-8:27 -> renamed_field

                hover saved field after save
                - range: /src/lib.rs:9:22-9:35
                - markdown:
                  ```rust
                  lsp_save_flow::SavedUser
                  ```

                  ```rust
                  pub renamed_field: SavedName
                  ```

                saved document symbols after save
                - Struct SavedUser 1:11-1:20
                  - Field renamed_field 2:8-2:21
                - Struct SavedName 5:11-5:20
                - Function demo 7:7-7:11
            "#]],
        )
        .await;

    fixture.check_notification_effects(expect![[r#"
        notifications
        - inlay hint refresh
    "#]]);

    fixture.shutdown().await;
}
