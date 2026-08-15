//! End-to-end rename behavior at the engine service boundary.

use expect_test::expect;
use rg_lsp_proto::AnalysisAbort;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

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
async fn rename_requires_save_before_using_changed_global_spans() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_rename_current_source"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod consumer;

        pub struct SavedUser;

        //- /src/consumer.rs
        use crate::SavedUser;

        pub fn make(user: SavedUser) -> SavedUser {
            user
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture.did_open_saved("src/consumer.rs", 1).await;
    let declaration = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
mod consumer;

pub struct EditorUser;
"#,
            ),
        )
        .await;
    let consumer = fixture
        .did_change_full(
            "src/consumer.rs",
            2,
            MarkedText::parse(
                r#"
use crate::Editor$rename$User;

pub fn make(user: EditorUser) -> EditorUser {
    user
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty_global_operations_require_save(&consumer, "rename", "Account")
        .await;

    fixture.did_save_dirty(&declaration).await;
    fixture.did_save_dirty(&consumer).await;
    fixture
        .check_rename_after_save(
            &consumer,
            "rename after publishing both files",
            "rename",
            "Account",
            expect![[r#"
                rename after publishing both files
                - /src/consumer.rs
                  - 1:11-1:21 -> Account
                  - 3:18-3:28 -> Account
                  - 3:33-3:43 -> Account
                - /src/lib.rs
                  - 3:11-3:21 -> Account
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn record_constructor_references_and_rename_flow_through_enabled_test_module() {
    let fixture = LspEngineFixture::initialized_with_cfg_test(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_record_constructor_test_cfg"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        #[cfg(test)]
        mod tests {
            pub struct Us$declaration$er<T> {
                pub value: T,
            }

            pub fn make(value: u8) {
                let _user = Us$constructor$er::<u8> { value };
            }
        }
        "#,
        true,
    )
    .await;

    fixture
        .check(
            &[
                LspQuery::references(
                    "record constructor references from declaration",
                    "declaration",
                    true,
                ),
                LspQuery::references(
                    "record constructor references from use without declaration",
                    "constructor",
                    false,
                ),
            ],
            expect![[r#"
                record constructor references from declaration
                - /src/lib.rs:2:15-2:19
                - /src/lib.rs:7:20-7:24

                record constructor references from use without declaration
                - /src/lib.rs:7:20-7:24
            "#]],
        )
        .await;

    fixture
        .check_rename(
            "rename record constructor from enabled test module",
            "constructor",
            "Account",
            expect![[r#"
                rename record constructor from enabled test module
                - /src/lib.rs
                  - 2:15-2:19 -> Account
                  - 7:20-7:24 -> Account
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn stale_source_query_aborts_then_recovers_through_the_command_queue() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_stale_source_retry"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod usage;

        pub struct Us$rename$er;

        //- /src/usage.rs
        use crate::User;

        pub fn first(_: User) {}
        "#,
    )
    .await;

    // Simulate a watcher delay for an unopened reference file. The target capture remains exact,
    // while the reference scan cannot reconstruct the other file at its saved
    // revision. That mismatch still aborts explicitly and queues ordinary source recovery.
    fixture.write_file_without_notification(
        "src/usage.rs",
        "use crate::User;\n\npub fn first(_: User) {}\npub fn second(_: User) {}\n",
    );
    fixture
        .check_rename_abort("rename", "Entity", AnalysisAbort::SourceChanged)
        .await;

    // The recovery command was enqueued before the abort was published, so this second
    // request runs after the normal path-change pipeline has published the new usage source.
    fixture
        .check_rename(
            "rename after queued source recovery",
            "rename",
            "Entity",
            expect![[r#"
                rename after queued source recovery
                - /src/lib.rs
                  - 2:11-2:15 -> Entity
                - /src/usage.rs
                  - 0:11-0:15 -> Entity
                  - 2:16-2:20 -> Entity
                  - 3:17-3:21 -> Entity
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
