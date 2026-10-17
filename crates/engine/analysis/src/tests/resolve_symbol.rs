use expect_test::expect;

use super::utils::check_analysis;

#[test]
fn resolves_body_symbols_to_navigation_targets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_resolve_symbol
pub struct User;

pub fn helper() -> User {
    User
}

pub fn use_it() {
    let local: User = helper();
    let _again: User = loc$0al; // resolve("resolve local")
    let _made: User = hel$0per(); // resolve("resolve item")
}
"#,
        expect![[r#"
            local local @ 8:9-8:14 // resolve local
            fn helper @ 3:8-3:14 // resolve item
        "#]],
    );
}

#[test]
fn resolves_body_macro_call_symbols_to_navigation_targets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_macro_resolve_symbol
pub struct Text;

pub fn helper(value: u64) -> Text { Text }

macro_rules! make_text {
    ($value:expr) => { helper($value) };
}

pub fn use_it(input: u64) {
    let _text = make_$0text!(input); // resolve("resolve body macro call")
}
"#,
        expect![[r#"
            macro make_text @ 5:14-5:23 // resolve body macro call
        "#]],
    );
}

#[test]
fn resolves_body_local_struct_symbols_to_navigation_targets() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_struct_resolve
pub struct User;

pub fn make() {
    struct User;
    let _local: Us$0er = Us$1er; // resolve("resolve local type path"), resolve("resolve local constructor")
}
"#,
        expect![[r#"
            struct User @ 4:12-4:16 // resolve local type path
            struct User @ 4:12-4:16 // resolve local constructor
        "#]],
    );
}
