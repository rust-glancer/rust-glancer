mod utils;

use expect_test::expect;

use self::utils::{check_parse_db, check_parse_db_after_module_discovery};

#[test]
fn dumps_workspace_packages_targets_and_dependencies() {
    check_parse_db(
        r#"
        //- /Cargo.toml
        [workspace]
        members = ["app"]
        exclude = ["helper"]
        resolver = "3"

        //- /app/Cargo.toml
        [package]
        name = "app"
        version = "0.1.0"
        edition = "2024"

        [dependencies]
        helper = { path = "../helper" }

        [lib]
        path = "src/lib.rs"

        [[bin]]
        name = "app-cli"
        path = "src/main.rs"

        [[test]]
        name = "smoke"
        path = "tests/smoke.rs"

        //- /app/src/lib.rs
        pub struct App;

        //- /app/src/main.rs
        fn main() {}

        //- /app/tests/smoke.rs
        #[test]
        fn smoke() {}

        //- /helper/Cargo.toml
        [package]
        name = "helper"
        version = "0.1.0"
        edition = "2024"

        [lib]
        path = "src/lib.rs"

        [[bin]]
        name = "helper-cli"
        path = "src/main.rs"

        //- /helper/src/lib.rs
        pub struct Helper;

        //- /helper/src/main.rs
        fn main() {}
        "#,
        expect![[r#"
            packages 2 (workspace members: 1, dependencies: 1)

            package app [member]
            targets
            - app [lib] -> app/src/lib.rs
            - app-cli [bin] -> app/src/main.rs
            - smoke [test] -> app/tests/smoke.rs
            files
            - app/src/lib.rs
            - app/src/main.rs
            - app/tests/smoke.rs

            package helper [dependency]
            targets
            - helper [lib] -> helper/src/lib.rs
            files
            - helper/src/lib.rs
        "#]],
    );
}

#[test]
fn module_discovery_parses_reachable_out_of_line_files() {
    check_parse_db_after_module_discovery(
        r#"
        //- /Cargo.toml
        [package]
        name = "module_discovery"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod flat;
        pub mod nested;
        pub mod inline {
            pub mod child;
        }
        pub mod r#type;
        pub mod r#async {
            pub mod r#match;
        }
        #[path = "generated/api.rs"]
        pub mod api;
        pub mod missing;

        //- /src/flat.rs
        pub struct Flat;

        //- /src/nested/mod.rs
        pub struct Nested;

        //- /src/inline/child.rs
        pub struct Child;

        //- /src/type.rs
        pub struct RawType;

        //- /src/async/match.rs
        pub struct RawMatch;

        //- /src/generated/api.rs
        pub struct Api;
        "#,
        expect![[r#"
            packages 1 (workspace members: 1, dependencies: 0)

            package module_discovery [member]
            targets
            - module_discovery [lib] -> src/lib.rs
            files
            - src/async/match.rs
            - src/flat.rs
            - src/generated/api.rs
            - src/inline/child.rs
            - src/lib.rs
            - src/nested/mod.rs
            - src/type.rs
        "#]],
    );
}

#[test]
fn module_discovery_carries_module_path_provenance() {
    check_parse_db_after_module_discovery(
        r#"
        //- /Cargo.toml
        [package]
        name = "module_context_discovery"
        version = "0.1.0"
        edition = "2024"

        [lib]
        path = "src/tool.rs"

        //- /src/tool.rs
        pub mod flat;

        #[path = "models"]
        pub mod inline {
            pub mod nested;
        }

        //- /src/flat.rs
        #[path = "sibling.rs"]
        pub mod sibling;

        //- /src/sibling.rs
        pub struct Sibling;

        //- /src/models/nested.rs
        pub struct Nested;
        "#,
        expect![[r#"
            packages 1 (workspace members: 1, dependencies: 0)

            package module_context_discovery [member]
            targets
            - module_context_discovery [lib] -> src/tool.rs
            files
            - src/flat.rs
            - src/models/nested.rs
            - src/sibling.rs
            - src/tool.rs
        "#]],
    );
}

#[test]
fn deduplicates_package_files_across_target_roots_and_module_discovery() {
    check_parse_db_after_module_discovery(
        r#"
        //- /Cargo.toml
        [package]
        name = "shared_discovery"
        version = "0.1.0"
        edition = "2024"

        [lib]
        path = "src/lib.rs"

        [[bin]]
        name = "shared-discovery"
        path = "src/main.rs"

        [[bin]]
        name = "shared-root-alias"
        path = "src/lib.rs"

        //- /src/lib.rs
        pub mod shared;

        //- /src/main.rs
        mod shared;

        fn main() {}

        //- /src/shared.rs
        pub struct Shared;
        "#,
        expect![[r#"
            packages 1 (workspace members: 1, dependencies: 0)

            package shared_discovery [member]
            targets
            - shared_discovery [lib] -> src/lib.rs
            - shared-discovery [bin] -> src/main.rs
            - shared-root-alias [bin] -> src/lib.rs
            files
            - src/lib.rs
            - src/main.rs
            - src/shared.rs
        "#]],
    );
}

#[test]
fn module_discovery_terminates_on_module_cycles() {
    check_parse_db_after_module_discovery(
        r#"
        //- /Cargo.toml
        [package]
        name = "cycle_discovery"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        pub mod a;

        //- /src/a/mod.rs
        #[path = "../lib.rs"]
        pub mod root_again;
        "#,
        expect![[r#"
            packages 1 (workspace members: 1, dependencies: 0)

            package cycle_discovery [member]
            targets
            - cycle_discovery [lib] -> src/lib.rs
            files
            - src/a/mod.rs
            - src/lib.rs
        "#]],
    );
}
