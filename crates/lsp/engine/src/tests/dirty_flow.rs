use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn unsaved_global_declarations_are_syntax_only_until_save() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct SavedUser {
            pub saved_field: SavedName,
        }

        pub struct SavedName;

        pub fn demo(user: SavedUser) {
            let _ = user.saved_field;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub struct DirtyUser {
    /// Field that exists only in the unsaved buffer.
    pub dirty_field: DirtyName,
}

pub struct DirtyName;

pub fn demo(user: DirtyUser) {
    let _completion = user.dirty_$complete$;
    let _hover = user.dirty_$hover$field;
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete dirty field", "complete"),
                LspQuery::hover("hover dirty field", "hover"),
                LspQuery::document_symbol("dirty document symbols", "src/lib.rs"),
            ],
            expect![[r#"
                complete dirty field
                - none

                hover dirty field
                - none

                dirty document symbols
                - Struct DirtyUser 1:11-1:20
                  - Field dirty_field 3:8-3:19
                - Struct DirtyName 6:11-6:20
                - Function demo 8:7-8:11
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_imports_and_unchanged_item_headers_use_saved_semantics() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_item_surface"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod api {
            pub struct Only;
        }

        pub struct Foo;

        pub struct Wrapper {
            pub value: Foo,
        }

        impl Foo {
            pub fn new() {
                let saved = true;
            }
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
use ap$root_import$;
use crate::api::On$qualified_import$;

pub mod api {
    pub struct Only;
}

pub struct Foo;

pub struct Wrapper {
    pub value: F$type$oo,
}

impl F$impl$oo {
    pub fn n$method$ew() {
        let current = true;
    }
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete newly typed import root", "root_import"),
                LspQuery::completion("complete newly typed qualified import", "qualified_import"),
                LspQuery::hover("hover moved impl type", "impl"),
                LspQuery::hover("hover method with a dirty body", "method"),
                LspQuery::goto_definition("define moved impl type", "impl"),
                LspQuery::goto_type_definition("type of moved field", "type"),
                LspQuery::document_highlight("highlight moved method", "method"),
                LspQuery::document_highlight("highlight moved field type", "type"),
            ],
            expect![[r#"
                complete newly typed import root
                - Foo Struct
                  detail: struct Foo
                  edit: /src/lib.rs:1:4-1:6 -> Foo
                - Wrapper Struct
                  detail: struct Wrapper
                  edit: /src/lib.rs:1:4-1:6 -> Wrapper
                - api Module
                  detail: mod api
                  edit: /src/lib.rs:1:4-1:6 -> api

                complete newly typed qualified import
                - Only Struct
                  detail: struct Only
                  edit: /src/lib.rs:2:16-2:18 -> Only

                hover moved impl type
                - range: /src/lib.rs:14:5-14:8
                - markdown:
                  ```rust
                  lsp_dirty_item_surface::Foo
                  ```

                  ```rust
                  pub struct Foo
                  ```

                hover method with a dirty body
                - range: /src/lib.rs:15:11-15:14
                - markdown:
                  ```rust
                  lsp_dirty_item_surface::Foo::new
                  ```

                  ```rust
                  pub fn new()
                  ```

                define moved impl type
                - /src/lib.rs:8:11-8:14

                type of moved field
                - /src/lib.rs:8:11-8:14

                highlight moved method
                - read 15:11-15:14

                highlight moved field type
                - read 11:15-11:18
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_module_use_paths_resolve_in_saved_module_scope() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_use_paths"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod api {
            pub mod nested {
                pub struct User;
                pub struct Account;
            }
        }

        pub mod exports {
            pub use crate::api::{nested::User, nested::Account as PublicAccount};
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
// An unrelated edit moves every saved import span.
pub mod api {
    pub mod nested {
        pub struct User;
        pub struct Account;
    }
}

pub mod exports {
    pub use crate::api::{nested::Us$user_hover$er, nested::Account as Public$alias_definition$Account};
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::hover("hover a moved re-export path", "user_hover"),
                LspQuery::goto_definition("define a moved re-export alias", "alias_definition"),
            ],
            expect![[r#"
                hover a moved re-export path
                - range: /src/lib.rs:10:33-10:37
                - markdown:
                  ```rust
                  lsp_dirty_use_paths::api::nested::User
                  ```

                  ```rust
                  pub struct User
                  ```

                define a moved re-export alias
                - /src/lib.rs:5:19-5:26
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_associated_header_type_definition_lowers_transparent_aliases() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_type_alias"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Wrapper<T>(pub T);
        pub struct User;
        pub type Alias<T> = Wrapper<T>;

        pub fn inspect(_: Alias<User>) {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
// Move the unchanged signature away from every saved offset.
pub struct Wrapper<T>(pub T);
pub struct User;
pub type Alias<T> = Wrapper<T>;

pub fn inspect(_: Ali$alias_type$as<User>) {}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::goto_type_definition(
                "type of an alias in a moved header",
                "alias_type",
            )],
            expect![[r#"
                type of an alias in a moved header
                - /src/lib.rs:2:11-2:18
                - /src/lib.rs:3:11-3:15
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn completion_ignores_unsaved_changes_to_a_sibling_declaration() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_current_source_cross_file"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod consumer;

        pub struct SavedUser {
            pub saved_field: usize,
        }

        //- /src/consumer.rs
        use crate::SavedUser;

        pub fn inspect(user: SavedUser) {
            let _ = user.saved_field;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture.did_open_saved("src/consumer.rs", 1).await;
    fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
mod consumer;

pub struct SavedUser {
    pub saved_field: usize,
    /// Field that exists only in the unsaved sibling.
    pub editor_field: usize,
}
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
use crate::SavedUser;

pub fn inspect(user: SavedUser) {
    let _ = user.editor_$complete$;
    let _: Saved$definition$User = user;
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &consumer,
            &[
                LspQuery::completion("complete from saved sibling semantics", "complete"),
                LspQuery::goto_definition("navigate to the saved declaration", "definition"),
            ],
            expect![[r#"
                complete from saved sibling semantics
                - saved_field Field
                  detail: pub saved_field: usize
                  edit: /src/consumer.rs:4:17-4:24 -> saved_field

                navigate to the saved declaration
                - /src/lib.rs:3:11-3:20
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn definition_maps_a_saved_target_into_its_dirty_open_document() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_definition_destination"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod consumer;
        mod unrelated;

        pub struct SavedUser;

        //- /src/consumer.rs
        use crate::SavedUser;

        pub fn inspect(_: Saved$definition$User) {}

        //- /src/unrelated.rs
        pub struct Unrelated;
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    fixture.did_open_saved("src/consumer.rs", 1).await;
    fixture.did_open_saved("src/unrelated.rs", 1).await;
    fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
// Unsaved lines move the declaration away from its saved byte range.
// Navigation must use the captured destination instead.
mod consumer;
mod unrelated;

pub struct SavedUser;
"#,
            ),
        )
        .await;
    fixture
        .did_change_full(
            "src/unrelated.rs",
            2,
            MarkedText::parse("pub struct DifferentUnrelated;\n"),
        )
        .await;

    fixture
        .check(
            &[LspQuery::goto_definition(
                "map a saved target into current destination text",
                "definition",
            )],
            expect![[r#"
                map a saved target into current destination text
                - /src/lib.rs:6:11-6:20
            "#]],
        )
        .await;

    // Once the destination declaration itself changes, there is no structural proof for the
    // saved range. Omitting it is safer than navigating to whatever occupies the old coordinates.
    fixture
        .did_change_full(
            "src/lib.rs",
            3,
            MarkedText::parse(
                r#"
mod consumer;
mod unrelated;

pub struct CurrentUser;
"#,
            ),
        )
        .await;
    fixture
        .check(
            &[LspQuery::goto_definition(
                "omit an unprovable dirty destination",
                "definition",
            )],
            expect![[r#"
                omit an unprovable dirty destination
                - none
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn definition_does_not_treat_an_overlapping_saved_range_as_current() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_navigation_source_identity"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct SavedTarget;

        pub fn inspect() {
            let _: SavedTarget;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub fn inspect() {
    let _: Saved$definition$Target = todo!();
}
"#,
            ),
        )
        .await;

    // The saved declaration's old numeric range now falls inside the current function body. It
    // is still a saved declaration, and there is no current declaration to map it to.
    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::goto_definition(
                    "omit a saved declaration removed from current text",
                    "definition",
                ),
                LspQuery::document_highlight("highlight only the current reference", "definition"),
            ],
            expect![[r#"
                omit a saved declaration removed from current text
                - none

                highlight only the current reference
                - read 2:11-2:22
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn completion_does_not_depend_on_an_unrelated_open_source() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_removed_open_source"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod entry;
        mod removed;

        pub struct User {
            pub field: usize,
        }

        //- /src/entry.rs
        use crate::User;

        pub fn inspect(user: User) {
            let _ = user.field;
        }

        //- /src/removed.rs
        pub struct Removed;
        "#,
    )
    .await;

    fixture.did_open_saved("src/removed.rs", 1).await;
    let entry = fixture
        .did_open_dirty(
            "src/entry.rs",
            1,
            MarkedText::parse(
                r#"
use crate::User;

pub fn inspect(user: User) {
    let _ = user.fi$complete$;
}
"#,
            ),
        )
        .await;
    fixture.remove_file_without_notification("src/removed.rs");

    fixture
        .check_dirty(
            &entry,
            &[LspQuery::completion(
                "completion survives a removed open sibling",
                "complete",
            )],
            expect![[r#"
                completion survives a removed open sibling
                - field Field
                  detail: pub field: usize
                  edit: /src/entry.rs:4:17-4:19 -> field
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_completion_preserves_specialized_and_postfix_edit_ranges() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_specialized_completions"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn demo(local_capture: usize, condition: bool) {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub fn demo(local_capture: usize, condition: bool) {
    let _ = format!("{loc$format$}");
    let _ = env!("CARGO_MAN$environment$");
    let _ = condition.i$postfix$;
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("complete dirty format capture", "format"),
                LspQuery::completion("complete dirty Cargo environment", "environment"),
                LspQuery::completion("complete dirty boolean postfix", "postfix"),
            ],
            expect![[r#"
                complete dirty format capture
                - local_capture Variable
                  detail: format capture local_capture
                  edit: /src/lib.rs:2:22-2:25 -> local_capture

                complete dirty Cargo environment
                - CARGO_MANIFEST_DIR Value
                  detail: directory containing this package manifest
                  edit: /src/lib.rs:3:18-3:27 -> CARGO_MANIFEST_DIR
                - CARGO_MANIFEST_PATH Value
                  detail: path to this package manifest
                  edit: /src/lib.rs:3:18-3:27 -> CARGO_MANIFEST_PATH

                complete dirty boolean postfix
                - if Snippet
                  detail: postfix if expr
                  edit: /src/lib.rs:4:12-4:23 -> "if condition {\n}"
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_completion_uses_saved_semantics_for_changed_incomplete_signature() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_signature_completion"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Saved;
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub struct DirtyFixture;
pub struct Wrapper<T>(T);

pub fn demo<T, const N: usize>(value: Wrapper<Dirty$signature$
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::completion(
                "dirty incomplete signature completion",
                "signature",
            )],
            expect![[r#"
                dirty incomplete signature completion
                - Saved Struct
                  detail: struct Saved
                  edit: /src/lib.rs:4:46-4:51 -> Saved
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_moved_body_keeps_its_associated_generic_scope() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_moved_body_completion"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn inspect<SessionGeneric: Default>() {
            let _: SessionGeneric = SessionGeneric::default();
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
use std::fmt::Debug;

pub fn inspect<SessionGeneric: Default>() {
    let _: SessionG$generic$ = SessionGeneric::default();
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::completion(
                "complete a generic from a moved saved owner",
                "generic",
            )],
            expect![[r#"
                complete a generic from a moved saved owner
                - SessionGeneric TypeParameter
                  detail: type parameter SessionGeneric
                  edit: /src/lib.rs:4:11-4:19 -> SessionGeneric
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_impl_completion_does_not_borrow_an_overlapping_saved_impl_site() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_impl_completion"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct SyntaxDocumentSymbolCollector;

        impl SyntaxDocumentSymbolCollector {}

        mod nested {
            pub struct NestedSaved;
        }
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full("src/lib.rs", 2, MarkedText::parse("impl Syn$completion$"))
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[LspQuery::completion(
                "dirty impl header completion",
                "completion",
            )],
            expect![[r#"
                dirty impl header completion
                - SyntaxDocumentSymbolCollector Struct
                  detail: struct SyntaxDocumentSymbolCollector
                  edit: /src/lib.rs:0:5-0:8 -> SyntaxDocumentSymbolCollector
                - nested Module
                  detail: mod nested
                  edit: /src/lib.rs:0:5-0:8 -> nested
            "#]],
        )
        .await;

    // An inline module must use its current module path too. Its byte offsets deliberately overlap
    // different saved syntax, so a saved scanner cannot safely claim this cursor by position.
    let nested = fixture
        .did_change_full(
            "src/lib.rs",
            3,
            MarkedText::parse(
                r#"mod nested {
    impl Nes$nested$
}"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &nested,
            &[LspQuery::completion(
                "dirty nested impl header completion",
                "nested",
            )],
            expect![[r#"
                dirty nested impl header completion
                - NestedSaved Struct
                  detail: struct NestedSaved
                  edit: /src/lib.rs:1:9-1:12 -> NestedSaved
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn dirty_completion_handles_associated_paths_at_incomplete_file_end() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_associated_completion"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Widget<T>(T);

        impl<T: Default> Widget<T> {
            pub fn new() -> Self {
                Self(T::default())
            }
        }

        pub fn edit_in_place() {}

        pub fn unfinished_at_eof() {}
        "#,
    )
    .await;

    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            MarkedText::parse(
                r#"
pub struct Widget<T>(T);

impl<T: Default> Widget<T> {
    pub fn new() -> Self {
        Self(T::default())
    }
}

pub fn edit_in_place() {
    let _ = Widget::<u8>::ne$edit$();
}

pub fn unfinished_at_eof() {
    let _ = Widget::<u8>::ne$eof$
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::completion("dirty associated edit-in-place", "edit"),
                LspQuery::completion("dirty associated path at EOF", "eof"),
            ],
            expect![[r#"
                dirty associated edit-in-place
                - new Function
                  detail: pub fn new() -> Self
                  edit: /src/lib.rs:10:26-10:28 -> new

                dirty associated path at EOF
                - new Function
                  detail: pub fn new() -> Self
                  edit: /src/lib.rs:14:26-14:28 -> new
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

/// Exercise syntax that exists only in the editor buffer at the moments users commonly pause.
///
/// Forward-typing changes end at the completion cursor; edit-in-place changes retain their call or
/// pattern delimiters. Together they prevent the protocol path from relying on absent punctuation
/// or producing text that duplicates syntax the user already wrote.
#[tokio::test]
async fn dirty_completion_handles_realistic_incomplete_typing_states() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_completion_typed_states"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Widget<T>(pub T);

        impl<T: Default> Widget<T> {
            pub fn new() -> Self {
                Self(T::default())
            }
        }

        pub struct Record {
            pub record_field: u8,
        }

        pub enum Event {
            Start,
            Data(u8),
            Stop { code: u8 },
        }

        pub trait Service {
            fn required(&self);
        }

        pub struct Worker;

        #[macro_export]
        macro_rules! local_macro {
            () => { 1u8 };
        }

        pub mod typed;

        //- /src/typed.rs
        impl crate::Service for crate::Worker {}

        pub fn macro_run() {}
        pub fn pattern_run(event: crate::Event) {}
        pub fn associated_run() {}
        pub fn record_run() {}
        pub fn format_run(local_capture: usize) {}

        //- /src/typed/parser.rs
        pub struct Parser;
        "#,
    )
    .await;

    fixture.did_open_saved("src/typed.rs", 1).await;

    let macro_edit = fixture
        .did_change_full(
            "src/typed.rs",
            2,
            MarkedText::parse(
                r#"
pub fn macro_run() {
    let _ = crate::local_m$macro_edit$!();
}
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &macro_edit,
            &[LspQuery::completion(
                "dirty macro edit keeps existing invocation",
                "macro_edit",
            )],
            expect![[r#"
                dirty macro edit keeps existing invocation
                - Event Enum
                  detail: enum Event
                  edit: /src/typed.rs:2:19-2:26 -> Event
                - Record Struct
                  detail: struct Record
                  edit: /src/typed.rs:2:19-2:26 -> Record
                - Service Interface
                  detail: trait Service
                  edit: /src/typed.rs:2:19-2:26 -> Service
                - Widget Struct
                  detail: struct Widget
                  edit: /src/typed.rs:2:19-2:26 -> Widget
                - Worker Struct
                  detail: struct Worker
                  edit: /src/typed.rs:2:19-2:26 -> Worker
                - local_macro Function
                  detail: macro local_macro
                  edit: /src/typed.rs:2:19-2:26 -> local_macro
                - typed Module
                  detail: mod typed
                  edit: /src/typed.rs:2:19-2:26 -> typed
            "#]],
        )
        .await;

    let pattern_edits = fixture
        .did_change_full(
            "src/typed.rs",
            3,
            MarkedText::parse(
                r#"
pub fn pattern_run(event: crate::Event) {
    let crate::Event::Dat$tuple_pattern$(_) = event;
    let crate::Event::Sto$record_pattern$ { code: _ } = event;
}
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &pattern_edits,
            &[
                LspQuery::completion(
                    "dirty tuple pattern keeps existing delimiters",
                    "tuple_pattern",
                ),
                LspQuery::completion(
                    "dirty record pattern keeps existing delimiters",
                    "record_pattern",
                ),
            ],
            expect![[r#"
                dirty tuple pattern keeps existing delimiters
                - Data EnumMember
                  detail: variant Data
                  edit: /src/typed.rs:2:22-2:25 -> Data

                dirty record pattern keeps existing delimiters
                - Stop EnumMember
                  detail: variant Stop
                  edit: /src/typed.rs:3:22-3:25 -> Stop
            "#]],
        )
        .await;

    let associated = fixture
        .did_change_full(
            "src/typed.rs",
            4,
            MarkedText::parse(
                r#"
pub fn associated_run() {
    let _ = crate::Widget::<u8>::$associated$
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &associated,
            &[LspQuery::completion(
                "dirty empty associated path at EOF",
                "associated",
            )],
            expect![[r#"
                dirty empty associated path at EOF
                - new Function
                  detail: pub fn new() -> Self
                  edit: /src/typed.rs:2:33-2:33 -> new
            "#]],
        )
        .await;

    let record = fixture
        .did_change_full(
            "src/typed.rs",
            5,
            MarkedText::parse(
                r#"
pub fn record_run() {
    let _ = crate::Record { $record$
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &record,
            &[LspQuery::completion(
                "dirty empty record field at EOF",
                "record",
            )],
            expect![[r#"
                dirty empty record field at EOF
                - record_field Field
                  detail: pub record_field: u8
                  edit: /src/typed.rs:2:28-2:28 -> record_field
            "#]],
        )
        .await;

    let trait_impl = fixture
        .did_change_full(
            "src/typed.rs",
            6,
            MarkedText::parse(
                r#"
impl crate::Service for crate::Worker {
    $trait_impl$
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &trait_impl,
            &[LspQuery::completion(
                "dirty empty trait impl at EOF",
                "trait_impl",
            )],
            expect![[r#"
                dirty empty trait impl at EOF
                - unsafe Keyword
                  detail: keyword unsafe
                  edit: /src/typed.rs:2:4-2:4 -> unsafe
                - async Keyword
                  detail: keyword async
                  edit: /src/typed.rs:2:4-2:4 -> async
                - extern Keyword
                  detail: keyword extern
                  edit: /src/typed.rs:2:4-2:4 -> extern
                - fn Keyword
                  detail: keyword fn
                  edit: /src/typed.rs:2:4-2:4 -> fn
                - const Keyword
                  detail: keyword const
                  edit: /src/typed.rs:2:4-2:4 -> const
                - type Keyword
                  detail: keyword type
                  edit: /src/typed.rs:2:4-2:4 -> type
                - required Function
                  detail: required trait member: fn required(&self)
                  edit: /src/typed.rs:2:4-2:4 -> "fn required(&self) {\n    todo!()\n}"
            "#]],
        )
        .await;

    let format_capture = fixture
        .did_change_full(
            "src/typed.rs",
            7,
            MarkedText::parse(
                r#"
pub fn format_run(local_capture: usize) {
    let _ = format!("{$format_capture$
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &format_capture,
            &[LspQuery::completion(
                "dirty empty format capture at EOF",
                "format_capture",
            )],
            expect![[r#"
                dirty empty format capture at EOF
                - local_capture Variable
                  detail: format capture local_capture
                  edit: /src/typed.rs:2:22-2:22 -> local_capture
            "#]],
        )
        .await;

    let module = fixture
        .did_change_full(
            "src/typed.rs",
            8,
            MarkedText::parse(
                r#"
mod $module$
"#,
            ),
        )
        .await;
    fixture
        .check_dirty(
            &module,
            &[LspQuery::completion(
                "dirty empty module declaration at EOF",
                "module",
            )],
            expect![[r#"
                dirty empty module declaration at EOF
                - parser Module
                  detail: mod parser
                  edit: /src/typed.rs:1:4-1:4 -> parser
            "#]],
        )
        .await;

    fixture.shutdown().await;
}

#[tokio::test]
async fn restored_unsaved_open_keeps_exact_syntax_without_new_global_semantics() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_restored_dirty_flow"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub struct Saved;
        "#,
    )
    .await;

    let dirty = fixture
        .did_open_dirty(
            "src/lib.rs",
            1,
            MarkedText::parse(
                r#"
pub struct Restored {
    /// Field that exists only in the restored editor buffer.
    pub restored_field: RestoredName,
}

pub struct RestoredName;

pub fn demo(value: Restored) {
    let _hover = value.restored_$hover$field;
}
"#,
            ),
        )
        .await;

    fixture
        .check_dirty(
            &dirty,
            &[
                LspQuery::hover("hover restored field", "hover"),
                LspQuery::document_symbol("restored document symbols", "src/lib.rs"),
            ],
            expect![[r#"
                hover restored field
                - none

                restored document symbols
                - Struct Restored 1:11-1:19
                  - Field restored_field 3:8-3:22
                - Struct RestoredName 6:11-6:23
                - Function demo 8:7-8:11
            "#]],
        )
        .await;

    fixture.shutdown().await;
}
