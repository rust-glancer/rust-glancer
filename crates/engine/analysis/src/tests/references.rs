use expect_test::expect;

use super::utils::{AnalysisQuery, ReferenceQuery, check_analysis, check_analysis_queries};

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
            // type references
            `User` @ src/lib.rs:1:12-1:16
            `User` @ src/lib.rs:7:21-7:25
            `User` @ src/lib.rs:12:16-12:20
            `User` @ src/lib.rs:13:17-13:21

            // type references without declaration
            `User` @ src/lib.rs:7:21-7:25
            `User` @ src/lib.rs:12:16-12:20
            `User` @ src/lib.rs:13:17-13:21

            // field references
            `name` @ src/lib.rs:2:5-2:9
            `name` @ src/lib.rs:8:10-8:14

            // function references
            `helper` @ src/lib.rs:7:8-7:14
            `helper` @ src/lib.rs:14:17-14:23

            // local references
            `local` @ src/lib.rs:12:9-12:14
            `local` @ src/lib.rs:13:24-13:29
            `local` @ src/lib.rs:14:24-14:29
        "#]],
    );
}

#[test]
fn finds_record_constructor_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_record_constructor_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod model {
    pub struct Qual$qualified_decl$ified<T> {
        pub value: T,
    }
}

pub struct Us$user_decl$er<T> {
    pub value: T,
}

pub enum Action<T> {
    Start { value: T },
}

use crate::User as Account;

pub fn make(value: u8) {
    struct Local {
        value: u8,
    }

    let _plain = User { value };
    let _generic = User::<u8> { value };
    let _qualified = model::Qual$qualified_use$ified::<u8> { value };
    let _alias = Acc$alias_use$ount { value };
    let _variant = Action::Sta$variant_use$rt { value };
    let _local = Loc$local_use$al { value };
}
"#,
        &[
            AnalysisQuery::references(
                "record struct references from declaration",
                "user_decl",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "record struct references from imported constructor alias without declaration",
                "alias_use",
                ReferenceQuery::all().without_declaration(),
            ),
            AnalysisQuery::references(
                "qualified generic record constructor references",
                "qualified_use",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "record variant constructor references",
                "variant_use",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "body-local record constructor references",
                "local_use",
                ReferenceQuery::all(),
            ),
        ],
        expect![[r#"
            // record struct references from declaration
            `User` @ src/lib.rs:7:12-7:16
            `User` @ src/lib.rs:15:12-15:16
            `User` @ src/lib.rs:22:18-22:22
            `User` @ src/lib.rs:23:20-23:24

            // record struct references from imported constructor alias without declaration
            `User` @ src/lib.rs:15:12-15:16
            `User` @ src/lib.rs:22:18-22:22
            `User` @ src/lib.rs:23:20-23:24
            `Account` @ src/lib.rs:25:18-25:25

            // qualified generic record constructor references
            `Qualified` @ src/lib.rs:2:16-2:25
            `Qualified` @ src/lib.rs:24:29-24:38

            // record variant constructor references
            `Start` @ src/lib.rs:12:5-12:10
            `Start` @ src/lib.rs:26:28-26:33

            // body-local record constructor references
            `Local` @ src/lib.rs:18:12-18:17
            `Local` @ src/lib.rs:27:18-27:23
        "#]],
    );
}

#[test]
fn finds_item_initializer_references() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_item_initializer_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const LIMIT: u32 = 3;
pub static CURRENT: u32 = LI$0MIT; // references("const references", ReferenceQuery::all())

pub fn use_it() -> u32 {
    LIMIT
}
"#,
        expect![[r#"
            // const references
            `LIMIT` @ src/lib.rs:1:11-1:16
            `LIMIT` @ src/lib.rs:2:27-2:32
            `LIMIT` @ src/lib.rs:5:5-5:10
        "#]],
    );
}

#[test]
fn finds_body_macro_call_references() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_macro_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Text;

pub fn helper(value: u64) -> Text { Text }

/// Builds text from a value.
macro_rules! make_$macro_decl$text {
    ($value:expr) => { helper($value) };
}

pub fn use_it(input: u64) {
    let _first = make_$macro_call$text!(input);
    let _second = make_text!(input + 1);
}
"#,
        &[
            AnalysisQuery::references(
                "macro references from declaration",
                "macro_decl",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "macro references from body call",
                "macro_call",
                ReferenceQuery::all(),
            ),
            AnalysisQuery::references(
                "macro references without declaration",
                "macro_call",
                ReferenceQuery::all().without_declaration(),
            ),
        ],
        expect![[r#"
            // macro references from declaration
            `make_text` @ src/lib.rs:6:14-6:23
            `make_text` @ src/lib.rs:11:18-11:27
            `make_text` @ src/lib.rs:12:19-12:28

            // macro references from body call
            `make_text` @ src/lib.rs:6:14-6:23
            `make_text` @ src/lib.rs:11:18-11:27
            `make_text` @ src/lib.rs:12:19-12:28

            // macro references without declaration
            `make_text` @ src/lib.rs:11:18-11:27
            `make_text` @ src/lib.rs:12:19-12:28
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
            // function references
            `foo` @ src/lib.rs:2:5-2:8
            `foo` @ src/lib.rs:5:4-5:7

            // local references
            `foo` @ src/lib.rs:6:9-6:12
            `foo` @ src/lib.rs:7:5-7:8
        "#]],
    );
}

#[test]
fn finds_body_local_method_references() {
    check_analysis(
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
        fn i$0d(&self) -> Id { // references("body-local method references", ReferenceQuery::all())
            Id
        }
    }

    let user: User;
    user.id();
}
"#,
        expect![[r#"
            // body-local method references
            `id` @ src/lib.rs:7:12-7:14
            `id` @ src/lib.rs:13:10-13:12
        "#]],
    );
}

#[test]
fn finds_more_body_local_item_references() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_more_body_local_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    type Al$0ias = GlobalId; // references("body-local alias references", ReferenceQuery::all())
    const DE$0FAULT: Alias = GlobalId; // references("body-local const references", ReferenceQuery::all())
    static CU$0RRENT: GlobalId = GlobalId; // references("body-local static references", ReferenceQuery::all())
    fn hel$0per() -> Alias { // references("body-local function references", ReferenceQuery::all())
        DEFAULT
    }
    enum Action {
        Sta$0rt, // references("body-local enum variant references", ReferenceQuery::all())
        Stop,
    }

    let _typed: Alias = helper();
    let _default = DEFAULT;
    let _current = CURRENT;
    let _action = Action::Start;
}
"#,
        expect![[r#"
            // body-local alias references
            `Alias` @ src/lib.rs:4:10-4:15
            `Alias` @ src/lib.rs:5:20-5:25
            `Alias` @ src/lib.rs:7:20-7:25
            `Alias` @ src/lib.rs:15:17-15:22

            // body-local const references
            `DEFAULT` @ src/lib.rs:5:11-5:18
            `DEFAULT` @ src/lib.rs:8:9-8:16
            `DEFAULT` @ src/lib.rs:16:20-16:27

            // body-local static references
            `CURRENT` @ src/lib.rs:6:12-6:19
            `CURRENT` @ src/lib.rs:17:20-17:27

            // body-local function references
            `helper` @ src/lib.rs:7:8-7:14
            `helper` @ src/lib.rs:15:25-15:31

            // body-local enum variant references
            `Start` @ src/lib.rs:11:9-11:14
            `Start` @ src/lib.rs:18:27-18:32
        "#]],
    );
}

#[test]
fn finds_scope_ordered_body_local_value_references() {
    check_analysis(
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
    fn hel$0per() -> Outer { // references("outer function references", ReferenceQuery::all())
        Outer
    }
    let value = Outer;

    {
        fn val$0ue() -> Inner { // references("inner function references", ReferenceQuery::all())
            Inner
        }
        let _from_fn = value();
    };

    {
        const hel$0per: Inner = Inner; // references("inner const references", ReferenceQuery::all())
        let _from_const = helper;
    };
}
"#,
        expect![[r#"
            `helper` @ src/lib.rs:5:8-5:14 // outer function references

            // inner function references
            `value` @ src/lib.rs:11:12-11:17
            `value` @ src/lib.rs:14:24-14:29

            // inner const references
            `helper` @ src/lib.rs:18:15-18:21
            `helper` @ src/lib.rs:19:27-19:33
        "#]],
    );
}

#[test]
fn finds_parent_body_local_references_from_nested_body_owners() {
    check_analysis(
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
    const SE$0ED: Local = Local; // references("parent body-local const references", ReferenceQuery::all())
    static CURRENT: Local = SE$0ED; // references("parent body-local const references from static initializer", ReferenceQuery::all())

    fn helper() -> Local {
        SEED
    }

    const AGAIN: Local = SE$0ED; // references("parent body-local const references from const initializer", ReferenceQuery::all())
    static LAST: Local = SEED;
    let _direct = SEED;
}
"#,
        expect![[r#"
            // parent body-local const references
            `SEED` @ src/lib.rs:5:11-5:15
            `SEED` @ src/lib.rs:6:29-6:33
            `SEED` @ src/lib.rs:9:9-9:13
            `SEED` @ src/lib.rs:12:26-12:30
            `SEED` @ src/lib.rs:13:26-13:30
            `SEED` @ src/lib.rs:14:19-14:23

            // parent body-local const references from static initializer
            `SEED` @ src/lib.rs:5:11-5:15
            `SEED` @ src/lib.rs:6:29-6:33
            `SEED` @ src/lib.rs:9:9-9:13
            `SEED` @ src/lib.rs:12:26-12:30
            `SEED` @ src/lib.rs:13:26-13:30
            `SEED` @ src/lib.rs:14:19-14:23

            // parent body-local const references from const initializer
            `SEED` @ src/lib.rs:5:11-5:15
            `SEED` @ src/lib.rs:6:29-6:33
            `SEED` @ src/lib.rs:9:9-9:13
            `SEED` @ src/lib.rs:12:26-12:30
            `SEED` @ src/lib.rs:13:26-13:30
            `SEED` @ src/lib.rs:14:19-14:23
        "#]],
    );
}

#[test]
fn finds_body_local_associated_item_references() {
    check_analysis(
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
        const DE$0FAULT: GlobalId = GlobalId; // references("body-local associated const references", ReferenceQuery::all())
        type I$0d = GlobalId; // references("body-local associated type references", ReferenceQuery::all())
    }

    let _default = User::DEFAULT;
    let _typed: User::Id = GlobalId;
}
"#,
        expect![[r#"
            // body-local associated const references
            `DEFAULT` @ src/lib.rs:7:15-7:22
            `DEFAULT` @ src/lib.rs:11:26-11:33

            // body-local associated type references
            `Id` @ src/lib.rs:8:14-8:16
            `Id` @ src/lib.rs:12:23-12:25
        "#]],
    );
}

#[test]
fn finds_parent_body_local_references_inside_associated_item_bodies() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_assoc_body_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn make() {
    const SE$0ED: GlobalId = GlobalId; // references("parent body-local const references inside associated bodies", ReferenceQuery::all())

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
        expect![[r#"
            // parent body-local const references inside associated bodies
            `SEED` @ src/lib.rs:4:11-4:15
            `SEED` @ src/lib.rs:9:41-9:45
            `SEED` @ src/lib.rs:12:13-12:17
            `SEED` @ src/lib.rs:17:35-17:39
            `SEED` @ src/lib.rs:20:13-20:17
        "#]],
    );
}

#[test]
fn finds_lowercase_const_references_from_ambiguous_patterns() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_ambiguous_pattern_const_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const re$0ady: u8 = 1; // references("lowercase const references from ambiguous pattern", ReferenceQuery::all())

pub fn use_it(value: u8) {
    match value {
        ready => {}
        _ => {}
    }
}
"#,
        expect![[r#"
            // lowercase const references from ambiguous pattern
            `ready` @ src/lib.rs:1:11-1:16
            `ready` @ src/lib.rs:5:9-5:14
        "#]],
    );
}

#[test]
fn reference_scopes_handle_external_declarations() {
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
        &[
            AnalysisQuery::references(
                "target-scoped external type references",
                "scoped_type_ref",
                ReferenceQuery::current_target(),
            )
            .in_lib("app"),
            AnalysisQuery::references(
                "file-scoped external type references",
                "scoped_type_ref",
                ReferenceQuery::current_file(),
            )
            .in_lib("app"),
        ],
        expect![[r#"
            // target-scoped external type references
            `Tool` @ app/src/lib.rs:2:23-2:27
            `Tool` @ app/src/lib.rs:3:25-3:29
            `Tool` @ helper/src/lib.rs:1:12-1:16

            // file-scoped external type references
            `Tool` @ app/src/lib.rs:2:23-2:27
            `Tool` @ app/src/lib.rs:3:25-3:29
        "#]],
    );
}

#[test]
fn reference_file_selection_honors_current_and_explicit_file_sets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reference_file_selection"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod other;
mod skipped;

pub struct User;

pub fn use_here() {
    let _: Us$file_selection_type_ref$er;
}

//- /src/other.rs
use crate::User;

pub fn use_there(_: User) {}

//- /src/skipped.rs
use crate::User;

pub fn use_elsewhere(_: User) {}
"#,
        &[
            AnalysisQuery::references(
                "current-file type references",
                "file_selection_type_ref",
                ReferenceQuery::current_file(),
            ),
            AnalysisQuery::references(
                "explicit-file-list type references",
                "file_selection_type_ref",
                ReferenceQuery::files(&["src/lib.rs", "src/other.rs"]),
            ),
        ],
        expect![[r#"
            // current-file type references
            `User` @ src/lib.rs:4:12-4:16
            `User` @ src/lib.rs:7:12-7:16

            // explicit-file-list type references
            `User` @ src/lib.rs:4:12-4:16
            `User` @ src/lib.rs:7:12-7:16
            `User` @ src/other.rs:1:12-1:16
            `User` @ src/other.rs:3:21-3:25
        "#]],
    );
}

#[test]
fn scoped_references_from_dependency_include_reverse_dependency_uses() {
    check_analysis(
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
pub struct A$0pi; // references("dependency-scoped type references", ReferenceQuery::libs(&["dep", "helper", "app"])).in_lib("dep")

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
        expect![[r#"
            // dependency-scoped type references
            `Api` @ app/src/lib.rs:1:24-1:27
            `Api` @ dep/src/lib.rs:1:12-1:15
            `Api` @ dep/src/lib.rs:3:19-3:22
            `Api` @ helper/src/lib.rs:1:27-1:30
        "#]],
    );
}

#[test]
fn references_search_declared_spelling_for_imported_types() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_import_alias_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct Us$0er; // references("aliased type references", ReferenceQuery::all())
}

use api::User as Account;

pub fn use_it(_: Account) {}
"#,
        expect![[r#"
            // aliased type references
            `User` @ src/lib.rs:2:16-2:20
            `User` @ src/lib.rs:5:10-5:14
        "#]],
    );
}

#[test]
fn finds_self_references_for_nominal_types() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_self_references"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Us$0er; // references("type references including Self", ReferenceQuery::all())

impl User {
    pub fn make() -> Self {
        User
    }
}
"#,
        expect![[r#"
            // type references including Self
            `User` @ src/lib.rs:1:12-1:16
            `User` @ src/lib.rs:3:6-3:10
            `Self` @ src/lib.rs:4:22-4:26
            `User` @ src/lib.rs:5:9-5:13
        "#]],
    );
}

#[test]
fn finds_named_and_relative_module_references() {
    // The two `super` segments in `super::super::Root` select `parent` and the crate root.
    // Query both targets to show that each segment is resolved separately.
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "module_reference_filter"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod pa$0rent; // references("parent module references", ReferenceQuery::all())
pub struct Root;
pub fn named(_: parent::Item) {}
pub fn rooted(_: cr$0ate::Root) {} // references("crate root references", ReferenceQuery::all())
pub fn here(_: self::Root) {}

//- /src/parent.rs
pub struct Item;
pub mod child;
use self::Item as Local;
pub fn root(_: super::Root) {}

//- /src/parent/child.rs
use super::Item;
use super::super::Root;

//- /src/main.rs
use module_reference_filter::Root;
fn main() {}
"#,
        expect![[r#"
            // parent module references
            `parent` @ src/lib.rs:1:9-1:15
            `parent` @ src/lib.rs:3:17-3:23
            `self` @ src/parent.rs:3:5-3:9
            `super` @ src/parent/child.rs:1:5-1:10
            `super` @ src/parent/child.rs:2:5-2:10

            // crate root references
            `crate` @ src/lib.rs:4:18-4:23
            `self` @ src/lib.rs:5:16-5:20
            `super` @ src/parent.rs:4:16-4:21
            `super` @ src/parent/child.rs:2:12-2:17
            `module_reference_filter` @ src/main.rs:1:5-1:28
        "#]],
    );
}
