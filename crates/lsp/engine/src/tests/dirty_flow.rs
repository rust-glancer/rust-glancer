use expect_test::expect;
use test_fixture::testonly::MarkedText;

use super::utils::{LspEngineFixture, LspQuery};

#[tokio::test]
async fn queries_use_dirty_full_text_overlay() {
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
                - dirty_field Field
                  detail: pub dirty_field: DirtyName
                  edit: /src/lib.rs:9:27-9:33 -> dirty_field

                hover dirty field
                - range: /src/lib.rs:10:22-10:33
                - markdown:
                  ```rust
                  lsp_dirty_flow::DirtyUser
                  ```

                  ```rust
                  pub dirty_field: DirtyName
                  ```

                  Field that exists only in the unsaved buffer.

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
async fn dirty_completion_preserves_specialized_and_postfix_edit_ranges() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_dirty_specialized_completions"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub fn saved() {}
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
async fn dirty_completion_uses_incomplete_function_signature() {
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
                - T TypeParameter
                  detail: type parameter T
                  edit: /src/lib.rs:4:46-4:51 -> T
                - N Constant
                  detail: const parameter N
                  edit: /src/lib.rs:4:46-4:51 -> N
                - DirtyFixture Struct
                  detail: struct DirtyFixture
                  edit: /src/lib.rs:4:46-4:51 -> DirtyFixture
                - Wrapper Struct
                  detail: struct Wrapper
                  edit: /src/lib.rs:4:46-4:51 -> Wrapper
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
        pub fn saved() {}

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
pub fn run() {
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
pub fn run(event: crate::Event) {
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
pub fn run() {
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
pub fn run() {
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
pub fn run(local_capture: usize) {
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
async fn restored_unsaved_open_uses_dirty_full_text_overlay() {
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
                - range: /src/lib.rs:9:23-9:37
                - markdown:
                  ```rust
                  lsp_restored_dirty_flow::Restored
                  ```

                  ```rust
                  pub restored_field: RestoredName
                  ```

                  Field that exists only in the restored editor buffer.

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
