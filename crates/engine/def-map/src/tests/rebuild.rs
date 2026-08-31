use std::sync::Arc;

use rg_item_tree::ItemTreeDb;
use rg_package_store::PackageStoreError;
use rg_parse::ParseDb;
use rg_text::PackageNameInterners;
use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, fixture_crate};

use rg_ir_model::{CrateId, CrateRef};

use crate::{
    CrateData, DefMapBuildProgress, DefMapDb, DefMapLoader, LoadDefMap, PackageDefMaps,
    PackageDefMapsManifest, PackageSlot,
};

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
        !renamed_entry.bindings(crate::Namespace::Types).is_empty(),
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
#[macro_export]
macro_rules! make_dep_item {
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

#[test]
fn rebuild_collects_foreign_declarations_in_the_dirty_package() {
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
pub struct Dep;

//- /crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
dep = { path = "../dep" }

//- /crates/app/src/lib.rs
pub struct Before;
"#,
        "dep",
    );
    let rebuilt = fixture.rebuild_package_after_edit(
        r#"
//- /crates/app/src/lib.rs
unsafe extern "C" {
    pub fn rebuilt_foreign();
    pub type RebuiltOpaque;
}

pub use self::RebuiltOpaque as ImportedOpaque;
"#,
        "app",
    );

    let root = rebuilt.lib_root_module("app");
    for name in ["rebuilt_foreign", "RebuiltOpaque", "ImportedOpaque"] {
        assert!(
            root.scope.entry(name).is_some(),
            "rebuilt app root should retain foreign-derived binding `{name}`",
        );
    }
    assert!(
        root.unresolved_imports.is_empty(),
        "a reexport of a rebuilt foreign type should resolve",
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
        let output = crate::testonly::build_source_closed_def_map(
            &workspace, &parse, &item_tree, &mut names,
        );
        let (mut old, _) = output.into_parts();
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
        let old_read = self.old.read_txn(DefMapLoader::new(ExpectedPackageLoader {
            package: self.clean_package,
            payload: Arc::clone(&self.clean_payload),
        }));
        let packages = [package];
        let mut session = self
            .old
            .start_package_build(
                &old_read,
                &self.workspace,
                &parse,
                &item_tree,
                &packages,
                &packages,
                &mut names,
                crate::MacroExpansionPerformancePreference::default(),
            )
            .expect("fixture DefMap package rebuild session should start");
        let def_map = match session
            .advance(&old_read, &parse, &item_tree, &mut names)
            .expect("fixture DefMap package rebuild session should advance")
        {
            DefMapBuildProgress::Complete(output) => output.into_parts().0,
            DefMapBuildProgress::NeedsMacroSourceFiles(requests) => panic!(
                "source-closed rebuild fixture requested {} macro source file(s)",
                requests.len(),
            ),
        };

        RebuiltDefMaps { parse, def_map }
    }

    fn build_item_tree(
        workspace: &WorkspaceMetadata,
    ) -> (ParseDb, ItemTreeDb, PackageNameInterners) {
        let mut parse = ParseDb::build(workspace).expect("fixture parse db should build");
        let mut names = PackageNameInterners::new(parse.package_count());
        let packages = (0..parse.package_count()).collect::<Vec<_>>();
        let item_tree = ItemTreeDb::build_packages(&mut parse, &packages, &mut names)
            .expect("fixture item-tree db should build");

        (parse, item_tree, names)
    }
}

struct RebuiltDefMaps {
    parse: ParseDb,
    def_map: DefMapDb,
}

impl RebuiltDefMaps {
    fn lib_root_module(&self, package_name: &str) -> &crate::ModuleData {
        let package_slot = package_slot(&self.parse, package_name);
        let crate_ref = lib_crate_ref(&self.parse, package_slot);
        let package = self
            .def_map
            .resident_package(crate_ref.package)
            .expect("rebuilt package should exist");
        let def_map = package
            .def_map(crate_ref.crate_id)
            .expect("rebuilt crate def-map should exist");
        let root_module = package
            .crate_data(crate_ref.crate_id)
            .and_then(|crate_data| crate_data.root_module())
            .expect("rebuilt crate def-map should have a root module");

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

fn lib_crate_ref(parse: &ParseDb, package_slot: PackageSlot) -> CrateRef {
    let package = parse
        .package(package_slot.0)
        .expect("fixture package should exist");
    let target = package
        .targets()
        .iter()
        .find(|target| target.kind.is_lib())
        .expect("fixture package should have a library target");
    CrateRef {
        package: package_slot,
        crate_id: CrateId(target.id.0),
    }
}

#[derive(Debug)]
struct ExpectedPackageLoader {
    package: PackageSlot,
    payload: Arc<PackageDefMaps>,
}

impl LoadDefMap for ExpectedPackageLoader {
    fn load_manifest(
        &self,
        package: PackageSlot,
    ) -> Result<Arc<PackageDefMapsManifest>, PackageStoreError> {
        assert_eq!(
            package, self.package,
            "only the expected clean dependency package should be loaded"
        );
        Ok(Arc::new(self.payload.manifest()))
    }

    fn load_crate(
        &self,
        package: PackageSlot,
        crate_id: CrateId,
    ) -> Result<Arc<CrateData>, PackageStoreError> {
        assert_eq!(
            package, self.package,
            "only the expected clean dependency package should be loaded"
        );
        Ok(Arc::new(
            self.payload
                .crate_data(crate_id)
                .expect("requested clean dependency crate should exist")
                .clone(),
        ))
    }
}
