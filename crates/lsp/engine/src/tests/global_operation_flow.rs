//! Saved-project behavior shared by cross-file LSP operations.

use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn goto_implementation_uses_the_saved_global_index() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_global_operation_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait A$api$pi {}

        pub struct Service;

        impl Api for Service {}
        "#,
    )
    .await;

    fixture
        .check(
            &[LspQuery::goto_implementation(
                "implementations from saved semantics",
                "api",
            )],
            expect![[r#"
                implementations from saved semantics
                - /src/lib.rs:4:0-4:23
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
