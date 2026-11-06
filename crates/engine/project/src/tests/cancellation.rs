use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rg_analysis::{Analysis, ReferenceQuery, SavedSourceView};
use rg_body_ir::{
    BodyFileShard, BodyIrLoader, CrateBodies, LoadBodyIr, PackageBodies, PackageBodiesManifest,
    testonly::BodyIrFixture,
};
use rg_ir_model::{CrateId, CrateRef, FileId, PackageSlot};
use rg_ir_view::IndexedViewDb;
use rg_package_store::PackageStoreError;
use rg_std::CancellationToken;

const SOURCE: &str = r#"
//- /src/lib.rs crate:cancelled_search
mod first;
mod second;
pub struct User;
pub fn root() -> User { User }

//- /src/first.rs
pub fn first() -> crate::User { crate::User }

//- /src/second.rs
pub fn second() -> crate::User { crate::User }
"#;

/// Cancel only after a real scan has opened its second file. Later file decodes must be skipped.
#[derive(Debug)]
struct ScanLoader {
    bodies: PackageBodies,
    loaded: Arc<AtomicUsize>,
    cancellation: CancellationToken,
    cancel_after: Option<usize>,
}

impl LoadBodyIr for ScanLoader {
    fn load_manifest(
        &self,
        _: PackageSlot,
    ) -> Result<Arc<PackageBodiesManifest>, PackageStoreError> {
        Ok(Arc::new(self.bodies.manifest()))
    }

    fn load_file_shard(
        &self,
        _: PackageSlot,
        crate_id: CrateId,
        file: FileId,
    ) -> Result<Arc<BodyFileShard>, PackageStoreError> {
        let shard = self
            .bodies
            .crate_bodies(crate_id)
            .expect("fixture crate exists")
            .file_shard(file);
        let loaded = self.loaded.fetch_add(1, Ordering::Relaxed) + 1;
        if self.cancel_after == Some(loaded) {
            self.cancellation.cancel();
        }
        Ok(Arc::new(shard))
    }

    fn load_crate(
        &self,
        _: PackageSlot,
        _: CrateId,
    ) -> Result<Arc<CrateBodies>, PackageStoreError> {
        panic!("source scans must load file shards individually")
    }
}

#[test]
fn cancelled_reference_and_rename_scans_discard_results_and_skip_later_files() {
    let fixture = BodyIrFixture::build(SOURCE);
    let target = CrateRef {
        package: PackageSlot(0),
        crate_id: CrateId(0),
    };
    let package = fixture
        .body_ir_db()
        .resident_package(target.package)
        .expect("fixture is resident")
        .clone();
    let mut deferred = fixture.body_ir_db().clone();
    deferred
        .offload_package(target.package)
        .expect("fixture package exists");
    let parse_package = &fixture.parse_db().packages()[0];
    let file = parse_package
        .parsed_files()
        .find(|file| file.path().ends_with("src/lib.rs"))
        .expect("fixture root exists");
    let offset = file
        .source_text()
        .expect("fixture source is readable")
        .find("struct User")
        .expect("fixture declaration exists") as u32
        + 7;

    for rename in [false, true] {
        for cancel_after in [Some(2), None] {
            let cancellation = CancellationToken::new();
            let loaded = Arc::new(AtomicUsize::new(0));
            let db =
                IndexedViewDb::new(
                    fixture
                        .def_map_db()
                        .read_txn(rg_def_map::DefMapLoader::resident_only("scan fixture")),
                    fixture.semantic_ir_db().read_txn(
                        rg_semantic_ir::SemanticIrLoader::resident_only("scan fixture"),
                    ),
                    deferred.read_txn(BodyIrLoader::new(ScanLoader {
                        bodies: package.clone(),
                        loaded: Arc::clone(&loaded),
                        cancellation: cancellation.clone(),
                        cancel_after,
                    })),
                    cancellation,
                );
            let analysis = Analysis::new(db, SavedSourceView::new(fixture.parse_db()));
            let targets = [target];
            let query = ReferenceQuery::find_references(&targets, true);
            let result = if rename {
                analysis
                    .rename(target, file.file_id(), offset, "Person", query)
                    .map(|rename| rename.expect("User can be renamed").edits.len())
            } else {
                analysis
                    .references(target, file.file_id(), offset, query)
                    .map(|refs| refs.len())
            };
            if cancel_after.is_some() {
                let error = result.expect_err("cancelled scan must not return partial results");
                assert!(error.chain().any(|cause| cause.is::<rg_std::Cancelled>()));
                assert_eq!(
                    loaded.load(Ordering::Relaxed),
                    2,
                    "later scan files must stay unloaded"
                );
            } else {
                assert!(
                    result.expect("fresh request should complete") >= 7,
                    "retry includes every declaration and use"
                );
                assert_eq!(loaded.load(Ordering::Relaxed), 3);
            }
        }
    }
}
