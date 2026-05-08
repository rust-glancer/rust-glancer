mod utils;

use expect_test::expect;

use self::utils::check_parse_db;

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
fn parses_shared_files_once_across_targets() {
    check_parse_db(
        r#"
        //- /Cargo.toml
        [package]
        name = "shared_root"
        version = "0.1.0"
        edition = "2024"

        [lib]
        path = "src/shared.rs"

        [[bin]]
        name = "shared-bin"
        path = "src/shared.rs"

        //- /src/shared.rs
        pub fn shared() {}
        fn main() {}
        "#,
        expect![[r#"
            packages 1 (workspace members: 1, dependencies: 0)

            package shared_root [member]
            targets
            - shared_root [lib] -> src/shared.rs
            - shared-bin [bin] -> src/shared.rs
            files
            - src/shared.rs
        "#]],
    );
}
