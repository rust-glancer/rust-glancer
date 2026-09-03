use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn returns_typed_comments_without_losing_other_folds() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_folding_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        use std::fmt;
        use std::io;

        // first
        // second

        fn demo() {
            value;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .check_folding(
            "fold saved document",
            "src/lib.rs",
            true,
            expect![[r#"
                fold saved document
                - imports 0:*-1:*
                - comment 3:*-4:*
                - code 6:*-8:*
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn folding_uses_dirty_live_text() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_folding_dirty"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn saved() {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse("// dirty one\n// dirty two\npub fn saved() {}\n"),
        )
        .await;
    fixture
        .check_folding(
            "fold dirty document",
            "src/lib.rs",
            true,
            expect![[r#"
                fold dirty document
                - comment 0:*-1:*
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
