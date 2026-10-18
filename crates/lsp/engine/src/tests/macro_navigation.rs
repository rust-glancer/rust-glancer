use expect_test::expect;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn macro_argument_navigation_tracks_unsaved_body_positions() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_macro_navigation"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct User;
        impl User {
            /// Account identifier.
            pub fn id(&self) -> u32 { 1 }
        }
        macro_rules! inner { ($value:expr) => { $value }; }
        macro_rules! outer { ($value:expr) => { inner!($value) }; }
        pub fn demo(user: User) {
            let _ = outer!(user.i$0d()); // hover("saved nested method")
            let _ = outer!(user.i$0d()); // goto("saved nested method definition")
        }
        "#,
    )
    .await;
    fixture
        .check(expect![[r#"
        // saved nested method
        range: /src/lib.rs:8:24-8:26
        markdown:
          ```rust
          lsp_macro_navigation::User::id

          pub fn id(&self) -> u32
          ```

          Account identifier.

        /src/lib.rs:3:11-3:13 // saved nested method definition
    "#]])
        .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub struct User;
impl User {
    /// Account identifier.
    pub fn id(&self) -> u32 { 1 }
}
macro_rules! inner { ($value:expr) => { $value }; }
macro_rules! outer { ($value:expr) => { inner!($value) }; }
pub fn demo(user: User) {
    let extra = 1;
    let current = user;
    let _ = outer!(current.i$0d()); // hover("dirty nested method")
    let _ = outer!(current.i$0d()); // goto("dirty nested method definition")
    let _ = outer!(curr$0ent); // goto("dirty macro local definition")
}
"#,
        )
        .await;
    fixture
        .check_dirty(
            &dirty,
            expect![[r#"
        // dirty nested method
        range: /src/lib.rs:11:27-11:29
        markdown:
          ```rust
          lsp_macro_navigation::User::id

          pub fn id(&self) -> u32
          ```

          Account identifier.

        /src/lib.rs:4:11-4:13 // dirty nested method definition
        /src/lib.rs:10:8-10:15 // dirty macro local definition
    "#]],
        )
        .await;
    fixture.shutdown().await;
}
