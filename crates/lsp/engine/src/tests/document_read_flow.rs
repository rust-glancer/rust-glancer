use expect_test::expect;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn const_hover_uses_current_initializer_values() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_const_values"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub const CAPACITY: usize = 4;
        pub const ENABLED: bool = false;
        pub fn inspect() {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let current = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub const CAPA$module$CITY: usize = 2 * 8;
pub const ENA$preview$BLED: bool = true;
pub fn inspect() {
    struct Buffer;
    impl Buffer {
        pub const CAPACITY: usize = 1 << 5;
    }
    const LOCAL: usize = { 2 + 3 };
    const LABEL: &str = "a  b";
    let _ = LA$local_preview$BEL;
    let _ = LO$local$CAL;
    let _ = Buffer::CAPA$associated$CITY;
}
"#,
        )
        .await;

    fixture
        .check_dirty_queries(
            &current,
            &[
                LspQuery::hover("edited module const value", "module"),
                LspQuery::hover("new associated const value", "associated"),
                LspQuery::hover("new body-local const value", "local"),
                LspQuery::hover("edited const initializer preview", "preview"),
                LspQuery::hover("new body-local initializer preview", "local_preview"),
            ],
            expect![[r#"
                // edited module const value
                range: /src/lib.rs:1:10-1:18
                markdown:
                  ```rust
                  lsp_const_values::CAPACITY

                  pub const CAPACITY: usize = 2 * 8; // 16
                  ```

                // new associated const value
                range: /src/lib.rs:12:20-12:28
                markdown:
                  ```rust
                  pub const CAPACITY: usize = 1 << 5; // 32
                  ```

                // new body-local const value
                range: /src/lib.rs:11:12-11:17
                markdown:
                  ```rust
                  const LOCAL: usize = { 2 + 3 }; // 5
                  ```

                // edited const initializer preview
                range: /src/lib.rs:2:10-2:17
                markdown:
                  ```rust
                  lsp_const_values::ENABLED

                  pub const ENABLED: bool = true
                  ```

                // new body-local initializer preview
                range: /src/lib.rs:10:12-10:17
                markdown:
                  ```rust
                  const LABEL: &str = "a  b"
                  ```
            "#]],
        )
        .await;

    fixture.did_save_dirty(&current).await;
    fixture
        .check_dirty_queries(
            &current,
            &[
                LspQuery::hover("saved initializer value", "module"),
                LspQuery::hover("saved initializer preview", "preview"),
            ],
            expect![[r#"
                // saved initializer value
                range: /src/lib.rs:1:10-1:18
                markdown:
                  ```rust
                  lsp_const_values::CAPACITY

                  pub const CAPACITY: usize = 2 * 8; // 16
                  ```

                // saved initializer preview
                range: /src/lib.rs:2:10-2:17
                markdown:
                  ```rust
                  lsp_const_values::ENABLED

                  pub const ENABLED: bool = true
                  ```
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn document_reads_combine_current_locals_with_saved_global_semantics() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_current_document_reads"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Name;

        pub struct User {
            pub name: Name,
        }

        pub fn make_user() -> User {
            User { name: Name }
        }

        pub fn inspect() {}

        pub fn untouched() {
            let saved = make_user();
        }

        pub fn keep(user: &User, count: u32) {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let current = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub struct Name;

pub struct User {
    pub name: Name,
}

pub fn make_user() -> User {
    User { name: Name }
}

$range_start$pub fn inspect() {
    let user = make_user();
    let alias = user;
    let _hover = alias.na$hover$me;
    let _copy = ali$definition$as;
    let _highlight = al$highlight$ias;
    let callback = |closure_user| -> User { closure_user };
    keep(&user, 1);
}$range_end$

pub fn untouched(_changed: usize) {
    let unrelated = make_user();
}

pub fn keep(user: &User, count: u32) {}
"#,
        )
        .await;

    fixture
        .check_dirty_queries(
            &current,
            &[
                LspQuery::hover("hover through a current local", "hover"),
                LspQuery::goto_definition("navigate to a current local", "definition"),
                LspQuery::document_highlight("highlight a current local", "highlight"),
                LspQuery::inlay_hint(
                    "hints for the selected current body",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
            ],
            expect![[r#"
                // hover through a current local
                range: /src/lib.rs:14:23-14:27
                markdown:
                  ```rust
                  lsp_current_document_reads::User

                  pub name: Name
                  ```

                /src/lib.rs:13:8-13:13 // navigate to a current local

                // highlight a current local
                read 13:8-13:13
                read 14:17-14:22
                read 15:16-15:21
                read 16:21-16:26

                // hints for the selected current body
                `: User` type @ 12:12
                  tooltip:
                    ```rust
                    User
                    ```
                `: User` type @ 13:13
                  tooltip:
                    ```rust
                    User
                    ```
                `: Name` type @ 14:14
                  tooltip:
                    ```rust
                    Name
                    ```
                `: User` type @ 15:13
                  tooltip:
                    ```rust
                    User
                    ```
                `: User` type @ 16:18
                  tooltip:
                    ```rust
                    User
                    ```
                `: |User| -> User` type @ 17:16
                  tooltip:
                    ```rust
                    |User| -> User
                    ```
                `: User` type @ 17:32
                  tooltip:
                    ```rust
                    User
                    ```
                `count:` parameter @ 18:16
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn inlay_tooltips_show_concrete_binding_and_chain_types() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_inlay_tooltips"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod iter {
            // Declare the iterator language item locally so this fixture does not need rust-src.
            #[lang = "iterator"]
            pub trait Iterator { type Item; }
            pub struct Adapter<T>(pub T);
            impl<T> Iterator for Adapter<T> { type Item = T; }
            impl<T> Adapter<T> {
                pub fn step(self) -> Self { self }
            }
            pub fn adapter() -> Adapter<u8> { loop {} }
        }

        pub struct Wrapper<T>(pub T);

        $range_start$pub fn demo(wrapper: Wrapper<iter::Adapter<u8>>) {
            let iterator = iter::adapter();
            let chain = iter::adapter()
                .step()
                .step();
            let number = 1u32;
            let wrapped = wrapper;
        }$range_end$
        "#,
    )
    .await;

    fixture
        .check_queries(
            &[LspQuery::inlay_hint(
                "concrete type tooltips",
                "src/lib.rs",
                "range_start",
                "range_end",
            )],
            expect![[r#"
                // concrete type tooltips
                `: impl Iterator<Item = u8>` type @ 15:16
                  tooltip:
                    ```rust
                    Adapter<u8>
                    ```
                `: impl Iterator<Item = u8>` type @ 16:13
                  tooltip:
                    ```rust
                    Adapter<u8>
                    ```
                `impl Iterator<Item = u8>` type @ 17:15
                  tooltip:
                    ```rust
                    Adapter<u8>
                    ```
                `: u32` type @ 19:14
                  tooltip:
                    ```rust
                    u32
                    ```
                `: Wrapper<…>` type @ 20:15
                  tooltip:
                    ```rust
                    Wrapper<Adapter<u8>>
                    ```
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn inlay_hints_shorten_nested_types_with_configurable_budgets() {
    let source = r#"
        //- /Cargo.toml
        [package]
        name = "lsp_nested_type_hints"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod iter {
            #[lang = "iterator"]
            pub trait Iterator { type Item; }
            pub struct Adapter<T>(pub T);
            impl<T> Iterator for Adapter<T> { type Item = T; }
        }
        pub struct Wrapper<T>(pub T);
        pub struct Result<T, E>(pub T, pub E);
        pub struct Failure;
        pub struct ÉlementAvecUnNomLongMaisLisible;
        pub struct ÉlémentTrèsLisibleAvecUnNomAssezLong;
        pub struct ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint;

        $start$pub fn demo(
            nested: &Wrapper<Result<iter::Adapter<ÉlementAvecUnNomLongMaisLisible>, Failure>>,
            tuple: (Wrapper<ÉlementAvecUnNomLongMaisLisible>, Wrapper<ÉlementAvecUnNomLongMaisLisible>),
            unicode: Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>,
            long: ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint,
        ) {
            let nested = nested;
            let tuple = tuple;
            let unicode = unicode;
            let long = long;
        }$end$
    "#;

    // Zero disables structural shortening; 45 also checks a Unicode label exactly at the budget.
    // The final case uses the configured default.
    for (max_length, expected) in [
        (
            Some(0),
            expect![[r#"
            // nested types
            `: &Wrapper<Result<impl Iterator<Item = ÉlementAvecUnNomLongMaisLisible>, Failure>>` type @ 19:14
              tooltip:
                ```rust
                &Wrapper<Result<Adapter<ÉlementAvecUnNomLongMaisLisible>, Failure>>
                ```
            `: (Wrapper<ÉlementAvecUnNomLongMaisLisible>, Wrapper<ÉlementAvecUnNomLongMaisLisible>)` type @ 20:13
              tooltip:
                ```rust
                (Wrapper<ÉlementAvecUnNomLongMaisLisible>, Wrapper<ÉlementAvecUnNomLongMaisLisible>)
                ```
            `: Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>` type @ 21:15
              tooltip:
                ```rust
                Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>
                ```
            `: ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint` type @ 22:12
              tooltip:
                ```rust
                ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint
                ```
        "#]],
        ),
        (
            Some(45),
            expect![[r#"
            // nested types
            `: &Wrapper<Result<…>>` type @ 19:14
              tooltip:
                ```rust
                &Wrapper<Result<Adapter<ÉlementAvecUnNomLongMaisLisible>, Failure>>
                ```
            `: (Wrapper<…>, Wrapper<…>)` type @ 20:13
              tooltip:
                ```rust
                (Wrapper<ÉlementAvecUnNomLongMaisLisible>, Wrapper<ÉlementAvecUnNomLongMaisLisible>)
                ```
            `: Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>` type @ 21:15
              tooltip:
                ```rust
                Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>
                ```
            `: ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint` type @ 22:12
              tooltip:
                ```rust
                ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint
                ```
        "#]],
        ),
        (
            None,
            expect![[r#"
            // nested types
            `: &Wrapper<Result<…>>` type @ 19:14
              tooltip:
                ```rust
                &Wrapper<Result<Adapter<ÉlementAvecUnNomLongMaisLisible>, Failure>>
                ```
            `: (Wrapper<…>, Wrapper<…>)` type @ 20:13
              tooltip:
                ```rust
                (Wrapper<ÉlementAvecUnNomLongMaisLisible>, Wrapper<ÉlementAvecUnNomLongMaisLisible>)
                ```
            `: Wrapper<…>` type @ 21:15
              tooltip:
                ```rust
                Wrapper<ÉlémentTrèsLisibleAvecUnNomAssezLong>
                ```
            `: ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint` type @ 22:12
              tooltip:
                ```rust
                ATypeNameThatIsLongerThanTheDefaultBudgetAndMustRemainWholeInEveryHint
                ```
        "#]],
        ),
    ] {
        let mut config = LspEngineFixture::engine_config();
        if let Some(max_length) = max_length {
            config.inlay_hints.max_length = max_length;
        }
        let fixture = LspEngineFixture::initialized_with_engine_config(source, config).await;
        fixture
            .check_queries(
                &[LspQuery::inlay_hint(
                    "nested types",
                    "src/lib.rs",
                    "start",
                    "end",
                )],
                expected,
            )
            .await;
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn document_highlights_saved_items_when_editor_source_is_exact() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_exact_document_highlights"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait Serv$0ice {} // highlight("highlight an exact trait item")

        pub fn insp$0ect() {} // highlight("highlight an exact function item")

        pub fn use_items<T: Service>() {
            inspect();
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture
        .check(expect![[r#"
            // highlight an exact trait item
            read 0:10-0:17
            read 4:20-4:27

            // highlight an exact function item
            read 2:7-2:14
            read 5:4-5:11
        "#]])
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn inlay_hints_skip_bodyless_declarations_and_leave_engine_available() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_bodyless_inlay"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub trait Service {
            fn required(&self);
        }

        pub struct Val$hover$ue;

        $range_start$pub fn inspect() {
            let value = Value;
        }$range_end$
        "#,
    )
    .await;

    fixture
        .check_queries(
            &[
                LspQuery::inlay_hint(
                    "saved inlay hints skip a trait method",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
                LspQuery::hover("engine remains available after saved inlay hints", "hover"),
            ],
            expect![[r#"
                // saved inlay hints skip a trait method
                `: Value` type @ 7:13
                  tooltip:
                    ```rust
                    Value
                    ```

                // engine remains available after saved inlay hints
                range: /src/lib.rs:4:11-4:16
                markdown:
                  ```rust
                  lsp_bodyless_inlay::Value

                  pub struct Value
                  ```
            "#]],
        )
        .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
pub trait Service {
    fn required(&self);
}

pub struct Val$hover$ue;

$range_start$pub fn inspect() {
    let value = Value;
}$range_end$

fn unfinished
"#,
        )
        .await;

    fixture
        .check_dirty_queries(
            &dirty,
            &[
                LspQuery::inlay_hint(
                    "dirty inlay hints skip bodyless and unfinished items",
                    "src/lib.rs",
                    "range_start",
                    "range_end",
                ),
                LspQuery::hover("engine remains available after dirty inlay hints", "hover"),
            ],
            expect![[r#"
                // dirty inlay hints skip bodyless and unfinished items
                `: Value` type @ 8:13
                  tooltip:
                    ```rust
                    Value
                    ```

                // engine remains available after dirty inlay hints
                range: /src/lib.rs:5:11-5:16
                markdown:
                  ```rust
                  lsp_bodyless_inlay::Value

                  pub struct Value
                  ```
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
