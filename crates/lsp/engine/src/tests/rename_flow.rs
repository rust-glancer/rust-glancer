//! End-to-end rename behavior at the engine service boundary.

use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn rename_returns_workspace_edit_for_clean_document() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_rename_clean_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct User;

        pub fn demo() {
            let _user: Us$rename$er;
        }
        "#,
    )
    .await;

    fixture
        .check_rename(
            "rename clean type",
            "rename",
            "Account",
            expect![[r#"
                rename clean type
                - /src/lib.rs
                  - 0:11-0:15 -> Account
                  - 3:15-3:19 -> Account
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn rename_rejects_when_other_documents_are_dirty() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_rename_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod other;

        pub struct User;

        pub fn demo() {
            let _user: Us$rename$er;
        }

        //- /src/other.rs
        pub fn helper() {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture.did_open_saved("src/other.rs", 1).await;
    fixture
        .did_change_full(
            "src/other.rs",
            2,
            MarkedText::parse(
                r#"
pub fn helper() {
    let _dirty = 1;
}
"#,
            ),
        )
        .await;

    fixture
        .check_rename_error(
            "rename with dirty sibling document",
            "rename",
            "Account",
            expect![[r#"
                rename with dirty sibling document
                - rename requires saving other dirty Rust documents first
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn stale_source_query_returns_neutral_then_recovers_through_the_command_queue() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_stale_source_retry"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct User;

        pub fn demo() {
            let _user: Us$rename$er;
        }
        "#,
    )
    .await;

    // Simulate a watcher delay: the disk advances, but the saved project still describes `User`.
    // Rename needs exact source text, so its first attempt detects the stale revision and returns
    // no edit. Recovery is queued behind that query instead of rebuilding on its call stack.
    fixture.write_file_without_notification(
        "src/lib.rs",
        "pub struct Acct;\n\npub fn demo() {\n    let _user: Acct;\n}\n",
    );
    fixture
        .check_rename(
            "rename that discovers an unnotified source change",
            "rename",
            "Entity",
            expect![[r#"
                rename that discovers an unnotified source change
                - none
            "#]],
        )
        .await;

    // The recovery command was enqueued before the neutral response was published, so this second
    // request runs after the normal path-change pipeline has published the `Acct` generation.
    fixture
        .check_rename(
            "rename after queued source recovery",
            "rename",
            "Entity",
            expect![[r#"
                rename after queued source recovery
                - /src/lib.rs
                  - 0:11-0:15 -> Entity
                  - 3:15-3:19 -> Entity
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
