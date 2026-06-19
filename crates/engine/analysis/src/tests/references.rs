use expect_test::expect;

use super::utils::{AnalysisQuery, ReferenceQuery, check_analysis_queries};

#[test]
fn finds_common_reference_subjects() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    na$field_ref$me: Name,
}

pub struct Name;

pub fn helper(user: User) -> Name {
    user.name
}

pub fn use_it() {
    let loc$local_ref$al: Us$type_ref$er;
    let _again: User = local;
    let _name = hel$fn_ref$per(local);
}
"#,
        &[
            AnalysisQuery::references("type references", "type_ref", ReferenceQuery::all()),
            AnalysisQuery::references(
                "type references without declaration",
                "type_ref",
                ReferenceQuery::all().without_declaration(),
            ),
            AnalysisQuery::references("field references", "field_ref", ReferenceQuery::all()),
            AnalysisQuery::references("function references", "fn_ref", ReferenceQuery::all()),
            AnalysisQuery::references("local references", "local_ref", ReferenceQuery::all()),
        ],
        expect![[r#"
            type references
            - `User` @ src/lib.rs:1:12-1:16
            - `User` @ src/lib.rs:7:21-7:25
            - `User` @ src/lib.rs:12:16-12:20
            - `User` @ src/lib.rs:13:17-13:21

            type references without declaration
            - `User` @ src/lib.rs:7:21-7:25
            - `User` @ src/lib.rs:12:16-12:20
            - `User` @ src/lib.rs:13:17-13:21

            field references
            - `name` @ src/lib.rs:2:5-2:9
            - `name` @ src/lib.rs:8:10-8:14

            function references
            - `helper` @ src/lib.rs:7:8-7:14
            - `helper` @ src/lib.rs:14:17-14:23

            local references
            - `local` @ src/lib.rs:12:9-12:14
            - `local` @ src/lib.rs:13:24-13:29
            - `local` @ src/lib.rs:14:24-14:29
        "#]],
    );
}

#[test]
fn finds_item_initializer_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_item_initializer_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const LIMIT: u32 = 3;
pub static CURRENT: u32 = LI$initializer_ref$MIT;

pub fn use_it() -> u32 {
    LIMIT
}
"#,
        &[AnalysisQuery::references(
            "const references",
            "initializer_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            const references
            - `LIMIT` @ src/lib.rs:1:11-1:16
            - `LIMIT` @ src/lib.rs:2:27-2:32
            - `LIMIT` @ src/lib.rs:5:5-5:10
        "#]],
    );
}

#[test]
fn local_binding_method_receiver_shadows_same_name_function() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_receiver_shadow_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn bar(baz: u8) -> u8 {
    fo$fn_ref$o(baz)
}

fn foo(baz: u8) -> u8 {
    let fo$local_ref$o: Option<u8> = Some(baz);
    fo$local_use$o.map(|baba| baba + baba);
    baz
}
"#,
        &[
            AnalysisQuery::references("function references", "fn_ref", ReferenceQuery::all()),
            AnalysisQuery::references("local references", "local_ref", ReferenceQuery::all()),
        ],
        expect![[r#"
            function references
            - `foo` @ src/lib.rs:2:5-2:8
            - `foo` @ src/lib.rs:5:4-5:7

            local references
            - `foo` @ src/lib.rs:6:9-6:12
            - `foo` @ src/lib.rs:7:5-7:8
        "#]],
    );
}

#[test]
fn finds_body_local_method_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_method_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Id;

pub fn use_it() {
    struct User;

    impl User {
        fn i$method_ref$d(&self) -> Id {
            Id
        }
    }

    let user: User;
    user.id();
}
"#,
        &[AnalysisQuery::references(
            "body-local method references",
            "method_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            body-local method references
            - `id` @ src/lib.rs:7:12-7:14
            - `id` @ src/lib.rs:13:10-13:12
        "#]],
    );
}

#[test]
fn finds_more_body_local_item_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_more_body_local_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    type Al$alias_ref$ias = GlobalId;
    const DE$const_ref$FAULT: Alias = GlobalId;
    static CU$static_ref$RRENT: GlobalId = GlobalId;
    fn hel$fn_ref$per() -> Alias {
        DEFAULT
    }
    enum Action {
        Sta$variant_ref$rt,
        Stop,
    }

    let _typed: Alias = helper();
    let _default = DEFAULT;
    let _current = CURRENT;
    let _action = Action::Start;
}
"#,
        &[
            AnalysisQuery::references(
                "body-local alias references",
                "alias_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local const references",
                "const_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local static references",
                "static_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local function references",
                "fn_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local enum variant references",
                "variant_ref",
                ReferenceQuery::all(),
            ),
        ],
        expect![[r#"
            body-local alias references
            - `Alias` @ src/lib.rs:4:10-4:15
            - `Alias` @ src/lib.rs:5:20-5:25
            - `Alias` @ src/lib.rs:7:20-7:25
            - `Alias` @ src/lib.rs:15:17-15:22

            body-local const references
            - `DEFAULT` @ src/lib.rs:5:11-5:18
            - `DEFAULT` @ src/lib.rs:8:9-8:16
            - `DEFAULT` @ src/lib.rs:16:20-16:27

            body-local static references
            - `CURRENT` @ src/lib.rs:6:12-6:19
            - `CURRENT` @ src/lib.rs:17:20-17:27

            body-local function references
            - `helper` @ src/lib.rs:7:8-7:14
            - `helper` @ src/lib.rs:15:25-15:31

            body-local enum variant references
            - `Start` @ src/lib.rs:11:9-11:14
            - `Start` @ src/lib.rs:18:27-18:32
        "#]],
    );
}

#[test]
fn finds_scope_ordered_body_local_value_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_value_shadowing_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Outer;
pub struct Inner;

pub fn make() {
    fn hel$outer_fn_ref$per() -> Outer {
        Outer
    }
    let value = Outer;

    {
        fn val$inner_fn_ref$ue() -> Inner {
            Inner
        }
        let _from_fn = value();
    };

    {
        const hel$inner_const_ref$per: Inner = Inner;
        let _from_const = helper;
    };
}
"#,
        &[
            AnalysisQuery::references(
                "outer function references",
                "outer_fn_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "inner function references",
                "inner_fn_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "inner const references",
                "inner_const_ref",
                ReferenceQuery::all(),
            ),
        ],
        expect![[r#"
            outer function references
            - `helper` @ src/lib.rs:5:8-5:14

            inner function references
            - `value` @ src/lib.rs:11:12-11:17
            - `value` @ src/lib.rs:14:24-14:29

            inner const references
            - `helper` @ src/lib.rs:18:15-18:21
            - `helper` @ src/lib.rs:19:27-19:33
        "#]],
    );
}

#[test]
fn finds_parent_body_local_references_from_nested_body_owners() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_nested_body_owner_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    struct Local;
    const SE$const_ref$ED: Local = Local;
    static CURRENT: Local = SE$static_initializer_use$ED;

    fn helper() -> Local {
        SEED
    }

    const AGAIN: Local = SE$const_initializer_use$ED;
    static LAST: Local = SEED;
    let _direct = SEED;
}
"#,
        &[
            AnalysisQuery::references(
                "parent body-local const references",
                "const_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "parent body-local const references from const initializer",
                "const_initializer_use",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "parent body-local const references from static initializer",
                "static_initializer_use",
                ReferenceQuery::all(),
            ),
        ],
        expect![[r#"
            parent body-local const references
            - `SEED` @ src/lib.rs:5:11-5:15
            - `SEED` @ src/lib.rs:6:29-6:33
            - `SEED` @ src/lib.rs:9:9-9:13
            - `SEED` @ src/lib.rs:12:26-12:30
            - `SEED` @ src/lib.rs:13:26-13:30
            - `SEED` @ src/lib.rs:14:19-14:23

            parent body-local const references from const initializer
            - `SEED` @ src/lib.rs:5:11-5:15
            - `SEED` @ src/lib.rs:6:29-6:33
            - `SEED` @ src/lib.rs:9:9-9:13
            - `SEED` @ src/lib.rs:12:26-12:30
            - `SEED` @ src/lib.rs:13:26-13:30
            - `SEED` @ src/lib.rs:14:19-14:23

            parent body-local const references from static initializer
            - `SEED` @ src/lib.rs:5:11-5:15
            - `SEED` @ src/lib.rs:6:29-6:33
            - `SEED` @ src/lib.rs:9:9-9:13
            - `SEED` @ src/lib.rs:12:26-12:30
            - `SEED` @ src/lib.rs:13:26-13:30
            - `SEED` @ src/lib.rs:14:19-14:23
        "#]],
    );
}

#[test]
fn finds_body_local_associated_item_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_assoc_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    struct User;

    impl User {
        const DE$assoc_const_ref$FAULT: GlobalId = GlobalId;
        type I$assoc_type_ref$d = GlobalId;
    }

    let _default = User::DEFAULT;
    let _typed: User::Id = GlobalId;
}
"#,
        &[
            AnalysisQuery::references(
                "body-local associated const references",
                "assoc_const_ref",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local associated type references",
                "assoc_type_ref",
                ReferenceQuery::all(),
            ),
        ],
        expect![[r#"
            body-local associated const references
            - `DEFAULT` @ src/lib.rs:7:15-7:22
            - `DEFAULT` @ src/lib.rs:11:26-11:33

            body-local associated type references
            - `Id` @ src/lib.rs:8:14-8:16
            - `Id` @ src/lib.rs:12:23-12:25
        "#]],
    );
}

#[test]
fn finds_parent_body_local_references_inside_associated_item_bodies() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_assoc_body_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    const SE$seed_ref$ED: GlobalId = GlobalId;

    struct User;

    trait Named {
        const TRAIT_DEFAULT: GlobalId = SEED;

        fn trait_make() -> GlobalId {
            SEED
        }
    }

    impl User {
        const DEFAULT: GlobalId = SEED;

        fn make() -> GlobalId {
            SEED
        }
    }

    let _default = User::DEFAULT;
    let _made = User::make();
}
"#,
        &[AnalysisQuery::references(
            "parent body-local const references inside associated bodies",
            "seed_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            parent body-local const references inside associated bodies
            - `SEED` @ src/lib.rs:4:11-4:15
            - `SEED` @ src/lib.rs:9:41-9:45
            - `SEED` @ src/lib.rs:12:13-12:17
            - `SEED` @ src/lib.rs:17:35-17:39
            - `SEED` @ src/lib.rs:20:13-20:17
        "#]],
    );
}

#[test]
fn finds_lowercase_const_references_from_ambiguous_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_ambiguous_pattern_const_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const re$ready_ref$ady: u8 = 1;

pub fn use_it(value: u8) {
    match value {
        ready => {}
        _ => {}
    }
}
"#,
        &[AnalysisQuery::references(
            "lowercase const references from ambiguous pattern",
            "ready_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            lowercase const references from ambiguous pattern
            - `ready` @ src/lib.rs:1:11-1:16
            - `ready` @ src/lib.rs:5:9-5:14
        "#]],
    );
}

#[test]
fn scoped_references_keep_external_declaration_without_external_uses() {
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

pub fn helper_use(tool: Tool) {
    let _again: Tool = tool;
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
helper = { path = "../helper" }

//- /crates/app/src/lib.rs
pub fn use_it() {
    let tool: helper::To$scoped_type_ref$ol = todo!();
    let _again: helper::Tool = tool;
}
"#,
        &[AnalysisQuery::references(
            "scoped type references",
            "scoped_type_ref",
            ReferenceQuery::current_target(),
        )
        .in_lib("app")],
        expect![[r#"
            scoped type references
            - `Tool` @ app/src/lib.rs:2:23-2:27
            - `Tool` @ app/src/lib.rs:3:25-3:29
            - `Tool` @ helper/src/lib.rs:1:12-1:16
        "#]],
    );
}

#[test]
fn file_scoped_references_skip_other_files_in_same_target() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_file_scoped_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;

pub struct User;

pub fn use_here() {
    let _: Us$file_scoped_type_ref$er;
}

//- /src/other.rs
use crate::User;

pub fn use_there(_: User) {}
"#,
        &[AnalysisQuery::references(
            "file-scoped type references",
            "file_scoped_type_ref",
            ReferenceQuery::current_file(),
        )],
        expect![[r#"
            file-scoped type references
            - `User` @ src/lib.rs:3:12-3:16
            - `User` @ src/lib.rs:6:12-6:16
        "#]],
    );
}

#[test]
fn file_list_references_scan_selected_files_only() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_file_list_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;
mod skipped;

pub struct User;

pub fn use_here() {
    let _: Us$file_list_type_ref$er;
}

//- /src/other.rs
use crate::User;

pub fn use_there(_: User) {}

//- /src/skipped.rs
use crate::User;

pub fn use_elsewhere(_: User) {}
"#,
        &[AnalysisQuery::references(
            "file-list type references",
            "file_list_type_ref",
            ReferenceQuery::files(&["src/lib.rs", "src/other.rs"]),
        )],
        expect![[r#"
            file-list type references
            - `User` @ src/lib.rs:4:12-4:16
            - `User` @ src/lib.rs:7:12-7:16
            - `User` @ src/other.rs:1:12-1:16
            - `User` @ src/other.rs:3:21-3:25
        "#]],
    );
}

#[test]
fn file_scoped_references_do_not_include_external_declaration() {
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
    let tool: helper::To$file_scoped_external_decl$ol = todo!();
    let _again: helper::Tool = tool;
}
"#,
        &[AnalysisQuery::references(
            "file-scoped external declaration references",
            "file_scoped_external_decl",
            ReferenceQuery::current_file(),
        )
        .in_lib("app")],
        expect![[r#"
            file-scoped external declaration references
            - `Tool` @ app/src/lib.rs:2:23-2:27
            - `Tool` @ app/src/lib.rs:3:25-3:29
        "#]],
    );
}

#[test]
fn scoped_references_from_dependency_include_reverse_dependency_uses() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/app"]
exclude = ["dep", "helper"]
resolver = "3"

//- /dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /dep/src/lib.rs
pub struct A$dep_api$pi;

pub fn dep_use(_: Api) {}

//- /helper/Cargo.toml
[package]
name = "helper"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /helper/src/lib.rs
pub fn helper_use(_: dep::Api) {}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../../dep" }
helper = { path = "../../helper" }

//- /crates/app/src/lib.rs
pub fn app_use(_: dep::Api) {
    helper::helper_use(todo!());
}
"#,
        &[AnalysisQuery::references(
            "dependency-scoped type references",
            "dep_api",
            ReferenceQuery::libs(&["dep", "helper", "app"]),
        )
        .in_lib("dep")],
        expect![[r#"
            dependency-scoped type references
            - `Api` @ app/src/lib.rs:1:24-1:27
            - `Api` @ dep/src/lib.rs:1:12-1:15
            - `Api` @ dep/src/lib.rs:3:19-3:22
            - `Api` @ helper/src/lib.rs:1:27-1:30
        "#]],
    );
}

#[test]
fn references_keep_import_alias_uses_for_type_declarations() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_import_alias_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct Us$aliased_type_ref$er;
}

use api::User as Account;

pub fn use_it(_: Account) {}
"#,
        &[AnalysisQuery::references(
            "aliased type references",
            "aliased_type_ref",
            ReferenceQuery::all(),
        )],
        expect![[r#"
            aliased type references
            - `User` @ src/lib.rs:2:16-2:20
            - `User` @ src/lib.rs:5:10-5:14
            - `Account` @ src/lib.rs:7:18-7:25
        "#]],
    );
}
