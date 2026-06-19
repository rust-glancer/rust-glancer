use std::sync::Arc;

use rg_item_tree::ItemTreeDb;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use rg_ir_model::TargetRef;

use crate::{DefMapDb, PackageSlot};
use rg_ir_storage::PackageDefMaps;

#[test]
fn rebuild_resolves_dirty_imports_through_clean_packages() {
    let fixture = RebuildFixture::build(
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
        "dep",
    );
    let rebuilt = fixture.rebuild_package_after_edit(
        r#"
//- /crates/app/src/lib.rs
pub use dep::api::Api as Renamed;
"#,
        "app",
    );

    let root = rebuilt.lib_root_module("app");
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
    let fixture = RebuildFixture::build(
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
        "dep",
    );
    let rebuilt = fixture.rebuild_package_after_edit(
        r#"
//- /crates/app/src/lib.rs
use dep::make_dep_item;

make_dep_item!();
"#,
        "app",
    );

    let root = rebuilt.lib_root_module("app");

    assert!(
        root.scope.entry("GeneratedFromDep").is_some(),
        "dirty app macro call should expand using the clean dependency macro definition"
    );
}

/// Rebuilds one edited package against an old snapshot with one clean package offloaded.
struct RebuildFixture {
    fixture: CrateFixture,
    workspace: WorkspaceMetadata,
    old: DefMapDb,
    clean_package: PackageSlot,
    clean_payload: Arc<PackageDefMaps>,
}

impl RebuildFixture {
    fn build(fixture: &str, clean_package: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace =
            WorkspaceMetadata::for_tests(fixture.metadata(), WorkspaceLoweringConfig::default())
                .expect("fixture workspace metadata should build");
        let (parse, item_tree, mut names) = Self::build_item_tree(&workspace);
        let mut old = DefMapDb::builder(&workspace, &parse, &item_tree)
            .name_interners(&mut names)
            .build()
            .expect("fixture def-map db should build");
        let clean_package = package_slot(&parse, clean_package);
        let clean_payload = Arc::new(
            old.resident_package(clean_package)
                .expect("old clean package should be resident before offload")
                .clone(),
        );
        old.offload_package(clean_package)
            .expect("old clean package should be offloadable");

        Self {
            fixture,
            workspace,
            old,
            clean_package,
            clean_payload,
        }
    }

    fn rebuild_package_after_edit(&self, edit: &str, package_name: &str) -> RebuiltDefMaps {
        self.fixture.write_fixture_files(edit);

        let (parse, item_tree, mut names) = Self::build_item_tree(&self.workspace);
        let package = package_slot(&parse, package_name);
        let old_read = self.old.read_txn(PackageLoader::new(ExpectedPackageLoader {
            package: self.clean_package,
            payload: Arc::clone(&self.clean_payload),
        }));
        let def_map = self
            .old
            .package_rebuilder(
                &old_read,
                &self.workspace,
                &parse,
                &item_tree,
                &[package],
                &mut names,
            )
            .build()
            .expect("fixture def-map package rebuild should succeed");

        RebuiltDefMaps { parse, def_map }
    }

    fn build_item_tree(
        workspace: &WorkspaceMetadata,
    ) -> (ParseDb, ItemTreeDb, PackageNameInterners) {
        let mut parse = ParseDb::build(workspace).expect("fixture parse db should build");
        let mut names = PackageNameInterners::new(parse.package_count());
        let item_tree =
            ItemTreeDb::build(&mut parse, &mut names).expect("fixture item-tree db should build");

        (parse, item_tree, names)
    }
}

struct RebuiltDefMaps {
    parse: ParseDb,
    def_map: DefMapDb,
}

impl RebuiltDefMaps {
    fn lib_root_module(&self, package_name: &str) -> &rg_ir_storage::ModuleData {
        let package_slot = package_slot(&self.parse, package_name);
        let target = lib_target(&self.parse, package_slot);
        let package = self
            .def_map
            .resident_package(target.package)
            .expect("rebuilt package should exist");
        let def_map = package
            .def_map(target.target)
            .expect("rebuilt target def-map should exist");
        let root_module = package
            .target_data(target.target)
            .and_then(|target_data| target_data.root_module())
            .expect("rebuilt target def-map should have a root module");

        def_map
            .module(root_module)
            .expect("rebuilt root module should exist")
    }
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
    payload: Arc<PackageDefMaps>,
}

impl LoadPackage<PackageDefMaps> for ExpectedPackageLoader {
    fn load(&self, package: PackageSlot) -> Result<Arc<PackageDefMaps>, PackageStoreError> {
        assert_eq!(
            package, self.package,
            "only the expected clean dependency package should be loaded"
        );
        Ok(Arc::clone(&self.payload))
    }
}
