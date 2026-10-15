//! Saved-project behavior shared by cross-file LSP operations.

use expect_test::expect;

use super::utils::LspEngineFixture;

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
        pub trait A$0pi {} // goto_impl("implementations from saved semantics")

        pub struct Service;

        impl Api for Service {}
        "#,
    )
    .await;

    fixture
        .check(expect![[r#"
            /src/lib.rs:4:0-4:23 // implementations from saved semantics
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn goto_implementation_returns_trait_methods_across_saved_files() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_trait_method_implementations"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod account;
        mod user;
        pub trait Named { fn name(&self); }
        pub fn use_it(user: user::User) { user.na$0me(); } // goto_impl("all methods from call")

        //- /src/user.rs
        pub struct User;
        impl crate::Named for User { fn na$0me(&self) {} } // goto_impl("all methods from impl declaration")

        //- /src/account.rs
        pub struct Account;
        impl crate::Named for Account { fn name(&self) {} }
        "#,
    )
    .await;

    fixture
        .check(expect![[r#"
            // all methods from call
            /src/account.rs:1:35-1:39
            /src/user.rs:1:32-1:36

            // all methods from impl declaration
            /src/account.rs:1:35-1:39
            /src/user.rs:1:32-1:36
        "#]])
        .await;

    fixture.shutdown().await;
}
