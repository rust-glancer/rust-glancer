use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn external_source_change_refreshes_saved_project() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_external_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct ExternalUser {
            pub old_field: OldName,
        }

        pub struct OldName;

        pub fn demo(user: ExternalUser) {
            let _completion = user.$complete$;
            let _hover = user.old_$hover$field;
        }
        "#,
    )
    .await;

    fixture
        .check(
            &[
                LspQuery::completion("complete field before external change", "complete"),
                LspQuery::hover("hover field before external change", "hover"),
                LspQuery::document_symbol("document symbols before external change", "src/lib.rs"),
            ],
            expect![[r#"
                complete field before external change
                - old_field Field
                  detail: pub old_field: OldName
                  edit: /src/lib.rs:7:27-7:27 -> old_field

                hover field before external change
                - range: /src/lib.rs:8:22-8:31
                - markdown:
                  ```rust
                  lsp_external_flow::ExternalUser
                  ```

                  ```rust
                  pub old_field: OldName
                  ```

                document symbols before external change
                - Struct ExternalUser 0:11-0:23
                  - Field old_field 1:8-1:17
                - Struct OldName 4:11-4:18
                - Function demo 6:7-6:11
            "#]],
        )
        .await;

    fixture
        .external_file_changed(
            "src/lib.rs",
            r#"pub struct ExternalUser {
    pub new_field: NewName,
}

pub struct NewName;

pub fn demo(user: ExternalUser) {
    let _completion = user.;
    let _hover = user.new_field;
}
"#,
        )
        .await;

    fixture
        .check(
            &[
                LspQuery::completion("complete field after external change", "complete"),
                LspQuery::hover("hover field after external change", "hover"),
                LspQuery::document_symbol("document symbols after external change", "src/lib.rs"),
            ],
            expect![[r#"
                complete field after external change
                - new_field Field
                  detail: pub new_field: NewName
                  edit: /src/lib.rs:7:27-7:27 -> new_field

                hover field after external change
                - range: /src/lib.rs:8:22-8:31
                - markdown:
                  ```rust
                  lsp_external_flow::ExternalUser
                  ```

                  ```rust
                  pub new_field: NewName
                  ```

                document symbols after external change
                - Struct ExternalUser 0:11-0:23
                  - Field new_field 1:8-1:17
                - Struct NewName 4:11-4:18
                - Function demo 6:7-6:11
            "#]],
        )
        .await;

    fixture.check_notification_effects(expect![[r#"
        notifications
        - inlay hint refresh
    "#]]);

    fixture.shutdown().await;
}

#[tokio::test]
async fn external_created_module_file_refreshes_saved_project() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_external_created"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod generated;

        pub fn demo(user: generated::GeneratedUser) {
            let _completion = user.$complete$;
            let _hover = user.generated_$hover$field;
        }
        "#,
    )
    .await;

    fixture
        .external_file_changed(
            "src/generated.rs",
            r#"pub struct GeneratedUser {
    pub generated_field: GeneratedName,
}

pub struct GeneratedName;
"#,
        )
        .await;

    fixture
        .check(
            &[
                LspQuery::completion("complete field after external module creation", "complete"),
                LspQuery::hover("hover field after external module creation", "hover"),
                LspQuery::document_symbol(
                    "document symbols after external module creation",
                    "src/generated.rs",
                ),
            ],
            expect![[r#"
                complete field after external module creation
                - generated_field Field
                  detail: pub generated_field: GeneratedName
                  edit: /src/lib.rs:3:27-3:27 -> generated_field

                hover field after external module creation
                - range: /src/lib.rs:4:22-4:37
                - markdown:
                  ```rust
                  lsp_external_created::generated::GeneratedUser
                  ```

                  ```rust
                  pub generated_field: GeneratedName
                  ```

                document symbols after external module creation
                - Struct GeneratedUser 0:11-0:24
                  - Field generated_field 1:8-1:23
                - Struct GeneratedName 4:11-4:24
            "#]],
        )
        .await;

    fixture.check_notification_effects(expect![[r#"
        notifications
        - inlay hint refresh
    "#]]);

    fixture.shutdown().await;
}
