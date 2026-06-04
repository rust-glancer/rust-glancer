use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

#[test]
fn prepares_and_renames_common_symbol_subjects() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    na$field_ref$me: Name,
}

pub struct Name;

pub fn hel$fn_ref$per(user: User) -> Name {
    user.na$field_use$me
}

pub fn use_it() {
    let loc$local_ref$al: Us$type_ref$er;
    let _again: User = local;
    let _name = helper(local);
}
"#,
        &[
            AnalysisQuery::prepare_rename("prepare type rename", "type_ref"),
            AnalysisQuery::rename("rename type", "type_ref", "Account"),
            AnalysisQuery::prepare_rename("prepare field rename", "field_use"),
            AnalysisQuery::rename("rename field", "field_use", "label"),
            AnalysisQuery::rename("rename function", "fn_ref", "build"),
            AnalysisQuery::rename("rename local", "local_ref", "selected"),
        ],
        expect![[r#"
            prepare type rename
            - `User` @ src/lib.rs:12:16-12:20

            rename type
            - target `User` @ src/lib.rs:12:16-12:20
            - `User` -> `Account` @ src/lib.rs:1:12-1:16
            - `User` -> `Account` @ src/lib.rs:7:21-7:25
            - `User` -> `Account` @ src/lib.rs:12:16-12:20
            - `User` -> `Account` @ src/lib.rs:13:17-13:21

            prepare field rename
            - `name` @ src/lib.rs:8:10-8:14

            rename field
            - target `name` @ src/lib.rs:8:10-8:14
            - `name` -> `label` @ src/lib.rs:2:5-2:9
            - `name` -> `label` @ src/lib.rs:8:10-8:14

            rename function
            - target `helper` @ src/lib.rs:7:8-7:14
            - `helper` -> `build` @ src/lib.rs:7:8-7:14
            - `helper` -> `build` @ src/lib.rs:14:17-14:23

            rename local
            - target `local` @ src/lib.rs:12:9-12:14
            - `local` -> `selected` @ src/lib.rs:12:9-12:14
            - `local` -> `selected` @ src/lib.rs:13:24-13:29
            - `local` -> `selected` @ src/lib.rs:14:24-14:29
        "#]],
    );
}

#[test]
fn rename_respects_local_binding_shadowing_same_name_function() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_method_receiver_shadow"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn bar(baz: u8) -> u8 {
    fo$fn_ref$o(baz)
}

fn foo(baz: u8) -> u8 {
    let fo$local_ref$o: Option<u8> = Some(baz);
    foo.map(|baba| baba + baba);
    baz
}
"#,
        &[
            AnalysisQuery::rename("rename function", "fn_ref", "qux"),
            AnalysisQuery::rename("rename local", "local_ref", "maybe"),
        ],
        expect![[r#"
            rename function
            - target `foo` @ src/lib.rs:2:5-2:8
            - `foo` -> `qux` @ src/lib.rs:2:5-2:8
            - `foo` -> `qux` @ src/lib.rs:5:4-5:7

            rename local
            - target `foo` @ src/lib.rs:6:9-6:12
            - `foo` -> `maybe` @ src/lib.rs:6:9-6:12
            - `foo` -> `maybe` @ src/lib.rs:7:5-7:8
        "#]],
    );
}

#[test]
fn renames_supported_item_kinds_from_representative_uses() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_supported_item_kinds"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub enum Status {
    Ready,
    Done,
}

pub union Packet {
    raw: u32,
}

pub trait Service {}

pub type Handle = User;

pub const LIMIT: u32 = 3;
pub static CURRENT: u32 = LIMIT;

impl User {
    pub fn render(&self) -> u32 {
        LIMIT
    }
}

impl Ser$trait_type_ref$vice for User {}

pub fn use_it(user: User, status: Sta$enum_type_ref$tus, packet: Pac$union_type_ref$ket) -> Han$alias_type_ref$dle {
    let _status = Status::Rea$variant_use$dy;
    let _packet: Packet = packet;
    let _limit = LI$const_use$MIT;
    let _current = CU$static_use$RRENT;
    let _rendered = user.ren$method_use$der();
    user.render()
}
"#,
        &[
            AnalysisQuery::rename("rename enum", "enum_type_ref", "Phase"),
            AnalysisQuery::rename("rename enum variant", "variant_use", "Active"),
            AnalysisQuery::rename("rename union", "union_type_ref", "PacketData"),
            AnalysisQuery::rename("rename trait", "trait_type_ref", "Renderable"),
            AnalysisQuery::rename("rename type alias", "alias_type_ref", "Output"),
            AnalysisQuery::rename("rename const", "const_use", "MAX"),
            AnalysisQuery::rename("rename static", "static_use", "GLOBAL"),
            AnalysisQuery::rename("rename method", "method_use", "draw"),
        ],
        expect![[r#"
            rename enum
            - target `Status` @ src/lib.rs:27:35-27:41
            - `Status` -> `Phase` @ src/lib.rs:3:10-3:16
            - `Status` -> `Phase` @ src/lib.rs:27:35-27:41
            - `Status` -> `Phase` @ src/lib.rs:28:19-28:25

            rename enum variant
            - target `Ready` @ src/lib.rs:28:27-28:32
            - `Ready` -> `Active` @ src/lib.rs:4:5-4:10
            - `Ready` -> `Active` @ src/lib.rs:28:27-28:32

            rename union
            - target `Packet` @ src/lib.rs:27:51-27:57
            - `Packet` -> `PacketData` @ src/lib.rs:8:11-8:17
            - `Packet` -> `PacketData` @ src/lib.rs:27:51-27:57
            - `Packet` -> `PacketData` @ src/lib.rs:29:18-29:24

            rename trait
            - target `Service` @ src/lib.rs:25:6-25:13
            - `Service` -> `Renderable` @ src/lib.rs:12:11-12:18
            - `Service` -> `Renderable` @ src/lib.rs:25:6-25:13

            rename type alias
            - target `Handle` @ src/lib.rs:27:62-27:68
            - `Handle` -> `Output` @ src/lib.rs:14:10-14:16
            - `Handle` -> `Output` @ src/lib.rs:27:62-27:68

            rename const
            - target `LIMIT` @ src/lib.rs:30:18-30:23
            - `LIMIT` -> `MAX` @ src/lib.rs:16:11-16:16
            - `LIMIT` -> `MAX` @ src/lib.rs:17:27-17:32
            - `LIMIT` -> `MAX` @ src/lib.rs:21:9-21:14
            - `LIMIT` -> `MAX` @ src/lib.rs:30:18-30:23

            rename static
            - target `CURRENT` @ src/lib.rs:31:20-31:27
            - `CURRENT` -> `GLOBAL` @ src/lib.rs:17:12-17:19
            - `CURRENT` -> `GLOBAL` @ src/lib.rs:31:20-31:27

            rename method
            - target `render` @ src/lib.rs:32:26-32:32
            - `render` -> `draw` @ src/lib.rs:20:12-20:18
            - `render` -> `draw` @ src/lib.rs:32:26-32:32
            - `render` -> `draw` @ src/lib.rs:33:10-33:16
        "#]],
    );
}

#[test]
fn rename_edits_multiple_modules_and_use_paths() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_multi_module"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod model;
pub mod use_sites;

pub use model::User;

pub fn make(user: User) -> model::User {
    use_sites::wrap(user)
}

//- /src/model.rs
pub struct User {
    pub id: u32,
}

pub fn clone_user(user: User) -> User {
    user
}

//- /src/use_sites.rs
use crate::model::Us$multi_use_path$er;

pub fn wrap(user: User) -> crate::model::User {
    crate::model::clone_user(user)
}
"#,
        &[AnalysisQuery::rename(
            "rename use-path type across modules",
            "multi_use_path",
            "Account",
        )],
        expect![[r#"
            rename use-path type across modules
            - target `User` @ src/use_sites.rs:1:19-1:23
            - `User` -> `Account` @ src/lib.rs:4:16-4:20
            - `User` -> `Account` @ src/lib.rs:6:19-6:23
            - `User` -> `Account` @ src/lib.rs:6:35-6:39
            - `User` -> `Account` @ src/model.rs:1:12-1:16
            - `User` -> `Account` @ src/model.rs:5:25-5:29
            - `User` -> `Account` @ src/model.rs:5:34-5:38
            - `User` -> `Account` @ src/use_sites.rs:1:19-1:23
            - `User` -> `Account` @ src/use_sites.rs:3:19-3:23
            - `User` -> `Account` @ src/use_sites.rs:3:42-3:46
        "#]],
    );
}

#[test]
fn rename_respects_nested_body_shadowing() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_nested_shadowing"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Value;

pub fn make() -> Value {
    fn target() -> Value {
        Value
    }

    let _first = tar$outer_fn_use$get();

    {
        let tar$inner_let_decl$get = Some(Value);
        let _mapped = target.map(|tar$closure_param_decl$get| target);
    }

    {
        fn tar$inner_fn_decl$get() -> Value {
            Value
        }
        let _value = target();
    }

    let _second = target();
    Value
}
"#,
        &[
            AnalysisQuery::rename("rename outer function", "outer_fn_use", "compute"),
            AnalysisQuery::rename("rename inner local", "inner_let_decl", "maybe"),
            AnalysisQuery::rename("rename closure parameter", "closure_param_decl", "item"),
            AnalysisQuery::rename("rename inner function", "inner_fn_decl", "local_compute"),
        ],
        expect![[r#"
            rename outer function
            - target `target` @ src/lib.rs:8:18-8:24
            - `target` -> `compute` @ src/lib.rs:4:8-4:14
            - `target` -> `compute` @ src/lib.rs:8:18-8:24
            - `target` -> `compute` @ src/lib.rs:22:19-22:25

            rename inner local
            - target `target` @ src/lib.rs:11:13-11:19
            - `target` -> `maybe` @ src/lib.rs:11:13-11:19
            - `target` -> `maybe` @ src/lib.rs:12:23-12:29

            rename closure parameter
            - target `target` @ src/lib.rs:12:35-12:41
            - `target` -> `item` @ src/lib.rs:12:35-12:41
            - `target` -> `item` @ src/lib.rs:12:43-12:49

            rename inner function
            - target `target` @ src/lib.rs:16:12-16:18
            - `target` -> `local_compute` @ src/lib.rs:16:12-16:18
            - `target` -> `local_compute` @ src/lib.rs:19:22-19:28
        "#]],
    );
}

#[test]
fn rename_field_in_record_literals_patterns_and_accesses() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_record_fields"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub na$field_decl$me: u8,
    pub age: u8,
}

pub fn use_it(user: User, name: u8, age: u8) {
    let User { na$field_pattern$me: extracted, age } = user;
    let rebuilt = User { na$field_literal$me: name, age };
    let _field = rebuilt.na$field_access$me;
    let _local = name;
}
"#,
        &[
            AnalysisQuery::prepare_rename("prepare pattern field rename", "field_pattern"),
            AnalysisQuery::rename("rename from literal field", "field_literal", "title"),
            AnalysisQuery::rename("rename record field", "field_access", "label"),
        ],
        expect![[r#"
            prepare pattern field rename
            - `name` @ src/lib.rs:7:16-7:20

            rename from literal field
            - target `name` @ src/lib.rs:8:26-8:30
            - `name` -> `title` @ src/lib.rs:2:9-2:13
            - `name` -> `title` @ src/lib.rs:7:16-7:20
            - `name` -> `title` @ src/lib.rs:8:26-8:30
            - `name` -> `title` @ src/lib.rs:9:26-9:30

            rename record field
            - target `name` @ src/lib.rs:9:26-9:30
            - `name` -> `label` @ src/lib.rs:2:9-2:13
            - `name` -> `label` @ src/lib.rs:7:16-7:20
            - `name` -> `label` @ src/lib.rs:8:26-8:30
            - `name` -> `label` @ src/lib.rs:9:26-9:30
        "#]],
    );
}

#[test]
fn rename_expands_record_shorthand_occurrences() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_rename_record_shorthand"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User {
    pub na$field_decl$me: u8,
    pub age: u8,
}

pub fn use_it(source: User, na$param_decl$me: u8, age: u8) -> u8 {
    let direct = User { na$literal_shorthand$me, age };
    let explicit = User { name: name, age };
    let User { na$pattern_shorthand$me, age: other } = source;
    let _field = source.na$field_access$me;
    name + direct.name + explicit.name + other
}
"#,
        &[
            AnalysisQuery::rename("rename field with shorthand", "field_decl", "title"),
            AnalysisQuery::rename(
                "rename parameter through shorthand literal",
                "literal_shorthand",
                "label",
            ),
            AnalysisQuery::rename(
                "rename pattern shorthand binding",
                "pattern_shorthand",
                "selected",
            ),
        ],
        expect![[r#"
            rename field with shorthand
            - target `name` @ src/lib.rs:2:9-2:13
            - `name` -> `title` @ src/lib.rs:2:9-2:13
            - `name` -> `title: name` @ src/lib.rs:7:25-7:29
            - `name` -> `title` @ src/lib.rs:8:27-8:31
            - `name` -> `title: name` @ src/lib.rs:9:16-9:20
            - `name` -> `title` @ src/lib.rs:10:25-10:29
            - `name` -> `title` @ src/lib.rs:11:19-11:23
            - `name` -> `title` @ src/lib.rs:11:35-11:39

            rename parameter through shorthand literal
            - target `name` @ src/lib.rs:7:25-7:29
            - `name` -> `label` @ src/lib.rs:6:29-6:33
            - `name` -> `name: label` @ src/lib.rs:7:25-7:29
            - `name` -> `label` @ src/lib.rs:8:33-8:37

            rename pattern shorthand binding
            - target `name` @ src/lib.rs:9:16-9:20
            - `name` -> `name: selected` @ src/lib.rs:9:16-9:20
            - `name` -> `selected` @ src/lib.rs:11:5-11:9
        "#]],
    );
}

#[test]
fn rejects_unsupported_rename_targets() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reject_rename"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod us$module_ref$er {
    pub struct User;
}

use user::User as Acc$alias_ref$ount;

pub fn use_it(_account: Account) {}
"#,
        &[
            AnalysisQuery::prepare_rename("reject module rename", "module_ref"),
            AnalysisQuery::prepare_rename("reject alias rename", "alias_ref"),
        ],
        expect![[r#"
            reject module rename
            - <none>

            reject alias rename
            - <none>
        "#]],
    );
}
