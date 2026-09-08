//! Publication races are exercised through owned inputs and the same public boundary as the LSP.

use std::fs;

use rg_body_ir::{BodyIrLoader, CrateBodiesCoverage};
use rg_ir_model::{CrateRef, FileId};
use rg_std::CancellationToken;

use crate::{
    AnalysisSurface, BodyPublicationOutcome, PackageResidencyPolicy, Project, SavedFileChange,
    SplitIndexingMode, testonly::ProjectSourceFixture,
};

const SOURCE: &str = r#"
//- /Cargo.toml
[package]
name = "body_products"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
mod first;
mod second;
mod third;
mod empty;
pub fn root() -> usize { 0 }

//- /src/first.rs
pub fn first() -> usize { let value = 1usize; val$first$ue }

//- /src/second.rs
pub fn second() -> usize { let value = 2usize; val$second$ue }

//- /src/third.rs
pub fn third() -> usize { let value = 3usize; value }

//- /src/empty.rs
pub struct Empty;

//- /tests/first.rs
#[test]
fn first() { let value = body_products::root(); }

//- /tests/second.rs
#[test]
fn second() { let value = body_products::root(); }

//- /tests/multi.rs
#[path = "../src/third.rs"]
mod third;
#[test]
fn multi() { let value = third::third(); }
"#;

struct Fixture {
    source: ProjectSourceFixture,
    project: Project,
}

impl Fixture {
    fn new(policy: PackageResidencyPolicy) -> Self {
        let source = ProjectSourceFixture::build(SOURCE);
        let project = Project::builder(source.workspace_metadata())
            .split_indexing_mode(SplitIndexingMode::EarlyStart)
            .package_residency_policy(policy)
            .build()
            .expect("body product fixture builds");
        Self { source, project }
    }

    fn file(&self, path: &str) -> (CrateRef, FileId) {
        let context = self
            .project
            .snapshot()
            .file_contexts_for_path(self.source.path(path))
            .expect("fixture path resolves")
            .into_iter()
            .next()
            .expect("fixture file has a context");
        (context.crates[0], context.file)
    }

    fn assert_local_type(&self, marker: &str) {
        let position = self.source.markers().position(marker);
        let (target, file) = self.file(&position.path);
        assert!(
            self.project
                .snapshot()
                .full_analysis(CancellationToken::new())
                .expect("saved analysis opens")
                .type_at(target, file, position.offset)
                .expect("local type resolves")
                .is_some()
        );
    }
}

#[test]
fn cumulative_file_products_preserve_empty_file_readiness_and_local_facts() {
    let mut fixture = Fixture::new(PackageResidencyPolicy::AllResident);
    let background = fixture.project.deferred_body_build();
    let first = fixture.file("src/first.rs");
    let second = fixture.file("src/second.rs");
    let empty = fixture.file("src/empty.rs");
    let cancellation = CancellationToken::new();
    // These independent jobs have the same declaration generation but incomparable file sets.
    let first_work = fixture
        .project
        .split_indexing()
        .prepare(AnalysisSurface::Files(&[first, empty]));
    let second_work = fixture
        .project
        .split_indexing()
        .prepare(AnalysisSurface::Files(&[second]));
    let first_products = first_work.build(&cancellation).expect("first files build");
    let second_products = second_work
        .build(&cancellation)
        .expect("second file builds");
    assert!(
        fixture
            .project
            .split_indexing()
            .publish(first_products, &cancellation)
            .expect("first files publish")
            .improved()
    );
    assert!(
        !fixture
            .project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Files(&[empty]))
    );
    fixture.assert_local_type("first");
    let publication = fixture
        .project
        .split_indexing()
        .publish(second_products, &cancellation)
        .expect("incomparable result is classified");
    assert_eq!(
        publication.outcomes(),
        &[(first.0, BodyPublicationOutcome::ReplanRequired)]
    );
    assert!(
        fixture
            .project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Files(&[second]))
    );
    fixture.assert_local_type("first");

    // Replanning a second-file request includes the existing first and empty files automatically.
    fixture
        .project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[second]), &cancellation)
        .expect("cumulative retry completes");
    for file in [first, second, empty] {
        assert!(
            !fixture
                .project
                .split_indexing()
                .needs_materialization(AnalysisSurface::Files(&[file]))
        );
    }
    let coverage = fixture
        .project
        .state
        .body_ir
        .crate_coverage(first.0)
        .expect("crate coverage exists");
    assert!(matches!(coverage, CrateBodiesCoverage::Files(files) if files.len() == 3));
    fixture.assert_local_type("first");
    fixture.assert_local_type("second");
    // A complete background result replaces the crate revision as a unit. Adding the root body
    // changes dense body ids, while a reader of the previous partial revision keeps its own facts.
    let previous = fixture.project.state.body_ir.clone();
    let reader = previous.read_txn(BodyIrLoader::resident_only("prior partial bodies"));
    let old_body = reader
        .bodies(first.0, Some(first.1))
        .expect("old file bodies exist")[0]
        .0;
    let complete = background
        .build(&cancellation)
        .expect("complete background product builds");
    fixture
        .project
        .split_indexing()
        .publish(complete, &cancellation)
        .expect("background product improves partial coverage");
    assert!(
        fixture
            .project
            .state
            .body_ir
            .crate_coverage(first.0)
            .expect("crate exists")
            .is_complete()
    );
    assert_eq!(
        reader
            .body(old_body)
            .expect("old body reads")
            .expect("old body exists")
            .source()
            .file_id,
        first.1
    );
    fixture.assert_local_type("first");
    fixture.assert_local_type("second");
}

#[test]
fn sibling_products_preserve_live_targets_in_either_arrival_order() {
    for policy in [
        PackageResidencyPolicy::AllResident,
        PackageResidencyPolicy::AllOffloadable,
    ] {
        for reverse in [false, true] {
            let mut fixture = Fixture::new(policy);
            let cancellation = CancellationToken::new();
            let background = fixture.project.deferred_body_build();
            let first = fixture.file("tests/first.rs").0;
            let second = fixture.file("tests/second.rs").0;
            let first_work = fixture
                .project
                .split_indexing()
                .prepare(AnalysisSurface::Crates(&[first]));
            let second_work = fixture
                .project
                .split_indexing()
                .prepare(AnalysisSurface::Crates(&[second]));
            let mut products = vec![
                first_work
                    .build(&cancellation)
                    .expect("first target builds"),
                second_work
                    .build(&cancellation)
                    .expect("second target builds"),
            ];
            if reverse {
                products.reverse();
            }
            for product in products {
                assert!(
                    fixture
                        .project
                        .split_indexing()
                        .publish(product, &cancellation)
                        .expect("target publishes")
                        .improved()
                );
            }
            // A background primary-target result must use these newly completed siblings.
            let product = background
                .build(&cancellation)
                .expect("background target builds");
            fixture
                .project
                .split_indexing()
                .publish(product, &cancellation)
                .expect("background publishes");
            assert!(
                !fixture
                    .project
                    .split_indexing()
                    .needs_materialization(AnalysisSurface::Crates(&[first, second]))
            );
            fixture.assert_local_type("first");
            fixture.assert_local_type("second");
            assert_eq!(
                fixture
                    .project
                    .state
                    .body_ir
                    .package_is_offloaded(first.package),
                policy == PackageResidencyPolicy::AllOffloadable
            );
        }
    }
}

#[test]
fn saved_generation_rejects_old_products_before_touching_its_artifact() {
    let mut fixture = Fixture::new(PackageResidencyPolicy::AllOffloadable);
    let target = fixture.file("tests/first.rs").0;
    let cancellation = CancellationToken::new();
    let old_generation = fixture.project.generation_id();
    let old = fixture
        .project
        .split_indexing()
        .prepare(AnalysisSurface::Crates(&[target]))
        .build(&cancellation)
        .expect("old generation builds");
    let path = fixture.source.path("src/empty.rs");
    fs::write(&path, "pub struct Updated;\n").expect("save fixture change");
    fixture
        .project
        .apply_change(SavedFileChange::fs_path(path))
        .expect("new source generation publishes");
    fixture
        .project
        .split_indexing()
        .finish()
        .expect("new primary target finishes");
    assert_ne!(fixture.project.generation_id(), old_generation);
    let state = &fixture.project.state;
    let header = state
        .cache_plan
        .artifact_header(target.package, &state.package_source_fingerprints)
        .expect("saved header exists");
    let path = state.cache_store.package_artifact_path(&header.package);
    let before = fs::read(&path).expect("saved artifact exists");
    let publication = fixture
        .project
        .split_indexing()
        .publish(old, &cancellation)
        .expect("obsolete products are discarded");
    assert_eq!(
        publication.outcomes(),
        &[(target, BodyPublicationOutcome::ObsoleteGeneration)]
    );
    assert_eq!(
        fs::read(&path).expect("saved artifact remains readable"),
        before
    );
    assert!(
        fixture
            .project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[target]))
    );
    assert!(
        fixture
            .project
            .state
            .body_ir
            .package_is_offloaded(target.package)
    );
}

#[test]
fn cached_reader_keeps_one_revision_across_sibling_publication() {
    let mut fixture = Fixture::new(PackageResidencyPolicy::AllOffloadable);
    fixture
        .project
        .split_indexing()
        .finish()
        .expect("primary target finishes");
    let first = fixture.file("src/first.rs");
    let second = fixture.file("src/second.rs");
    let test = fixture.file("tests/first.rs").0;
    let old_bodies = fixture.project.state.body_ir.clone();
    let reader = old_bodies.read_txn(fixture.project.state.query_read_loaders().body_ir);
    let before = reader
        .bodies(first.0, Some(first.1))
        .expect("first old shard loads");
    assert_eq!(before.len(), 1);
    // Loading the first shard pins the manifest and file descriptor. The next shard must still
    // come from that revision even after a different target rewrites the package artifact.
    fixture
        .project
        .split_indexing()
        .materialize(AnalysisSurface::Crates(&[test]), &CancellationToken::new())
        .expect("cached sibling publishes");
    assert_eq!(
        reader
            .bodies(second.0, Some(second.1))
            .expect("second old shard loads")
            .len(),
        1
    );
    assert!(
        reader
            .bodies(test, None)
            .expect("old deferred target remains readable")
            .is_empty()
    );
    let current = fixture
        .project
        .state
        .body_ir
        .read_txn(fixture.project.state.query_read_loaders().body_ir);
    assert_eq!(
        current
            .bodies(test, None)
            .expect("new target is readable")
            .len(),
        1
    );
    for (body, _) in before {
        assert!(
            reader
                .body(body)
                .expect("old body identity remains valid")
                .is_some()
        );
    }
}

#[test]
fn partial_product_replans_when_its_package_became_offloaded() {
    let mut fixture = Fixture::new(PackageResidencyPolicy::AllOffloadable);
    let file = fixture.file("tests/multi.rs");
    let cancellation = CancellationToken::new();
    let partial = fixture
        .project
        .split_indexing()
        .prepare(AnalysisSurface::Files(&[file]))
        .build(&cancellation)
        .expect("partial secondary target builds");
    fixture
        .project
        .split_indexing()
        .finish()
        .expect("primary target finishes and offloads");
    let publication = fixture
        .project
        .split_indexing()
        .publish(partial, &cancellation)
        .expect("partial product is classified against live residency");
    assert_eq!(
        publication.outcomes(),
        &[(file.0, BodyPublicationOutcome::ReplanRequired)]
    );
    assert!(
        fixture
            .project
            .state
            .body_ir
            .package_is_offloaded(file.0.package)
    );
    fixture
        .project
        .split_indexing()
        .materialize(AnalysisSurface::Files(&[file]), &cancellation)
        .expect("fresh plan completes only the requested cached target");
    assert!(
        !fixture
            .project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[file.0]))
    );
    let sibling = fixture.file("tests/second.rs").0;
    assert!(
        fixture
            .project
            .split_indexing()
            .needs_materialization(AnalysisSurface::Crates(&[sibling]))
    );
    assert!(
        fixture
            .project
            .state
            .body_ir
            .package_is_offloaded(file.0.package)
    );
}

#[test]
fn publication_preserves_typed_cache_failures_and_live_coverage() {
    for missing in [false, true] {
        let mut fixture = Fixture::new(PackageResidencyPolicy::AllOffloadable);
        fixture
            .project
            .split_indexing()
            .finish()
            .expect("primary target finishes");
        let target = fixture.file("tests/first.rs").0;
        let cancellation = CancellationToken::new();
        let products = fixture
            .project
            .split_indexing()
            .prepare(AnalysisSurface::Crates(&[target]))
            .build(&cancellation)
            .expect("secondary target builds");
        let generation = fixture.project.generation_id();
        let state = &fixture.project.state;
        let header = state
            .cache_plan
            .artifact_header(target.package, &state.package_source_fingerprints)
            .expect("artifact identity exists");
        let path = state.cache_store.package_artifact_path(&header.package);
        if missing {
            fs::remove_file(path).expect("remove disposable artifact");
        } else {
            fs::write(path, b"broken artifact").expect("replace disposable artifact");
        }
        let error = fixture
            .project
            .split_indexing()
            .publish(products, &cancellation)
            .expect_err("publication needs a valid sibling artifact");
        assert!(
            Project::is_recoverable_cache_load_failure(&error),
            "publication must retain its typed cache-read cause: {error:#}"
        );
        assert_eq!(fixture.project.generation_id(), generation);
        assert!(
            fixture
                .project
                .state
                .body_ir
                .package_is_offloaded(target.package)
        );
        assert!(
            fixture
                .project
                .split_indexing()
                .needs_materialization(AnalysisSurface::Crates(&[target]))
        );
    }
}
