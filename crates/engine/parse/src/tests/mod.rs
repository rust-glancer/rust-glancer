mod utils;

use expect_test::expect;

use self::utils::{check_parse_db, check_parse_db_after_module_discovery};

#[test]
fn reallocated_parse_releases_old_sources_and_preserves_file_lookups() {
    let fixture = test_fixture::fixture_crate(
        r#"
        //- /Cargo.toml
        [package]
        name = "catalog"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        mod shared;

        //- /src/main.rs
        mod shared;
        fn main() {}

        //- /src/shared.rs
        pub struct Catalog;
        "#,
    );
    let workspace = rg_workspace::WorkspaceMetadata::for_tests(
        fixture.metadata(),
        rg_workspace::WorkspaceLoweringConfig::default(),
    )
    .expect("fixture workspace should build");
    let mut parse = crate::ParseDb::build(&workspace).expect("fixture parse db should build");
    {
        let sources = parse.source_inventory_handle();
        for package in parse.packages_mut() {
            package
                .discover_modules(&sources)
                .expect("fixture modules should be discovered");
        }
    }
    parse.seal_sources();
    parse.evict_syntax_trees();
    parse.offload_line_indexes_for_packages(&[0]);
    parse.evict_saved_source_text();

    let before = parse.packages()[0]
        .parsed_files()
        .map(|file| {
            let entry = parse
                .source_inventory()
                .entry(file.path())
                .expect("parsed file should belong to the source inventory");
            (
                file.file_id(),
                file.path().to_path_buf(),
                file.source_revision(),
                std::sync::Arc::downgrade(&entry),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(before.len(), 3, "both targets should share one module file");

    let reallocated = parse.reallocated();
    assert!(
        before
            .iter()
            .all(|(_, _, _, entry)| entry.upgrade().is_some()),
        "readers holding the original parse db should keep their sources alive",
    );
    drop(parse);

    for (file_id, path, revision, old_entry) in before {
        assert!(
            old_entry.upgrade().is_none(),
            "reallocated file tables should release the original source entries",
        );
        let references = reallocated.file_refs_for_path(&path);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].file, file_id);
        let file = reallocated.packages()[0]
            .parsed_file(file_id)
            .expect("reallocation should preserve file ids");
        assert_eq!(file.path(), path);
        assert_eq!(file.source_revision(), revision);
        assert!(
            file.parse_syntax()
                .expect("source should reparse")
                .errors()
                .is_empty()
        );
        assert_eq!(
            file.line_index()
                .expect("line index should reload")
                .position(0),
            crate::Position { line: 0, column: 0 },
        );
    }
    reallocated
        .validate_saved_sources()
        .expect("reallocated sources should remain valid");
}

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
