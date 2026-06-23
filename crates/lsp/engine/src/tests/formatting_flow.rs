use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn formats_open_saved_document() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_formatting_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn demo(){println!("hi");}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .check_formatting(
            "format saved document",
            "src/lib.rs",
            expect![[r#"
                format saved document
                - /src/lib.rs:0:13-0:14 -> ""
                - /src/lib.rs:0:14-0:14 -> " {\n    "
                - /src/lib.rs:0:29-0:29 -> "\n"
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn unchanged_formatting_returns_no_edits() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_formatting_unchanged"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn demo() {
            println!("hi");
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .check_formatting(
            "format unchanged document",
            "src/lib.rs",
            expect![[r#"
                format unchanged document
                - no edits
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn unopened_document_returns_no_formatting_response() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_formatting_unopened"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn demo(){println!("hi");}
        "#,
    )
    .await;

    fixture
        .check_formatting(
            "format unopened document",
            "src/lib.rs",
            expect![[r#"
                format unopened document
                - no response
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn formatting_uses_dirty_live_text() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_formatting_dirty"
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
            MarkedText::parse(r#"pub fn dirty(){println!("dirty");}"#),
        )
        .await;

    fixture
        .check_formatting(
            "format dirty document",
            "src/lib.rs",
            expect![[r#"
                format dirty document
                - /src/lib.rs:0:14-0:15 -> ""
                - /src/lib.rs:0:15-0:15 -> " {\n    "
                - /src/lib.rs:0:33-0:34 -> ""
                - /src/lib.rs:0:34-0:34 -> "\n}\n"
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
