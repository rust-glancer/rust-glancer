use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn shared_document_queries_use_one_target_for_saved_and_current_source() {
    let fixture = LspEngineFixture::initialized(
        r#"
//- /Cargo.toml
[package]
name = "selected_target"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod shared;
pub struct Library { pub only_library: u8 }
pub fn make() -> Library { Library { only_library: 1 } }

//- /src/main.rs
mod shared;
pub struct Binary { pub only_binary: bool }
pub fn make() -> Binary { Binary { only_binary: true } }
fn main() {}

//- /src/shared.rs
$start$pub fn inspect() {
    let value = crate::ma$factory$ke();
    let _ = val$value$ue;
    let _ = value.only_$member$;
}$end$
"#,
    )
    .await;
    let queries = [
        LspQuery::inlay_hint("selected target hints", "src/shared.rs", "start", "end"),
        LspQuery::goto_type_definition("selected target type", "value"),
        LspQuery::goto_definition("selected target definition", "factory"),
        LspQuery::completion("selected target fields", "member"),
        LspQuery::references("selected target references", "factory", true),
    ];
    fixture
        .check_queries(
            &queries,
            expect![[r#"
        // selected target hints
        `: Library` type @ 1:13
          tooltip:
            ```rust
            Library
            ```

        /src/lib.rs:1:11-1:18 // selected target type
        /src/lib.rs:2:7-2:11 // selected target definition

        // selected target fields
        only_library Field
          detail: pub only_library: u8
          edit: /src/shared.rs:3:18-3:23 -> only_library

        // selected target references
        /src/lib.rs:2:7-2:11
        /src/shared.rs:1:23-1:27
    "#]],
        )
        .await;

    fixture
        .check_rename(
            "rename the selected factory",
            "factory",
            "create",
            expect![[r#"
                // rename the selected factory
                /src/lib.rs
                  2:7-2:11 -> create
                /src/shared.rs
                  1:23-1:27 -> create
            "#]],
        )
        .await;

    // Moving the body forces current-source preparation without changing its selected owner.
    let current = fixture
        .did_open_dirty(
            "src/shared.rs",
            1,
            r#"
// Moved in the editor.
$start$pub fn inspect() {
    let value = crate::ma$factory$ke();
    let _ = val$value$ue;
    let _ = value.only_$member$;
}$end$
"#,
        )
        .await;
    fixture
        .check_dirty_queries(
            &current,
            &queries[..4],
            expect![[r#"
            // selected target hints
            `: Library` type @ 3:13
              tooltip:
                ```rust
                Library
                ```

            /src/lib.rs:1:11-1:18 // selected target type
            /src/lib.rs:2:7-2:11 // selected target definition

            // selected target fields
            only_library Field
              detail: pub only_library: u8
              edit: /src/shared.rs:5:18-5:23 -> only_library
        "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn selecting_one_origin_preserves_cross_crate_references_and_unique_edits() {
    let fixture = LspEngineFixture::initialized(
        r#"
//- /Cargo.toml
[workspace]
members = ["api", "app"]
resolver = "3"

//- /api/Cargo.toml
[package]
name = "api"
version = "0.1.0"
edition = "2024"

//- /api/src/lib.rs
pub struct Us$subject$er;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"
[dependencies]
api = { path = "../api" }

//- /app/src/lib.rs
mod shared;
pub fn library(_: api::User) {}

//- /app/src/main.rs
mod shared;
fn main() { let _ = api::User; }

//- /app/src/shared.rs
pub fn inspect(_: api::User) {}
"#,
    )
    .await;
    fixture
        .check_queries(
            &[LspQuery::references(
                "all dependent targets",
                "subject",
                true,
            )],
            expect![[r#"
                // all dependent targets
                /api/src/lib.rs:0:11-0:15
                /app/src/lib.rs:1:23-1:27
                /app/src/shared.rs:0:23-0:27
                /app/src/main.rs:1:25-1:29
            "#]],
        )
        .await;
    fixture
        .check_rename(
            "one edit per physical occurrence",
            "subject",
            "Account",
            expect![[r#"
                // one edit per physical occurrence
                /api/src/lib.rs
                  0:11-0:15 -> Account
                /app/src/lib.rs
                  1:23-1:27 -> Account
                /app/src/main.rs
                  1:25-1:29 -> Account
                /app/src/shared.rs
                  0:23-0:27 -> Account
            "#]],
        )
        .await;
    fixture.shutdown().await;
}
