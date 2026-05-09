use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries, check_analysis_queries_with_sysroot};

#[test]
fn resolves_body_references_to_definition_targets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_goto_definition"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _again: User = loc$goto_local$al;
    let _made: User = hel$goto_item$per();
}
"#,
        &[
            AnalysisQuery::goto("goto local", "goto_local"),
            AnalysisQuery::goto("goto item", "goto_item"),
        ],
        expect![[r#"
            goto local
            - local local @ 8:9-8:14

            goto item
            - fn helper @ 3:8-3:14
        "#]],
    );
}

#[test]
fn resolves_binding_declarations_to_themselves() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_binding_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let loc$goto_decl$al: User = helper();
}
"#,
        &[AnalysisQuery::goto("goto declaration binding", "goto_decl")],
        expect![[r#"
            goto declaration binding
            - local local @ 8:9-8:14
        "#]],
    );
}

#[test]
fn lands_on_declaration_names_after_doc_comments() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_doc_comment_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
/// User docs.
pub struct User;

/// Builds a user.
pub fn make() -> User {
    User
}

pub fn use_it() {
    let _user: Us$goto_type$er = ma$goto_fn$ke();
}
"#,
        &[
            AnalysisQuery::goto("goto documented type", "goto_type"),
            AnalysisQuery::goto("goto documented function", "goto_fn"),
        ],
        expect![[r#"
            goto documented type
            - struct User @ 2:12-2:16

            goto documented function
            - fn make @ 5:8-5:12
        "#]],
    );
}

#[test]
fn resolves_field_accesses_to_field_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub profile: Profile,
}

pub fn use_it(user: User) {
    let _profile: Profile = user.pro$goto_field$file;
}
"#,
        &[AnalysisQuery::goto("goto field", "goto_field")],
        expect![[r#"
            goto field
            - field profile @ 4:9-4:16
        "#]],
    );
}

#[test]
fn resolves_body_local_field_accesses_to_field_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_field_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        local_id: GlobalId,
    }

    let user: User;
    let _id: GlobalId = user.loc$goto_field$al_id;
}
"#,
        &[AnalysisQuery::goto("goto body-local field", "goto_field")],
        expect![[r#"
            goto body-local field
            - field local_id @ 5:9-5:17
        "#]],
    );
}

#[test]
fn resolves_body_local_method_calls_to_method_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_method_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }
    }

    let user: User;
    let _id: GlobalId = user.i$goto_method$d();
}
"#,
        &[AnalysisQuery::goto("goto body-local method", "goto_method")],
        expect![[r#"
            goto body-local method
            - fn id @ 7:12-7:14
        "#]],
    );
}

#[test]
fn resolves_associated_functions_and_enum_variants_in_body_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_path_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Widget;

impl Widget {
    pub fn create() -> Self {
        Widget
    }
}

pub enum Action {
    Configure(Widget),
    Project { widget: Widget },
    Quit,
}

pub fn use_it(action: Action) {
    let widget = Wi$goto_assoc_type$dget::cre$goto_assoc_fn$ate();
    let _action = A$goto_enum_expr$ction::Con$goto_expr_variant$figure(Widget::create());

    match action {
        A$goto_enum_tuple_pattern$ction::Con$goto_tuple_variant$figure(widget) => widget,
        Action::Pro$goto_record_variant$ject { widget } => widget,
        Action::Quit => Widget,
    };
}
"#,
        &[
            AnalysisQuery::goto("goto associated type prefix", "goto_assoc_type"),
            AnalysisQuery::goto("goto associated function", "goto_assoc_fn"),
            AnalysisQuery::goto("goto enum expression prefix", "goto_enum_expr"),
            AnalysisQuery::goto("goto expression variant", "goto_expr_variant"),
            AnalysisQuery::goto("goto tuple pattern enum prefix", "goto_enum_tuple_pattern"),
            AnalysisQuery::goto("goto tuple pattern variant", "goto_tuple_variant"),
            AnalysisQuery::goto("goto record pattern variant", "goto_record_variant"),
        ],
        expect![[r#"
            goto associated type prefix
            - struct Widget @ 1:12-1:18

            goto associated function
            - fn create @ 4:12-4:18

            goto enum expression prefix
            - enum Action @ 9:10-9:16

            goto expression variant
            - variant Configure @ 10:5-10:14

            goto tuple pattern enum prefix
            - enum Action @ 9:10-9:16

            goto tuple pattern variant
            - variant Configure @ 10:5-10:14

            goto record pattern variant
            - variant Project @ 11:5-11:12
        "#]],
    );
}

#[test]
fn resolves_crate_prefixes_inside_body_type_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/helper", "crates/app"]
resolver = "3"

//- /crates/helper/Cargo.toml
[package]
name = "helper"
version = "0.1.0"
edition = "2024"

//- /crates/helper/src/lib.rs
pub struct Tool;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
helper = { path = "../helper" }

//- /crates/app/src/lib.rs
pub fn use_it() {
    let _tool: hel$goto_crate_prefix$per::Tool = todo!();
}
"#,
        &[AnalysisQuery::goto("goto crate prefix", "goto_crate_prefix").in_lib("app")],
        expect![[r#"
            goto crate prefix
            - module crate @ <root>
        "#]],
    );
}

#[test]
fn resolves_tuple_field_accesses_to_tuple_field_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_field_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Left;
pub struct Right;

pub struct Pair(pub Left, pub Right);

pub fn use_it(pair: Pair) {
    let _left: Left = pair.$goto_tuple_field$0;
}
"#,
        &[AnalysisQuery::goto("goto tuple field", "goto_tuple_field")],
        expect![[r#"
            goto tuple field
            - field #0 @ 4:17-4:25
        "#]],
    );
}

#[test]
fn resolves_direct_trait_method_calls_to_trait_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_direct_trait_method_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub trait Identify {
    fn id(&self);
}

impl Identify for User {
    fn id(&self) {}
}

pub fn use_it(user: User) {
    user.i$goto_direct_trait$d();
}
"#,
        &[AnalysisQuery::goto(
            "goto direct trait method",
            "goto_direct_trait",
        )],
        expect![[r#"
            goto direct trait method
            - fn id @ 4:8-4:10
        "#]],
    );
}

#[test]
fn resolves_use_and_signature_paths_to_definitions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_signature_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod api {
    pub trait Named {}
    pub struct User;
}

use a$goto_use_module$pi::Us$goto_use$er;

impl api::Na$goto_impl_trait$med for Us$goto_impl_self$er {}

pub fn make(user: Us$goto_param$er) -> Us$goto_ret$er {
    user
}
"#,
        &[
            AnalysisQuery::goto("goto use module", "goto_use_module"),
            AnalysisQuery::goto("goto use path", "goto_use"),
            AnalysisQuery::goto("goto impl trait", "goto_impl_trait"),
            AnalysisQuery::goto("goto impl self type", "goto_impl_self"),
            AnalysisQuery::goto("goto parameter type", "goto_param"),
            AnalysisQuery::goto("goto return type", "goto_ret"),
        ],
        expect![[r#"
            goto use module
            - module api @ 1:1-4:2

            goto use path
            - struct User @ 3:16-3:20

            goto impl trait
            - trait Named @ 2:15-2:20

            goto impl self type
            - struct User @ 3:16-3:20

            goto parameter type
            - struct User @ 3:16-3:20

            goto return type
            - struct User @ 3:16-3:20
        "#]],
    );
}

#[test]
fn resolves_bin_root_paths_to_library_definitions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub struct Thing;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

[lib]
path = "src/lib.rs"

[[bin]]
name = "app-bin"
path = "src/main.rs"

//- /crates/app/src/lib.rs
pub struct Api;

//- /crates/app/src/main.rs
fn main() {
    let _api: app::A$goto_bin_lib$pi = todo!();
    let _thing: dep::Th$goto_bin_dep$ing = todo!();
}
"#,
        &[
            AnalysisQuery::goto("goto bin root to library item", "goto_bin_lib").in_bin("app"),
            AnalysisQuery::goto("goto bin root to dependency item", "goto_bin_dep").in_bin("app"),
        ],
        expect![[r#"
            goto bin root to library item
            - struct Api @ 1:12-1:15

            goto bin root to dependency item
            - struct Thing @ 1:12-1:17
        "#]],
    );
}

#[test]
fn resolves_standard_prelude_signature_paths() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_prelude_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(_: Std$goto_prelude$Prelude) {}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct StdPrelude;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::StdPrelude;
    }
}
"#,
        &[
            AnalysisQuery::goto("goto prelude type", "goto_prelude")
                .in_lib("analysis_prelude_goto"),
        ],
        expect![[r#"
            goto prelude type
            - struct StdPrelude @ 2:16-2:26
        "#]],
    );
}

#[test]
fn resolves_standard_prelude_signature_paths_shadowed_by_non_type_names() {
    check_analysis_queries_with_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_prelude_shadow_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
const StdPrelude: u8 = 0;

pub fn value_shadow(_: Std$goto_value_shadow$Prelude) {}

mod macro_shadow {
    macro_rules! StdPrelude {
        () => {};
    }

    pub fn use_it(_: Std$goto_macro_shadow$Prelude) {}
}

//- /sysroot/library/core/src/lib.rs
pub struct Core;

//- /sysroot/library/alloc/src/lib.rs
pub struct Alloc;

//- /sysroot/library/std/src/lib.rs
pub mod marker {
    pub struct StdPrelude;
}

pub mod prelude {
    pub mod rust_2024 {
        pub use crate::marker::StdPrelude;
    }
}
"#,
        &[
            AnalysisQuery::goto("goto prelude type shadowed by value", "goto_value_shadow")
                .in_lib("analysis_prelude_shadow_goto"),
            AnalysisQuery::goto("goto prelude type shadowed by macro", "goto_macro_shadow")
                .in_lib("analysis_prelude_shadow_goto"),
        ],
        expect![[r#"
            goto prelude type shadowed by value
            - struct StdPrelude @ 2:16-2:26

            goto prelude type shadowed by macro
            - struct StdPrelude @ 2:16-2:26
        "#]],
    );
}

#[test]
fn resolves_cursors_inside_out_of_line_nested_modules() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "nested_analysis"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Api;
pub mod outer;

//- /src/outer.rs
pub mod inner;

//- /src/outer/inner.rs
pub fn use_it(_: crate::A$goto_nested_file$pi) {}
"#,
        &[
            AnalysisQuery::goto("goto from nested file", "goto_nested_file")
                .in_lib("nested_analysis"),
        ],
        expect![[r#"
            goto from nested file
            - struct Api @ 1:12-1:15
        "#]],
    );
}

#[test]
fn resolves_field_declarations_and_field_type_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_field_signature_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Profile;

pub struct User {
    pub pro$goto_field_decl$file: Pro$goto_field_type$file,
}
"#,
        &[
            AnalysisQuery::goto("goto field declaration", "goto_field_decl"),
            AnalysisQuery::goto("goto field type", "goto_field_type"),
        ],
        expect![[r#"
            goto field declaration
            - field profile @ 4:9-4:16

            goto field type
            - struct Profile @ 1:12-1:19
        "#]],
    );
}

#[test]
fn resolves_import_alias_cursors_to_imported_definitions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_import_alias_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod api {
    pub struct User;
}

use api::User as Acc$goto_import_alias$ount;
"#,
        &[AnalysisQuery::goto(
            "goto import alias",
            "goto_import_alias",
        )],
        expect![[r#"
            goto import alias
            - struct User @ 2:16-2:20
        "#]],
    );
}

#[test]
fn resolves_self_in_impl_signatures_to_impl_self_type() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_impl_self_signature_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn new() -> Se$goto_impl_self_signature$lf {
        User
    }
}
"#,
        &[AnalysisQuery::goto(
            "goto impl signature Self",
            "goto_impl_self_signature",
        )],
        expect![[r#"
            goto impl signature Self
            - struct User @ 1:12-1:16
        "#]],
    );
}

#[test]
fn resolves_body_local_structs_before_module_structs() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_struct_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn make() {
    struct User;
    let _local: Us$goto_local_type$er = Us$goto_local_ctor$er;
}

pub fn outside() {
    let _outside: User = Us$goto_module_ctor$er;
}
"#,
        &[
            AnalysisQuery::goto("goto local type path", "goto_local_type"),
            AnalysisQuery::goto("goto local constructor", "goto_local_ctor"),
            AnalysisQuery::goto("goto module constructor", "goto_module_ctor"),
        ],
        expect![[r#"
            goto local type path
            - struct User @ 4:12-4:16

            goto local constructor
            - struct User @ 4:12-4:16

            goto module constructor
            - struct User @ 1:12-1:16
        "#]],
    );
}

#[test]
fn resolves_body_let_annotation_paths_with_body_context() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_annotation_goto"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

impl User {
    pub fn capture(&self) {
        let _this: Se$goto_body_self$lf = self;
    }
}

pub fn make() {
    struct User;
    let _: Us$goto_wildcard_type$er = User;
    let (_left, _right): (Us$goto_tuple_left$er, Us$goto_tuple_right$er) = User;
}
"#,
        &[
            AnalysisQuery::goto("goto body Self annotation", "goto_body_self"),
            AnalysisQuery::goto("goto wildcard annotation", "goto_wildcard_type"),
            AnalysisQuery::goto("goto tuple annotation left", "goto_tuple_left"),
            AnalysisQuery::goto("goto tuple annotation right", "goto_tuple_right"),
        ],
        expect![[r#"
            goto body Self annotation
            - struct User @ 1:12-1:16

            goto wildcard annotation
            - struct User @ 10:12-10:16

            goto tuple annotation left
            - struct User @ 10:12-10:16

            goto tuple annotation right
            - struct User @ 10:12-10:16
        "#]],
    );
}
