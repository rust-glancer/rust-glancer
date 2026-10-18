use expect_test::expect;

use super::utils::{AnalysisQuery, check_analysis_queries};

const FIXTURE: &str = r#"
//- /src/lib.rs crate:macro_navigation
pub struct User { pub id: u32 }
impl User {
    /// Account identifier.
    pub fn id(&self) -> u32 { self.id }
}
pub fn make_user() -> User { User { id: 1 } }

macro_rules! inner { ($value:expr) => { $value }; }
macro_rules! outer { ($value:expr) => { inner!($value) }; }
macro_rules! invoke { ($value:expr, $member:ident) => { $value.$member() }; }
macro_rules! duplicate { ($value:expr) => { ($value, $value) }; }
macro_rules! inner_type { ($ty:ty) => { $ty }; }
macro_rules! outer_type { ($ty:ty) => { inner_type!($ty) }; }
macro_rules! bind { ($name:ident) => { $name }; }

pub fn demo(user: User) {
    let _ = outer!($local$user);
    let _ = outer!(user.$method$id());
    let _ = outer!(user.$field$id);
    let _ = outer!($function$make_user());
    let _ = outer!($qualifier$crate::$qualified$make_user());
    let _ = outer!(User { $record$id: 1 });
    let _ = invoke!(user, $member$id);
    let _ = duplicate!(user.$repeated$id());
    let _ = outer!($nested$inner!(user));
    let _: outer_type!($type$User);
    let bind!($binding$captured) = user;
    let _ = outer!($bound_use$captured.id);
}

pub struct First;
pub struct Second;
impl First { pub fn run(&self) -> u32 { 1 } }
impl Second { pub fn run(&self) -> u32 { 2 } }
macro_rules! both { ($member:ident) => { (First.$member(), Second.$member()) }; }
pub fn repeated_meanings() { let _ = both!($ambiguous$run); }
"#;

#[test]
fn macro_argument_hover_uses_expansion_provenance() {
    check_analysis_queries(
        FIXTURE,
        &[
            AnalysisQuery::hover("local through nested expansion", "local"),
            AnalysisQuery::hover("method through nested expansion", "method"),
            AnalysisQuery::hover("field through nested expansion", "field"),
            AnalysisQuery::hover("function through nested expansion", "function"),
            AnalysisQuery::hover("qualified function", "qualified"),
            AnalysisQuery::hover("record field", "record"),
            AnalysisQuery::hover("member assembled by template", "member"),
            AnalysisQuery::hover("repeated expression", "repeated"),
            AnalysisQuery::hover("nested invocation name", "nested"),
            AnalysisQuery::hover("nested type argument", "type"),
            AnalysisQuery::hover("pattern binding", "binding"),
            AnalysisQuery::hover("use of pattern binding", "bound_use"),
            AnalysisQuery::hover("distinct expansion meanings", "ambiguous"),
        ],
        expect![[r#"
            // local through nested expansion
            range: 17:20-17:24
            block:
              kind: variable
              signature:
                let user: User

            // method through nested expansion
            range: 18:25-18:27
            block:
              kind: method
              path: macro_navigation::User::id
              signature:
                pub fn id(&self) -> u32
              docs:
                Account identifier.

            // field through nested expansion
            range: 19:25-19:27
            block:
              kind: field
              path: macro_navigation::User
              signature:
                pub id: u32

            // function through nested expansion
            range: 20:20-20:29
            block:
              kind: fn
              path: macro_navigation::make_user
              signature:
                pub fn make_user() -> User

            // qualified function
            range: 21:27-21:36
            block:
              kind: fn
              path: macro_navigation::make_user
              signature:
                pub fn make_user() -> User

            // record field
            range: 22:27-22:29
            block:
              kind: field
              path: macro_navigation::User
              signature:
                pub id: u32

            // member assembled by template
            range: 23:27-23:29
            block:
              kind: method
              path: macro_navigation::User::id
              signature:
                pub fn id(&self) -> u32
              docs:
                Account identifier.

            // repeated expression
            range: 24:29-24:31
            block:
              kind: method
              path: macro_navigation::User::id
              signature:
                pub fn id(&self) -> u32
              docs:
                Account identifier.

            // nested invocation name
            range: 25:20-25:25
            block:
              kind: macro
              path: macro_navigation::inner
              signature:
                macro_definition inner

            // nested type argument
            range: 26:24-26:28
            block:
              kind: struct
              path: macro_navigation::User
              signature:
                pub struct User {
                    pub id: u32,
                }

            // pattern binding
            range: 27:15-27:23
            block:
              kind: variable
              signature:
                let captured: User

            // use of pattern binding
            range: 28:20-28:28
            block:
              kind: variable
              signature:
                let captured: User

            // distinct expansion meanings
            range: 36:44-36:47
            block:
              kind: method
              path: macro_navigation::First::run
              signature:
                pub fn run(&self) -> u32
            block:
              kind: method
              path: macro_navigation::Second::run
              signature:
                pub fn run(&self) -> u32
        "#]],
    );
}

#[test]
fn macro_argument_goto_uses_expansion_provenance() {
    check_analysis_queries(
        FIXTURE,
        &[
            AnalysisQuery::goto("local through nested expansion", "local"),
            AnalysisQuery::goto("method through nested expansion", "method"),
            AnalysisQuery::goto("field through nested expansion", "field"),
            AnalysisQuery::goto("function through nested expansion", "function"),
            AnalysisQuery::goto("path qualifier", "qualifier"),
            AnalysisQuery::goto("qualified function", "qualified"),
            AnalysisQuery::goto("record field", "record"),
            AnalysisQuery::goto("member assembled by template", "member"),
            AnalysisQuery::goto("repeated expression", "repeated"),
            AnalysisQuery::goto("nested invocation name", "nested"),
            AnalysisQuery::goto("nested type argument", "type"),
            AnalysisQuery::goto("pattern binding", "binding"),
            AnalysisQuery::goto("use of pattern binding", "bound_use"),
            AnalysisQuery::goto("distinct expansion meanings", "ambiguous"),
            AnalysisQuery::resolve("singular selection keeps first expanded use", "ambiguous"),
        ],
        expect![[r#"
            local user @ 16:13-16:17 // local through nested expansion
            fn id @ 4:12-4:14 // method through nested expansion
            field id @ 1:23-1:25 // field through nested expansion
            fn make_user @ 6:8-6:17 // function through nested expansion
            module crate @ <root> // path qualifier
            fn make_user @ 6:8-6:17 // qualified function
            field id @ 1:23-1:25 // record field
            fn id @ 4:12-4:14 // member assembled by template
            fn id @ 4:12-4:14 // repeated expression
            macro inner @ 8:14-8:19 // nested invocation name
            struct User @ 1:12-1:16 // nested type argument
            local captured @ 27:15-27:23 // pattern binding
            local captured @ 27:15-27:23 // use of pattern binding

            // distinct expansion meanings
            fn run @ 33:21-33:24
            fn run @ 34:22-34:25

            fn run @ 33:21-33:24 // singular selection keeps first expanded use
        "#]],
    );
}

#[test]
fn macro_argument_goto_type_keeps_all_expanded_return_types() {
    check_analysis_queries(
        r#"
//- /src/lib.rs crate:macro_navigation_types
pub struct A;
pub struct B;
pub struct First;
pub struct Second;
impl First { pub fn run(&self) -> A { A } }
impl Second { pub fn run(&self) -> B { B } }
macro_rules! both { ($method:ident) => { (First.$method(), Second.$method()) }; }
macro_rules! duplicate { ($value:expr) => { ($value, $value) }; }
pub fn demo() {
    let _ = both!($direct$run);
    let _ = duplicate!(both!($nested$run));
}
"#,
        &[
            AnalysisQuery::goto_type("distinct return types", "direct"),
            AnalysisQuery::goto_type("nested repeated return types", "nested"),
        ],
        expect![[r#"
            // distinct return types
            struct A @ 1:12-1:13
            struct B @ 2:12-2:13

            // nested repeated return types
            struct A @ 1:12-1:13
            struct B @ 2:12-2:13
        "#]],
    );
}

#[test]
fn macro_arguments_keep_external_crate_provenance() {
    check_analysis_queries(
        r#"
//- /macros/src/lib.rs crate:forwarding_macros root:macros
#[macro_export]
macro_rules! inner { ($value:expr) => { $value }; }
#[macro_export]
macro_rules! forward { ($($value:tt)*) => { $crate::inner!($($value)*) }; }

//- /app/src/lib.rs crate:macro_consumer root:app deps:forwarding_macros
pub struct User { pub r#type: u32 }
impl User { pub fn id(&self) -> u32 { self.r#type } }
pub fn demo(user: User) {
    let _ = forwarding_macros::forward!($receiver$user.$method$id());
    let _ = forwarding_macros::forward!(user.$field$r#type);
}
"#,
        &[
            AnalysisQuery::hover("external macro receiver", "receiver").in_lib("macro_consumer"),
            AnalysisQuery::goto("external macro receiver", "receiver").in_lib("macro_consumer"),
            AnalysisQuery::hover("external macro method", "method").in_lib("macro_consumer"),
            AnalysisQuery::goto("external macro method", "method").in_lib("macro_consumer"),
            AnalysisQuery::hover("raw field argument", "field").in_lib("macro_consumer"),
            AnalysisQuery::goto("raw field argument", "field").in_lib("macro_consumer"),
        ],
        expect![[r#"
            // external macro receiver
            range: 4:41-4:45
            block:
              kind: variable
              signature:
                let user: User

            local user @ 3:13-3:17 // external macro receiver

            // external macro method
            range: 4:46-4:48
            block:
              kind: method
              path: macro_consumer::User::id
              signature:
                pub fn id(&self) -> u32

            fn id @ 2:20-2:22 // external macro method

            // raw field argument
            range: 5:46-5:52
            block:
              kind: field
              path: macro_consumer::User
              signature:
                pub r#type: u32

            field r#type @ 1:23-1:29 // raw field argument
        "#]],
    );
}
