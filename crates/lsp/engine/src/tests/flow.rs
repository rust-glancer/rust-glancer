use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn goto_definition_on_modules_selects_their_definition_source() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_module_definitions"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod fla$0t; // goto("flat module declaration")
        pub mod dir$0ectory; // goto("directory module declaration")
        #[path = "alternate.rs"]
        pub mod cus$0tom; // goto("path attribute module declaration")
        pub mod inl$0ine { // goto("inline module declaration")
            pub mod nes$0ted; // goto("nested module in an inline module")
        }

        use fla$0t::Item; // goto("out-of-line module reference")
        use inl$0ine::nested::Item as Nested; // goto("inline module reference")
        use cra$0te::custom::Item as Custom; // goto("crate root reference")

        //- /src/flat.rs
        //! Module contents.
        pub struct Item;

        //- /src/directory/mod.rs
        pub mod chi$0ld; // goto("nested module in another file")

        //- /src/directory/child.rs
        pub struct Item;

        //- /src/alternate.rs
        pub struct Item;

        //- /src/inline/nested.rs
        pub struct Item;
        "#,
    )
    .await;

    fixture
        .check(expect![[r#"
            /src/flat.rs:0:0-0:0 // flat module declaration
            /src/directory/mod.rs:0:0-0:0 // directory module declaration
            /src/alternate.rs:0:0-0:0 // path attribute module declaration
            /src/lib.rs:4:8-4:14 // inline module declaration
            /src/inline/nested.rs:0:0-0:0 // nested module in an inline module
            /src/flat.rs:0:0-0:0 // out-of-line module reference
            /src/lib.rs:4:8-4:14 // inline module reference
            /src/lib.rs:0:0-0:0 // crate root reference
            /src/directory/child.rs:0:0-0:0 // nested module in another file
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn returns_protocol_edits_for_specialized_and_postfix_completions() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_specialized_completions"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        extern "C-un$0" fn foreign(); // complete("complete extern ABI")

        macro_rules! capture {
            ($value: ex$0) => { $value }; // complete("complete macro fragment")
        }

        pub fn demo(local_capture: usize, condition: bool) {
            let _ = format!("{loc$0}"); // complete("complete format capture")
            let _ = env!("CARGO_MAN$0"); // complete("complete Cargo environment")
            let _ = (local_capture + 1).ma$0; // complete("complete postfix match")
            let _ = condition.i$0; // complete("complete boolean postfix")
        }
        "#,
    )
    .await;

    fixture
        .check(expect![[r#"
            // complete extern ABI
            C-unwind Value
              detail: extern ABI C-unwind
              edit: /src/lib.rs:0:8-0:12 -> C-unwind

            // complete macro fragment
            expr Value
              detail: macro fragment expr
              edit: /src/lib.rs:3:13-3:15 -> expr
            expr_2021 Value
              detail: macro fragment expr_2021
              edit: /src/lib.rs:3:13-3:15 -> expr_2021

            // complete format capture
            local_capture Variable
              detail: format capture local_capture
              edit: /src/lib.rs:7:22-7:25 -> local_capture

            // complete Cargo environment
            CARGO_MANIFEST_DIR Value
              detail: directory containing this package manifest
              edit: /src/lib.rs:8:18-8:27 -> CARGO_MANIFEST_DIR
            CARGO_MANIFEST_PATH Value
              detail: path to this package manifest
              edit: /src/lib.rs:8:18-8:27 -> CARGO_MANIFEST_PATH

            // complete postfix match
            match Snippet
              detail: postfix match expr
              edit: /src/lib.rs:9:12-9:34 -> "match (local_capture + 1) {\n    _ => {},\n}"

            // complete boolean postfix
            if Snippet
              detail: postfix if expr
              edit: /src/lib.rs:10:12-10:23 -> "if condition {\n}"
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn returns_versioned_code_action_edits_for_exact_imports() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [workspace]
        members = ["catalog", "app"]
        resolver = "3"

        //- /catalog/Cargo.toml
        [package]
        name = "catalog"
        version = "0.1.0"
        edition = "2024"

        //- /catalog/src/lib.rs
        pub mod collections {
            pub struct BTreeMap;
            pub struct HashMap;
        }

        //- /app/Cargo.toml
        [package]
        name = "app"
        version = "0.1.0"
        edition = "2024"

        [dependencies]
        catalog = { path = "../catalog" }

        //- /app/src/lib.rs
        use catalog::collections::BTreeMap;

        pub fn demo() {
            let _: HashMap$0; // code_actions("import exact unresolved item")
        }
        "#,
    )
    .await;
    fixture.did_open_saved("app/src/lib.rs", 7).await;

    fixture
        .check(expect![[r#"
            // import exact unresolved item
            quickfix Import `catalog::collections::HashMap`
              preferred: true
              document: /app/src/lib.rs version 7
              edit: 0:4-0:34 -> catalog::collections::{BTreeMap, HashMap}
              result:
                use catalog::collections::{BTreeMap, HashMap};

                pub fn demo() {
                    let _: HashMap;
                }
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn answers_lsp_queries_from_saved_project() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_saved_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        /// Display name shown in the UI.
        pub struct Name;

        /// User account stored by the service.
        pub struct User {
            /// The user's display name.
            pub name: Name,
        }

        /// Builds a user.
        pub fn make_user() -> User {
            User { name: Name }
        }

        pub fn demo() {
            let user = make_u$goto$ser();
            let _hover = make_$hover$user();
            let _name = user.na$complete$;
        }

        pub mod keyword_site {
            f$saved_keyword$
        }
        "#,
    )
    .await;

    fixture
        .check_queries(
            &[
                LspQuery::goto_definition("goto function", "goto"),
                LspQuery::hover("hover function", "hover"),
                LspQuery::completion("complete field", "complete"),
                LspQuery::completion("complete saved item keyword", "saved_keyword"),
                LspQuery::document_symbol("document symbols", "src/lib.rs"),
            ],
            expect![[r#"
                /src/lib.rs:10:7-10:16 // goto function

                // hover function
                range: /src/lib.rs:16:17-16:26
                markdown:
                  ```rust
                  lsp_saved_flow::make_user

                  pub fn make_user() -> User
                  ```

                  Builds a user.

                // complete field
                name Field
                  detail: pub name: Name
                  edit: /src/lib.rs:17:21-17:23 -> name

                // complete saved item keyword
                fn Keyword
                  detail: keyword fn
                  edit: /src/lib.rs:21:4-21:5 -> fn

                // document symbols
                Struct Name 1:11-1:15
                Struct User 4:11-4:15
                  Field name 6:8-6:12
                Function make_user 10:7-10:16
                Function demo 14:7-14:11
                Module keyword_site 20:8-20:20
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn returns_protocol_edits_for_item_scope_completions() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_item_scope_completions"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait Service<T> {
            type Output;
            fn required(&self, value: T) -> Self::Output;
        }

        pub struct Worker;

        impl Service<u8> for Worker {
            fn req$0 // complete("complete missing trait member")
        }

        macro_rules! local_item {
            () => { struct Generated; };
        }

        local_i$0!(); // complete("complete module macro")
        mod pars$0; // complete("complete module declaration")

        //- /src/parser.rs
        pub struct Parser;
        "#,
    )
    .await;

    fixture
        .check(
            expect![[r#"
                // complete missing trait member
                required Function
                  detail: required trait member: fn required(&self, value: u8) -> Self::Output
                  filter: fn required
                  edit: /src/lib.rs:8:4-8:10 -> "fn required(&self, value: u8) -> Self::Output {\n    todo!()\n}"

                // complete module macro
                local_item Function
                  detail: macro local_item
                  edit: /src/lib.rs:15:0-15:7 -> local_item

                // complete module declaration
                parser Module
                  detail: mod parser
                  edit: /src/lib.rs:16:4-16:8 -> parser
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
