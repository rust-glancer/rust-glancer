use expect_test::expect;

use super::utils::check_analysis;

#[test]
fn finds_body_symbols_at_offsets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_symbol_at
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let loc$0al: User = helper(); // symbol("symbol at declaration")
    let _again: User = loc$0al; // symbol("symbol at local path")
    let _made: User = hel$0per(); // symbol("symbol at item path")
}
"#,
        expect![[r#"
            local local @ 8:9-8:14 // symbol at declaration
            expr path local @ 9:24-9:29 // symbol at local path
            expr path helper @ 10:23-10:29 // symbol at item path
        "#]],
    );
}

#[test]
fn finds_item_and_signature_symbols_at_offsets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_signature_symbol_at
pub struct Us$0er; // symbol("symbol at struct declaration")

pub trait Na$0med { // symbol("symbol at trait declaration")
    fn describe(&self) -> User;
}

pub fn make(user: Us$0er) -> Us$1er { // symbol("symbol at parameter type"), symbol("symbol at return type")
    user
}
"#,
        expect![[r#"
            struct User @ 1:12-1:16 // symbol at struct declaration
            trait Named @ 3:11-3:16 // symbol at trait declaration
            type path User @ 7:19-7:23 // symbol at parameter type
            type path User @ 7:28-7:32 // symbol at return type
        "#]],
    );
}

#[test]
fn finds_body_macro_call_symbols_at_offsets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_macro_symbol_at
pub struct Text;

pub fn helper(value: u64) -> Text { Text }

macro_rules! make_text {
    ($value:expr) => { helper($value) };
}

pub fn use_it(input: u64) {
    let _text = make_$0text!(input); // symbol("symbol at body macro call")
}
"#,
        expect![[r#"
            macro make_text @ 10:17-10:26 // symbol at body macro call
        "#]],
    );
}

#[test]
fn finds_body_local_struct_symbols_at_offsets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_struct_symbol
pub struct User;

pub fn make() {
    struct Us$0er; // symbol("symbol at local struct declaration")
    let _local: Us$0er = Us$1er; // symbol("symbol at local type path"), symbol("symbol at local constructor")
}
"#,
        expect![[r#"
            struct User @ 4:12-4:16 // symbol at local struct declaration
            type path User @ 5:17-5:21 // symbol at local type path
            expr path User @ 5:24-5:28 // symbol at local constructor
        "#]],
    );
}

#[test]
fn finds_more_body_local_item_symbols_at_offsets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_more_body_local_symbols
pub struct GlobalId;

pub fn use_it() {
    type Al$0ias = GlobalId; // symbol("symbol at local alias declaration")
    const DE$0FAULT: Alias = GlobalId; // symbol("symbol at local const declaration")
    static CU$0RRENT: GlobalId = GlobalId; // symbol("symbol at local static declaration")
    fn hel$0per() -> Alias { // symbol("symbol at local function declaration")
        DEFAULT
    }
    enum Action {
        Sta$0rt, // symbol("symbol at local variant declaration")
        Stop,
    }

    let _typed: Al$0ias = hel$1per(); // symbol("symbol at local alias path"), symbol("symbol at local function path")
    let _default = DE$0FAULT; // symbol("symbol at local const path")
    let _current = CU$0RRENT; // symbol("symbol at local static path")
    let _action = Action::Sta$0rt; // symbol("symbol at local variant path")
}
"#,
        expect![[r#"
            type_alias Alias @ 4:10-4:15 // symbol at local alias declaration
            const DEFAULT @ 5:11-5:18 // symbol at local const declaration
            static CURRENT @ 6:12-6:19 // symbol at local static declaration
            fn helper @ 7:8-7:14 // symbol at local function declaration
            variant Start @ 11:9-11:14 // symbol at local variant declaration
            type path Alias @ 15:17-15:22 // symbol at local alias path
            expr path helper @ 15:25-15:31 // symbol at local function path
            expr path DEFAULT @ 16:20-16:27 // symbol at local const path
            expr path CURRENT @ 17:20-17:27 // symbol at local static path
            expr path Action::Start @ 18:19-18:32 // symbol at local variant path
        "#]],
    );
}
