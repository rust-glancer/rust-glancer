use std::sync::{Arc, mpsc};

use rg_lsp_proto::{AnalysisConfig, SysrootDiscovery};
use test_fixture::fixture_crate;

use super::{ProjectConfiguration, ProjectCoordinator};
use crate::memory::MemoryControl;

// Read a local binding through the published snapshot so this also checks cache-backed bodies.
fn assert_saved_body_type(project: &ProjectCoordinator, path: &std::path::Path) {
    let source = std::fs::read_to_string(path).expect("fixture source should exist");
    let offset = source.rfind("value").expect("fixture has a local value") as u32;
    project
        .saved_project()
        .expect("saved project should be available")
        .with_snapshot(rg_std::CancellationToken::new(), |snapshot| {
            let context = snapshot
                .file_context_for_path(path)
                .expect("fixture context should load")
                .expect("fixture source should be indexed");
            let analysis = snapshot.analysis();
            let ty = analysis
                .type_at(context.crate_ref, context.file, offset)
                .expect("saved local type should resolve")
                .expect("saved local has a type");
            assert_eq!(
                ty.primitive(),
                Some(rg_ir_model::PrimitiveTy::UnsignedInt(
                    rg_ir_model::UnsignedIntTy::Usize
                ))
            );
            Ok(())
        })
        .expect("query operation should succeed");
}

#[test]
fn eager_startup_and_reindex_have_body_analysis() {
    let fixture = fixture_crate(
        r#"
            //- /Cargo.toml
            [package]
            name = "eager_indexing_fixture"
            version = "0.1.0"
            edition = "2024"

            //- /src/lib.rs
            pub fn indexed_body() -> usize {
                let value = 1usize;
                value
            }
            "#,
    );
    let (sender, receiver) = mpsc::channel();
    let memory_control: Arc<dyn MemoryControl> = Arc::new(());
    let mut project = ProjectCoordinator::new(sender, memory_control);
    project
        .initialize(
            fixture.path(""),
            ProjectConfiguration::from(AnalysisConfig {
                sysroot_discovery: SysrootDiscovery::Disabled,
                ..AnalysisConfig::default()
            }),
        )
        .expect("eager fixture project should initialize");

    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "initialization finishes synchronously",
    );
    assert_saved_body_type(&project, &fixture.path("src/lib.rs"));

    project
        .reindex_workspace()
        .expect("eager workspace reindex should succeed");
    assert_saved_body_type(&project, &fixture.path("src/lib.rs"));
    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "reindex finishes synchronously",
    );
}
