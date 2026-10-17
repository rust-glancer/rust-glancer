use expect_test::expect;

use super::utils::check_workspace_symbols;

#[test]
fn searches_symbols_across_workspace_targets_and_dependencies() {
    check_workspace_symbols(
        r#"
//- /crates/dep/src/lib.rs crate:dep root:crates/dep
pub struct DepApi {
    pub dep_api_field: u32,
}

pub trait DepApiTrait {
    fn dep_api_method(&self);
}

//- /crates/app/src/lib.rs crate:app root:crates/app deps:dep
pub struct Api {
    pub api_field: u32,
}

impl Api {
    pub fn api_method(&self) {}
}

pub enum ApiState {
    ApiReady,
}

//- /crates/app/src/main.rs crate:app root:crates/app deps:dep kind:bin name:app-bin
struct CliApi;

impl CliApi {
    fn api_cli(&self) {}
}
"#,
        "api",
        expect![[r#"
            // workspace symbols `api`
            struct Api @ app[lib] src/lib.rs:1:12-1:15
            method api_cli in impl CliApi @ app[bin] src/main.rs:4:8-4:15
            field api_field in Api @ app[lib] src/lib.rs:2:9-2:18
            method api_method in impl Api @ app[lib] src/lib.rs:6:12-6:22
            variant ApiReady in ApiState @ app[lib] src/lib.rs:10:5-10:13
            enum ApiState @ app[lib] src/lib.rs:9:10-9:18
            struct CliApi @ app[bin] src/main.rs:1:8-1:14
            field dep_api_field in DepApi @ dep[lib] src/lib.rs:2:9-2:22
            method dep_api_method in trait DepApiTrait @ dep[lib] src/lib.rs:6:8-6:22
            struct DepApi @ dep[lib] src/lib.rs:1:12-1:18
            trait DepApiTrait @ dep[lib] src/lib.rs:5:11-5:22
        "#]],
    );
}

#[test]
fn searches_module_declarations() {
    check_workspace_symbols(
        r#"
//- /src/lib.rs crate:analysis_module_workspace_symbols
mod api;

mod inline_api {
    pub fn run() {}
}

//- /src/api.rs
pub struct Endpoint;
"#,
        "api",
        expect![[r#"
            // workspace symbols `api`
            module api @ analysis_module_workspace_symbols[lib] src/lib.rs:1:5-1:8
            module inline_api @ analysis_module_workspace_symbols[lib] src/lib.rs:3:5-3:15
        "#]],
    );
}
