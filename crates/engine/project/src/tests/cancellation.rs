use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rg_body_ir::{
    BodyFileShard, BodyIrLoader, CrateBodies, LoadBodyIr, PackageBodies, PackageBodiesManifest,
    testonly::BodyIrFixture,
};
use rg_ir_model::{CrateId, CrateRef, PackageSlot};
use rg_ir_view::IndexedViewDb;
use rg_package_store::PackageStoreError;
use rg_parse::FileId;
use rg_std::CancellationToken;

use rg_analysis::{Analysis, ReferenceQuery, SavedSourceView};

const SOURCE: &str = r#"
//- /Cargo.toml
[package]
name = "cancelled_search"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
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
            .file_shard(file)
            .expect("resident fixture can supply a shard");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterializationPoint {
    BaselinePrepared,
    BeforePublication,
}

thread_local! {
    static CANCEL_MATERIALIZATION: std::cell::Cell<Option<(MaterializationPoint, usize)>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn materialization_checkpoint(
    point: MaterializationPoint,
    cancellation: &CancellationToken,
) {
    CANCEL_MATERIALIZATION.with(|target| {
        if let Some((selected, remaining)) = target.get()
            && selected == point
        {
            if remaining == 0 {
                target.set(None);
                cancellation.cancel();
            } else {
                target.set(Some((selected, remaining - 1)));
            }
        }
    });
}

#[test]
fn cancelled_materialization_preserves_coverage_and_allows_retry() {
    use crate::{
        AnalysisSurface, PackageResidencyPolicy, Project, SplitIndexingMode,
        testonly::ProjectSourceFixture,
    };
    use std::fmt::Write as _;

    let mut source = String::from(
        "//- /Cargo.toml\n[workspace]\nmembers = [\"first\", \"second\"]\nresolver = \"3\"\n",
    );
    for name in ["first", "second"] {
        writeln!(
            source,
            r#"
//- /{name}/Cargo.toml
[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

//- /{name}/src/lib.rs
pub fn value() -> usize {{ 1 }}

//- /{name}/tests/requested.rs
#[test]
fn requested() {{ let value = {name}::value(); }}

//- /{name}/tests/untouched.rs
#[test]
fn untouched() {{ let value = {name}::value(); }}
"#
        )
        .expect("fixture text can be written");
    }
    for policy in [
        PackageResidencyPolicy::AllResident,
        PackageResidencyPolicy::AllOffloadable,
    ] {
        for (point, completed) in [
            (MaterializationPoint::BaselinePrepared, 0),
            (MaterializationPoint::BeforePublication, 0),
            (MaterializationPoint::BeforePublication, 1),
        ] {
            // Each scenario needs cold secondary-target coverage. Reusing a fixture directory
            // would also reuse the completed artifact written by the preceding retry.
            let fixture = ProjectSourceFixture::build(&source);
            let mut project = Project::builder(fixture.workspace_metadata())
                .split_indexing_mode(SplitIndexingMode::EarlyStart)
                .package_residency_policy(policy)
                .build()
                .expect("materialization fixture builds");
            let mut requested = Vec::new();
            let mut untouched = Vec::new();
            let mut libraries = Vec::new();
            for name in ["first", "second"] {
                for (path, targets) in [
                    ("src/lib.rs", &mut libraries),
                    ("tests/requested.rs", &mut requested),
                    ("tests/untouched.rs", &mut untouched),
                ] {
                    let contexts = project
                        .snapshot()
                        .file_contexts_for_path(fixture.path(&format!("{name}/{path}")))
                        .expect("fixture context resolves");
                    let target = contexts
                        .into_iter()
                        .flat_map(|context| context.crates)
                        .find(|target| {
                            project.state.parse.packages()[target.package.0]
                                .targets()
                                .get(target.crate_id.0)
                                .is_some_and(|target| target.src_path.ends_with(path))
                        })
                        .expect("source is a Cargo target root");
                    targets.push(target);
                }
            }
            requested.sort_unstable_by_key(|target| (target.package.0, target.crate_id.0));
            project
                .split_indexing()
                .finish()
                .expect("primary targets finish");
            let generation = project.generation_id();
            let cancellation = CancellationToken::new();
            CANCEL_MATERIALIZATION.with(|target| target.set(Some((point, completed))));
            let result = project
                .split_indexing()
                .materialize(AnalysisSurface::Crates(&requested), &cancellation);
            assert!(
                CANCEL_MATERIALIZATION.with(|target| target.get().is_none()),
                "cancellation must occur inside materialization: {policy:?}, {point:?}, completed={completed}, targets={requested:?}, result={result:?}"
            );
            let error = result.expect_err("cancelled materialization has no query result");
            assert!(error.chain().any(|cause| cause.is::<rg_std::Cancelled>()));
            assert_eq!(project.generation_id(), generation);
            for (index, target) in requested.iter().enumerate() {
                assert_eq!(
                    project
                        .state
                        .body_ir
                        .crate_coverage(*target)
                        .expect("target exists")
                        .is_complete(),
                    index < completed
                );
            }
            for target in &libraries {
                assert!(
                    project
                        .state
                        .body_ir
                        .crate_coverage(*target)
                        .expect("library exists")
                        .is_complete()
                );
            }
            for target in &untouched {
                assert!(
                    !project
                        .state
                        .body_ir
                        .crate_coverage(*target)
                        .expect("sibling exists")
                        .is_complete()
                );
            }
            if policy == PackageResidencyPolicy::AllOffloadable {
                assert!(
                    requested
                        .iter()
                        .all(|target| project.state.body_ir.package_is_offloaded(target.package)),
                    "no manifest overlay may escape publication"
                );
            }
            // Cached siblings must remain decodable, including their bodies, after an abandoned
            // manifest baseline or a completed artifact rewrite for a different target.
            let txn = project
                .state
                .body_ir
                .read_txn(project.state.query_read_loaders().body_ir);
            for library in &libraries {
                assert!(
                    !txn.bodies(*library, None)
                        .expect("saved library bodies remain readable")
                        .is_empty()
                );
            }
            drop(txn);
            let analysis = project
                .snapshot()
                .full_analysis(CancellationToken::new())
                .expect("saved view remains valid");
            assert_eq!(
                analysis
                    .workspace_symbols("value")
                    .expect("saved symbols remain readable")
                    .len(),
                2
            );
            drop(analysis);
            project
                .split_indexing()
                .materialize(
                    AnalysisSurface::Crates(&requested),
                    &CancellationToken::new(),
                )
                .expect("fresh request completes remaining coverage");
            assert!(requested.iter().all(|target| {
                project
                    .state
                    .body_ir
                    .crate_coverage(*target)
                    .expect("target exists")
                    .is_complete()
            }));
            assert_eq!(project.generation_id(), generation);
        }
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
