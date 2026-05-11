use expect_test::expect;

use super::utils::{DocumentSymbolsQuery, check_document_symbols};

#[test]
fn outlines_semantic_and_body_local_items() {
    check_document_symbols(
        r#"
//- /Cargo.toml
[package]
name = "analysis_document_symbols"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const DEFAULT_ID: u32 = 1;
pub static LIMIT: u8 = 4;
pub type UserId = u32;

pub struct User {
    pub id: UserId,
    name: UserId,
}

pub union Raw {
    bytes: [u8; 4],
    value: u32,
}

pub enum State {
    Empty,
    Loaded { user: User },
    Pair(User, User),
}

pub trait Named {
    const KIND: &'static str;
    type Output;
    fn name(&self) -> Self::Output;
}

impl User {
    pub fn new(id: UserId) -> Self {
        struct Local {
            inner: UserId,
        }

        impl Local {
            fn inner(&self) -> UserId {
                self.inner
            }
        }

        missing()
    }
}

pub fn make() {
    struct Temp {
        value: UserId,
    }
}
"#,
        DocumentSymbolsQuery::new("document symbols", "/src/lib.rs"),
        expect![[r#"
            document symbols
            - const DEFAULT_ID @ 1:1-1:31 selection 1:11-1:21
            - static LIMIT @ 2:1-2:26 selection 2:12-2:17
            - type_alias UserId @ 3:1-3:23 selection 3:10-3:16
            - struct User @ 5:1-8:2 selection 5:12-5:16
              - field id @ 6:9-6:11
              - field name @ 7:5-7:9
            - union Raw @ 10:1-13:2 selection 10:11-10:14
              - field bytes @ 11:5-11:10
              - field value @ 12:5-12:10
            - enum State @ 15:1-19:2 selection 15:10-15:15
              - variant Empty @ 16:5-16:10
              - variant Loaded @ 17:5-17:26 selection 17:5-17:11
                - field user @ 17:14-17:18
              - variant Pair @ 18:5-18:21 selection 18:5-18:9
                - field #0 @ 18:10-18:14
                - field #1 @ 18:16-18:20
            - trait Named @ 21:1-25:2 selection 21:11-21:16
              - const KIND @ 22:5-22:30 selection 22:11-22:15
              - type_alias Output @ 23:5-23:17 selection 23:10-23:16
              - method name @ 24:5-24:36 selection 24:8-24:12
            - impl User @ 27:1-41:2
              - method new @ 28:5-40:6 selection 28:12-28:15
                - struct Local @ 29:9-31:10 selection 29:16-29:21
                  - field inner @ 30:13-30:18
                - impl Local @ 33:9-37:10
                  - method inner @ 34:13-36:14 selection 34:16-34:21
            - fn make @ 43:1-47:2 selection 43:8-43:12
              - struct Temp @ 44:5-46:6 selection 44:12-44:16
                - field value @ 45:9-45:14
        "#]],
    );
}

#[test]
fn outlines_bin_owned_out_of_line_module_files() {
    check_document_symbols(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bin_document_symbols"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "analysis-bin"
path = "src/main.rs"

//- /src/lib.rs
pub struct Api;

//- /src/main.rs
mod cli;

fn main() {}

//- /src/cli.rs
mod inner;

pub struct CliRoot;

//- /src/cli/inner.rs
pub struct Nested {
    pub value: u32,
}

pub fn run() {}
"#,
        DocumentSymbolsQuery::new("nested module document symbols", "/src/cli/inner.rs")
            .in_bin("analysis_bin_document_symbols"),
        expect![[r#"
            nested module document symbols
            - struct Nested @ 1:1-3:2 selection 1:12-1:18
              - field value @ 2:9-2:14
            - fn run @ 5:1-5:16 selection 5:8-5:11
        "#]],
    );
}

#[test]
fn outlines_module_declarations_and_inline_module_children() {
    check_document_symbols(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_document_symbols"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod api {
    pub struct User {
        pub id: u32,
    }

    mod nested {
        pub fn run() {}
    }
}

mod generated;

pub struct Root;

//- /src/generated.rs
pub struct Generated;
"#,
        DocumentSymbolsQuery::new("module document symbols", "/src/lib.rs"),
        expect![[r#"
            module document symbols
            - module api @ 1:1-9:2 selection 1:5-1:8
              - struct User @ 2:5-4:6 selection 2:16-2:20
                - field id @ 3:13-3:15
              - module nested @ 6:5-8:6 selection 6:9-6:15
                - fn run @ 7:9-7:24 selection 7:16-7:19
            - module generated @ 11:1-11:15 selection 11:5-11:14
            - struct Root @ 13:1-13:17 selection 13:12-13:16
        "#]],
    );

    check_document_symbols(
        r#"
//- /Cargo.toml
[package]
name = "analysis_module_document_symbols"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod api {
    pub struct User {
        pub id: u32,
    }

    mod nested {
        pub fn run() {}
    }
}

mod generated;

pub struct Root;

//- /src/generated.rs
pub struct Generated;
"#,
        DocumentSymbolsQuery::new("out-of-line module file symbols", "/src/generated.rs"),
        expect![[r#"
            out-of-line module file symbols
            - struct Generated @ 1:1-1:22 selection 1:12-1:21
        "#]],
    );
}
