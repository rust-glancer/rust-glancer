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
    let _completion = user.renamed_$complete$;
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
                  edit: /src/lib.rs:8:27-8:35 -> renamed_field

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
        - none
    "#]]);

    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_save_validation_preserves_the_published_generation() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_failed_save_validation"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Published;
        "#,
    )
    .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let before = fixture
        .did_save_current("src/lib.rs")
        .await
        .expect("unchanged saved source should return its published generation");

    fixture
        .did_change_full("src/lib.rs", 2, MarkedText::parse("pub struct Proposed;\n"))
        .await;
    let error = fixture
        .did_save_current("src/lib.rs")
        .await
        .expect_err("disk that does not contain the proposal must reject publication");
    let message = error.to_string();
    assert!(
        message.contains("stale") || message.contains("revision"),
        "save failure should preserve its validation cause: {error:?}"
    );

    // The rejected candidate never published. Returning the editor to the still-saved disk value
    // therefore returns the same generation, with no rollback operation.
    fixture
        .did_change_full(
            "src/lib.rs",
            3,
            MarkedText::parse("pub struct Published;\n"),
        )
        .await;
    let after = fixture
        .did_save_current("src/lib.rs")
        .await
        .expect("the prior published value should remain coherent");
    assert_eq!(after, before);

    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_manifest_rebuild_leaves_the_previous_project_queryable() {
    const MANIFEST: &str = r#"[package]
name = "lsp_failed_manifest_save"
version = "0.1.0"
edition = "2024"
"#;
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_failed_manifest_save"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Published;
        "#,
    )
    .await;
    let invalid_manifest = "[package]\nname =\n";
    fixture
        .did_open_dirty("Cargo.toml", 2, MarkedText::parse(invalid_manifest))
        .await;
    fixture.write_file_without_notification("Cargo.toml", invalid_manifest);

    fixture
        .did_save_current("Cargo.toml")
        .await
        .expect_err("invalid Cargo metadata must reject the project candidate");
    fixture.write_file_without_notification("Cargo.toml", MANIFEST);

    fixture
        .check(
            &[LspQuery::document_symbol(
                "symbols after rejected manifest rebuild",
                "src/lib.rs",
            )],
            expect![[r#"
                symbols after rejected manifest rebuild
                - Struct Published 0:11-0:20
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
