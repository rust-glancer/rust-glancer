use expect_test::expect;

use super::utils::{
    AnalysisQuery, check_analysis_queries, check_analysis_queries_with_fake_sysroot,
    check_analysis_with_fake_sysroot,
};

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

#[test]
fn builtin_macro_arguments_keep_expression_meanings() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:builtin_navigation
use std::format_args as arguments;
pub struct User { pub id: u32 }
pub struct Profile;
impl User {
    pub fn profile(&self) -> Profile { Profile }
    pub fn valid(&self) -> bool { true }
}
pub fn identity<T, U>(value: T) -> T { value }
macro_rules! forward { ($($value:tt)*) => { $($value)* }; }
pub fn demo(user: User) {
    let _ = format!("{}", $receiver$user.id);
    let _ = format!("{}", user.$field$id);
    let _ = format!("{named}", named = user.$named$id);
    assert!(user.$condition$valid(), "{}", user.$message$id);
    let _ = forward!(format!("{}", forward!(user.$nested$id)));
    let _ = format!("{:?}", user.$method$profile());
    let _ = format_args_nl!("{}", user.$newline$id);
    let _ = arguments!("{}", $alias$user.id);
    let _ = format!("{:?}", (|left: User, right: User| left.$closure$id)(user, user));
    let _ = format!("{}", identity::<User, Profile>(user).$generic$id);
}
"#,
        &[
            AnalysisQuery::hover("format receiver", "receiver").in_lib("builtin_navigation"),
            AnalysisQuery::goto("format receiver", "receiver").in_lib("builtin_navigation"),
            AnalysisQuery::hover("format field", "field").in_lib("builtin_navigation"),
            AnalysisQuery::goto("format field", "field").in_lib("builtin_navigation"),
            AnalysisQuery::goto("named format value", "named").in_lib("builtin_navigation"),
            AnalysisQuery::hover("assert condition", "condition").in_lib("builtin_navigation"),
            AnalysisQuery::goto("assert condition", "condition").in_lib("builtin_navigation"),
            AnalysisQuery::goto("assert message value", "message").in_lib("builtin_navigation"),
            AnalysisQuery::goto("nested macro value", "nested").in_lib("builtin_navigation"),
            AnalysisQuery::hover("format method", "method").in_lib("builtin_navigation"),
            AnalysisQuery::goto("format method", "method").in_lib("builtin_navigation"),
            AnalysisQuery::goto_type("format method return type", "method")
                .in_lib("builtin_navigation"),
            AnalysisQuery::goto("newline format value", "newline").in_lib("builtin_navigation"),
            AnalysisQuery::goto("aliased builtin value", "alias").in_lib("builtin_navigation"),
            AnalysisQuery::goto("closure with comma-separated parameters", "closure")
                .in_lib("builtin_navigation"),
            AnalysisQuery::goto("call with comma-separated type arguments", "generic")
                .in_lib("builtin_navigation"),
        ],
        expect![[r#"
            // format receiver
            range: 11:27-11:31
            block:
              kind: variable
              signature:
                let user: User

            local user @ 10:13-10:17 // format receiver

            // format field
            range: 12:32-12:34
            block:
              kind: field
              path: builtin_navigation::User
              signature:
                pub id: u32

            field id @ 2:23-2:25 // format field
            field id @ 2:23-2:25 // named format value

            // assert condition
            range: 14:18-14:23
            block:
              kind: method
              path: builtin_navigation::User::valid
              signature:
                pub fn valid(&self) -> bool

            fn valid @ 6:12-6:17 // assert condition
            field id @ 2:23-2:25 // assert message value
            field id @ 2:23-2:25 // nested macro value

            // format method
            range: 16:34-16:41
            block:
              kind: method
              path: builtin_navigation::User::profile
              signature:
                pub fn profile(&self) -> Profile

            fn profile @ 5:12-5:19 // format method
            struct Profile @ 3:12-3:19 // format method return type
            field id @ 2:23-2:25 // newline format value
            local user @ 10:13-10:17 // aliased builtin value
            field id @ 2:23-2:25 // closure with comma-separated parameters
            field id @ 2:23-2:25 // call with comma-separated type arguments
        "#]],
    );
}

#[test]
fn builtin_assert_constrains_its_condition_and_preserves_shadowing() {
    check_analysis_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:builtin_inference
pub fn make<T>() -> T { loop {} }
pub struct User;
pub fn demo() {
    let value = make();
    let assertion$0 = assert!(val$1ue); // ty("assert result").in_lib("builtin_inference"), ty("assert condition expectation").in_lib("builtin_inference")
    let arguments$0 = format_args!("{}", value); // ty("format result").in_lib("builtin_inference")
}
pub mod custom {
    macro_rules! assert { ($value:expr) => { $value }; }
    pub fn demo(user: super::User) {
        let value$0 = assert!(user); // ty("user-defined assert result").in_lib("builtin_inference")
    }
}
"#,
        expect![[r#"
            () // assert result
            bool // assert condition expectation
            nominal struct core[lib]::crate::fmt::Arguments // format result
            nominal struct builtin_inference[lib]::crate::User // user-defined assert result
        "#]],
    );
}

#[test]
fn format_captures_use_lexical_scope_and_literal_source_ranges() {
    check_analysis_queries_with_fake_sysroot(
        r###"
//- /src/lib.rs crate:format_captures
use std::format_args as arguments;
pub struct User;
impl User {
    pub fn show(&self) { let _ = format!("{$self_capture$self:?}"); }
}
macro_rules! forward { ($value:expr) => { $value }; }
pub fn demo(user: User, café: u32, number: f64, width: usize, precision: usize, r#type: u8) {
    let _ = format_args!("hello {$local$user:?}");
    let _ = arguments!(r##"{{escaped}} {$raw$user:?}"##);
    let _ = format_args!("\t☕ {$unicode$caf\u{e9}}");
    let _ = format_args!("\u{7b}$escaped_braces$user\u{7d}");
    let _ = format_args!("continued\
        {$continued$user  }");
    let _ = format_args!("{number:>$width$width$.$precision$precision$}");
    let _ = format!("{label} {$named_sibling$user:?}", label = 42);
    let _ = format!("{$keyword$type}");
    assert!(true, "bad {$assert_capture$user:?}");
    let _ = forward!(format!("{$nested$user:?}"));
    let _ = format_args_nl!("{$newline$user:?}");
    {
        let user = 10u64;
        let _ = format!("{$shadow$user}");
    }
}
"###,
        &[
            AnalysisQuery::hover("implicit local", "local").in_lib("format_captures"),
            AnalysisQuery::goto("implicit local", "local").in_lib("format_captures"),
            AnalysisQuery::goto_type("implicit local type", "local").in_lib("format_captures"),
            AnalysisQuery::goto("raw string and escaped braces", "raw").in_lib("format_captures"),
            AnalysisQuery::hover("escaped Unicode identifier", "unicode").in_lib("format_captures"),
            AnalysisQuery::goto("escaped Unicode identifier", "unicode").in_lib("format_captures"),
            AnalysisQuery::goto("escaped opening and closing braces", "escaped_braces")
                .in_lib("format_captures"),
            AnalysisQuery::hover("line continuation and trailing whitespace", "continued")
                .in_lib("format_captures"),
            AnalysisQuery::hover("width capture", "width").in_lib("format_captures"),
            AnalysisQuery::goto("width capture", "width").in_lib("format_captures"),
            AnalysisQuery::goto("precision capture", "precision").in_lib("format_captures"),
            AnalysisQuery::goto("capture beside explicit named argument", "named_sibling")
                .in_lib("format_captures"),
            AnalysisQuery::goto("raw identifier binding", "keyword").in_lib("format_captures"),
            AnalysisQuery::goto("assert message capture", "assert_capture")
                .in_lib("format_captures"),
            AnalysisQuery::goto("nested macro capture", "nested").in_lib("format_captures"),
            AnalysisQuery::goto("newline formatting capture", "newline").in_lib("format_captures"),
            AnalysisQuery::hover("shadowed local capture", "shadow").in_lib("format_captures"),
            AnalysisQuery::goto("shadowed local capture", "shadow").in_lib("format_captures"),
            AnalysisQuery::hover("self capture", "self_capture").in_lib("format_captures"),
        ],
        expect![[r#"
            // implicit local
            range: 8:34-8:38
            block:
              kind: variable
              signature:
                let user: User

            local user @ 7:13-7:17 // implicit local
            struct User @ 2:12-2:16 // implicit local type
            local user @ 7:13-7:17 // raw string and escaped braces

            // escaped Unicode identifier
            range: 10:34-10:43
            block:
              kind: variable
              signature:
                let café: u32

            local café @ 7:25-7:30 // escaped Unicode identifier
            local user @ 7:13-7:17 // escaped opening and closing braces

            // line continuation and trailing whitespace
            range: 13:10-13:14
            block:
              kind: variable
              signature:
                let user: User

            // width capture
            range: 14:36-14:41
            block:
              kind: variable
              signature:
                let width: usize

            local width @ 7:50-7:55 // width capture
            local precision @ 7:64-7:73 // precision capture
            local user @ 7:13-7:17 // capture beside explicit named argument
            local r#type @ 7:82-7:88 // raw identifier binding
            local user @ 7:13-7:17 // assert message capture
            local user @ 7:13-7:17 // nested macro capture
            local user @ 7:13-7:17 // newline formatting capture

            // shadowed local capture
            range: 22:27-22:31
            block:
              kind: variable
              signature:
                let user: u64

            local user @ 21:13-21:17 // shadowed local capture

            // self capture
            range: 4:44-4:48
            block:
              kind: variable
              signature:
                let self: &User
        "#]],
    );
}

#[test]
fn format_captures_keep_each_nested_expansion_meaning() {
    check_analysis_queries_with_fake_sysroot(
        r#"
//- /macros/src/lib.rs crate:capture_macros root:macros
#[macro_export]
macro_rules! inner {
    ($name:ident, $template:literal) => {{
        let $name = $crate::First;
        let first = format_args!($template);
        let $name = $crate::Second;
        let second = format_args!($template);
        (first, second)
    }};
}
#[macro_export]
macro_rules! outer { ($($input:tt)*) => { $crate::inner!($($input)*) }; }
pub struct First;
pub struct Second;

//- /app/src/lib.rs crate:capture_consumer root:app deps:capture_macros
pub fn demo() {
    let _ = capture_macros::outer!(value, "{$capture$value:?}");
}
"#,
        &[
            AnalysisQuery::hover("distinct capture meanings", "capture").in_lib("capture_consumer"),
            AnalysisQuery::goto("capture binding", "capture").in_lib("capture_consumer"),
            AnalysisQuery::goto_type("distinct capture types", "capture")
                .in_lib("capture_consumer"),
        ],
        expect![[r#"
            // distinct capture meanings
            range: 2:45-2:50
            block:
              kind: variable
              signature:
                let value: First
            block:
              kind: variable
              signature:
                let value: Second

            local value @ 2:36-2:41 // capture binding

            // distinct capture types
            struct First @ 13:12-13:17
            struct Second @ 14:12-14:18
        "#]],
    );
}
