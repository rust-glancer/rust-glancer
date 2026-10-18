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

#[tokio::test]
async fn builtin_macro_navigation_tracks_unsaved_arguments() {
    let source = r#"
#[rustc_builtin_macro]
macro_rules! format_args { ($($args:tt)*) => {}; }
#[rustc_builtin_macro]
macro_rules! assert { ($($args:tt)*) => {}; }
macro_rules! format { ($($args:tt)*) => { format_args!($($args)*) }; }
pub struct User;
impl User {
    pub fn id(&self) -> u32 { 1 }
    pub fn valid(&self) -> bool { true }
}
pub fn demo(user: User) {
    let _ = format!("{}", user.i$0d()); // hover("saved format method")
    let _ = format!("{}", user.i$0d()); // goto("saved format method definition")
    assert!(user.va$0lid(), "{}", user.id()); // goto("saved assert condition")
    let _ = format!("{:?}", us$0er); // goto("saved format local")
}
"#;
    let fixture = LspEngineFixture::initialized(&format!(
        r#"
//- /Cargo.toml
[package]
name = "lsp_builtin_navigation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
{source}
"#,
    ))
    .await;
    fixture
        .check(expect![[r#"
        // saved format method
        range: /src/lib.rs:12:31-12:33
        markdown:
          ```rust
          lsp_builtin_navigation::User::id

          pub fn id(&self) -> u32
          ```

        /src/lib.rs:8:11-8:13 // saved format method definition
        /src/lib.rs:9:11-9:16 // saved assert condition
        /src/lib.rs:11:12-11:16 // saved format local
    "#]])
        .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let current = source
        .replace(
            "pub fn demo(user: User) {",
            "pub fn demo(user: User) {\n    let extra = 1;\n    let current = user;",
        )
        .replace("user.", "current.")
        .replace("us$0er", "curr$0ent")
        .replace("saved ", "dirty ");
    let dirty = fixture.did_change_full("src/lib.rs", 2, &current).await;
    fixture
        .check_dirty(
            &dirty,
            expect![[r#"
        // dirty format method
        range: /src/lib.rs:14:34-14:36
        markdown:
          ```rust
          lsp_builtin_navigation::User::id

          pub fn id(&self) -> u32
          ```

        /src/lib.rs:8:11-8:13 // dirty format method definition
        /src/lib.rs:9:11-9:16 // dirty assert condition
        /src/lib.rs:13:8-13:15 // dirty format local
    "#]],
        )
        .await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn format_capture_navigation_tracks_unsaved_strings() {
    let fixture = LspEngineFixture::initialized_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "lsp_format_captures"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn demo(user: &str) {
    let _ = format!("🦀 {us$0e$1r}"); // hover("saved capture"), goto("saved capture definition")
}
"#,
    )
    .await;
    fixture
        .check(expect![[r#"
        // saved capture
        range: /src/lib.rs:1:25-1:29
        markdown:
          ```rust
          let user: &str
          ```

        /src/lib.rs:0:12-0:16 // saved capture definition
    "#]])
        .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub fn demo(user: &str) {
    let current = 1u64;
    let _ = format!("🦀 {curr$0e$1nt}"); // hover("dirty capture"), goto("dirty capture definition")
}
"#,
        )
        .await;
    fixture
        .check_dirty(
            &dirty,
            expect![[r#"
        // dirty capture
        range: /src/lib.rs:3:25-3:32
        markdown:
          ```rust
          let current: u64
          ```

        /src/lib.rs:2:8-2:15 // dirty capture definition
    "#]],
        )
        .await;
    fixture.shutdown().await;
}
