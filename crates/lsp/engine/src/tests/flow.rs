use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn answers_lsp_queries_from_saved_project() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_saved_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        /// Display name shown in the UI.
        pub struct Name;

        /// User account stored by the service.
        pub struct User {
            /// The user's display name.
            pub name: Name,
        }

        /// Builds a user.
        pub fn make_user() -> User {
            User { name: Name }
        }

        pub fn demo() {
            let user = make_u$goto$ser();
            let _hover = make_$hover$user();
            let _name = user.na$complete$;
        }
        "#,
    )
    .await;

    fixture
        .check(
            &[
                LspQuery::goto_definition("goto function", "goto"),
                LspQuery::hover("hover function", "hover"),
                LspQuery::completion("complete field", "complete"),
                LspQuery::document_symbol("document symbols", "src/lib.rs"),
            ],
            expect![[r#"
                goto function
                - /src/lib.rs:10:7-10:16

                hover function
                - range: /src/lib.rs:16:17-16:26
                - markdown:
                  ```rust
                  lsp_saved_flow::make_user
                  ```

                  ```rust
                  pub fn make_user() -> User
                  ```

                  Builds a user.

                complete field
                - name Field
                  detail: pub name: Name
                  edit: /src/lib.rs:17:21-17:23 -> name

                document symbols
                - Struct Name 1:11-1:15
                - Struct User 4:11-4:15
                  - Field name 6:8-6:12
                - Function make_user 10:7-10:16
                - Function demo 14:7-14:11
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
