use std::sync::Arc;

use rg_item_tree::ItemTreeDb;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;
use test_fixture::fixture_crate;

use crate::{DefMapDb, Package, PackageSlot, TargetRef};

#[test]
fn rebuild_resolves_dirty_imports_through_clean_packages() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub mod api {
    pub struct Api;
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub use dep::api::Api as Before;
"#,
    );
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build");
    let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
    let mut old_names = PackageNameInterners::new(parse.package_count());
    let item_tree =
        ItemTreeDb::build(&mut parse, &mut old_names).expect("fixture item-tree db should build");
    let mut old = DefMapDb::builder(&workspace, &parse, &item_tree)
        .name_interners(&mut old_names)
        .build()
        .expect("fixture def-map db should build");
    let dep_slot = package_slot(&parse, "dep");
    let dep_package = Arc::new(
        old.resident_package(dep_slot)
            .expect("old dep package should be resident before offload")
            .clone(),
    );
    old.offload_package(dep_slot)
        .expect("old dep package should be offloadable");

    fixture.write_fixture_files(
        r#"
//- /crates/app/src/lib.rs
pub use dep::api::Api as Renamed;
"#,
    );

    let mut parse = ParseDb::build(&workspace).expect("updated fixture parse db should build");
    let mut interner = PackageNameInterners::new(parse.package_count());
    let item_tree = ItemTreeDb::build(&mut parse, &mut interner)
        .expect("updated fixture item-tree db should build");

    let mut app_slot = None;
    for (package_idx, package) in parse.packages().iter().enumerate() {
        if package.package_name() == "app" {
            app_slot = Some(PackageSlot(package_idx));
        }
    }
    let app_slot = app_slot.expect("fixture app package should exist");

    let old_read = old.read_txn(PackageLoader::new(ExpectedPackageLoader {
        package: dep_slot,
        payload: dep_package,
    }));
    let rebuilt = old
        .package_rebuilder(
            &old_read,
            &workspace,
            &parse,
            &item_tree,
            &[app_slot],
            &mut interner,
        )
        .build()
        .expect("fixture def-map package rebuild should succeed");

    let app_package = parse
        .package(app_slot.0)
        .expect("fixture app package should exist after rebuild");
    let app_lib = app_package
        .targets()
        .iter()
        .find(|target| target.kind.is_lib())
        .expect("fixture app package should have a library target");
    let app_target = TargetRef {
        package: app_slot,
        target: app_lib.id,
    };
    let app_def_map = rebuilt
        .resident_package(app_target.package)
        .and_then(|package| package.target(app_target.target))
        .expect("rebuilt app def-map should exist");
    let root_module = app_def_map
        .root_module()
        .expect("rebuilt app def-map should have a root module");
    let root = app_def_map
        .module(root_module)
        .expect("rebuilt app root module should exist");
    let renamed_entry = root
        .scope
        .entry("Renamed")
        .expect("rebuilt app root should contain the renamed import");

    assert!(
        !renamed_entry.types().is_empty(),
        "dirty app import should resolve through the clean frozen dependency package"
    );
    assert!(
        root.unresolved_imports.is_empty(),
        "dirty app import through the clean dependency should not be recorded as unresolved"
    );
}

#[test]
fn rebuild_expands_dirty_macro_calls_from_clean_packages() {
    let fixture = fixture_crate(
        r#"
//- /Cargo.toml
[workspace]
members = ["crates/dep", "crates/app"]
resolver = "3"

//- /crates/dep/Cargo.toml
[package]
name = "dep"
version = "0.1.0"
edition = "2024"

//- /crates/dep/src/lib.rs
pub macro make_dep_item {
    () => {
        pub struct GeneratedFromDep;
    };
}

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
use dep::make_dep_item;

pub struct Before;
"#,
    );
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build");
    let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
    let mut old_names = PackageNameInterners::new(parse.package_count());
    let item_tree =
        ItemTreeDb::build(&mut parse, &mut old_names).expect("fixture item-tree db should build");
    let mut old = DefMapDb::builder(&workspace, &parse, &item_tree)
        .name_interners(&mut old_names)
        .build()
        .expect("fixture def-map db should build");
    let dep_slot = package_slot(&parse, "dep");
    let dep_package = Arc::new(
        old.resident_package(dep_slot)
            .expect("old dep package should be resident before offload")
            .clone(),
    );
    old.offload_package(dep_slot)
        .expect("old dep package should be offloadable");

    fixture.write_fixture_files(
        r#"
//- /crates/app/src/lib.rs
use dep::make_dep_item;

make_dep_item!();
"#,
    );

    let mut parse = ParseDb::build(&workspace).expect("updated fixture parse db should build");
    let mut interner = PackageNameInterners::new(parse.package_count());
    let item_tree = ItemTreeDb::build(&mut parse, &mut interner)
        .expect("updated fixture item-tree db should build");
    let app_slot = package_slot(&parse, "app");

    let old_read = old.read_txn(PackageLoader::new(ExpectedPackageLoader {
        package: dep_slot,
        payload: dep_package,
    }));
    let rebuilt = old
        .package_rebuilder(
            &old_read,
            &workspace,
            &parse,
            &item_tree,
            &[app_slot],
            &mut interner,
        )
        .build()
        .expect("fixture def-map package rebuild should succeed");

    let app_target = lib_target(&parse, app_slot);
    let app_def_map = rebuilt
        .resident_package(app_target.package)
        .and_then(|package| package.target(app_target.target))
        .expect("rebuilt app def-map should exist");
    let root_module = app_def_map
        .root_module()
        .expect("rebuilt app def-map should have a root module");
    let root = app_def_map
        .module(root_module)
        .expect("rebuilt app root module should exist");

    assert!(
        root.scope.entry("GeneratedFromDep").is_some(),
        "dirty app macro call should expand using the clean dependency macro definition"
    );
}

fn package_slot(parse: &ParseDb, name: &str) -> PackageSlot {
    parse
        .packages()
        .iter()
        .enumerate()
        .find_map(|(package_idx, package)| {
            (package.package_name() == name).then_some(PackageSlot(package_idx))
        })
        .expect("fixture package should exist")
}

fn lib_target(parse: &ParseDb, package_slot: PackageSlot) -> TargetRef {
    let package = parse
        .package(package_slot.0)
        .expect("fixture package should exist");
    let target = package
        .targets()
        .iter()
        .find(|target| target.kind.is_lib())
        .expect("fixture package should have a library target");
    TargetRef {
        package: package_slot,
        target: target.id,
    }
}

#[derive(Debug)]
struct ExpectedPackageLoader {
    package: PackageSlot,
    payload: Arc<Package>,
}

impl LoadPackage<Package> for ExpectedPackageLoader {
    fn load(&self, package: PackageSlot) -> Result<Arc<Package>, PackageStoreError> {
        assert_eq!(
            package, self.package,
            "only the expected clean dependency package should be loaded"
        );
        Ok(Arc::clone(&self.payload))
    }
}
