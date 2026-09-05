use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn formatting_uses_saved_unchanged_and_dirty_document_text() {
    // Keep the already-formatted source last so a fixture boundary does not add a blank line that
    // rustfmt would legitimately remove.
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_formatting_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod changed;
        mod unchanged;
        mod dirty;

        //- /src/changed.rs
        pub fn demo(){println!("hi");}

        //- /src/dirty.rs
        pub fn saved() {}

        //- /src/unchanged.rs
        pub fn demo() {
            println!("hi");
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/changed.rs", 1).await;
    fixture.did_open_saved("src/unchanged.rs", 1).await;
    fixture.did_open_saved("src/dirty.rs", 1).await;
    fixture
        .did_change_full(
            "src/dirty.rs",
            2,
            MarkedText::parse(r#"pub fn dirty(){println!("dirty");}"#),
        )
        .await;

    fixture
        .check_formatting(
            "format saved document",
            "src/changed.rs",
            expect![[r#"
                format saved document
                - /src/changed.rs:0:13-0:14 -> ""
                - /src/changed.rs:0:14-0:14 -> " {\n    "
                - /src/changed.rs:0:29-0:30 -> ""
                - /src/changed.rs:1:0-1:0 -> }
            "#]],
        )
        .await;
    fixture
        .check_formatting(
            "format unchanged document",
            "src/unchanged.rs",
            expect![[r#"
                format unchanged document
                - no edits
            "#]],
        )
        .await;
    fixture
        .check_formatting(
            "format dirty document",
            "src/dirty.rs",
            expect![[r#"
                format dirty document
                - /src/dirty.rs:0:14-0:15 -> ""
                - /src/dirty.rs:0:15-0:15 -> " {\n    "
                - /src/dirty.rs:0:33-0:34 -> ""
                - /src/dirty.rs:0:34-0:34 -> "\n}\n"
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
