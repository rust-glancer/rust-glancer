use std::path::{Path, PathBuf};

use rg_ir_model::{FileId, PackageSlot};
use rg_workspace::{SysrootSources, WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, FixtureMarkers, FixtureSpec, fixture_crate_with_markers};

use crate::{AnalysisChangeSummary, Project, ProjectSnapshot, SavedFileChange};

/// Materialized project fixture sources plus marker metadata.
pub struct ProjectSourceFixture {
    fixture: CrateFixture,
    markers: FixtureMarkers,
    has_sysroot: bool,
}

impl ProjectSourceFixture {
    pub fn build(spec: &str) -> Self {
        let (fixture, markers) = fixture_crate_with_markers(spec);
        Self {
            fixture,
            markers,
            has_sysroot: false,
        }
    }

    pub fn build_with_fake_sysroot(spec: &str) -> Self {
        let (fixture, markers) = fixture_crate_with_markers(spec);
        Self {
            fixture: fixture.with_fake_sysroot(),
            markers,
            has_sysroot: true,
        }
    }

    pub fn workspace_metadata(&self) -> WorkspaceMetadata {
        // Declared graphs keep source and cache tests focused on project behavior. Manifest-based
        // fixtures still use Cargo for tests of discovery and workspace graph changes.
        let workspace = if self.fixture.headers().next().is_some() {
            WorkspaceMetadata::from_fixture(&self.fixture.path(""), self.fixture.headers())
        } else {
            WorkspaceMetadata::for_tests(
                self.fixture.metadata(),
                WorkspaceLoweringConfig::default(),
            )
            .expect("fixture workspace metadata should build")
        };
        if !self.has_sysroot {
            return workspace;
        }

        let sysroot = SysrootSources::from_library_root(self.fixture.path("sysroot/library"))
            .expect("fixture sysroot should be complete");
        workspace.with_sysroot_sources(Some(sysroot))
    }

    pub fn build_project(&self) -> Project {
        Project::builder(self.workspace_metadata())
            .build()
            .expect("analysis project should build")
    }

    pub fn markers(&self) -> &FixtureMarkers {
        &self.markers
    }

    pub fn path(&self, relative_path: &str) -> PathBuf {
        self.fixture.path(relative_path)
    }

    pub fn write_fixture_files(&self, spec: &str) -> FixtureSpec {
        self.fixture.write_fixture_files(spec)
    }
}

/// Built project fixture for tests that need a mutable `Project` over fixture sources.
pub struct ProjectFixture {
    source: ProjectSourceFixture,
    project: Project,
}

impl ProjectFixture {
    pub fn build(spec: &str) -> Self {
        let source = ProjectSourceFixture::build(spec);
        let project = source.build_project();
        Self { source, project }
    }

    pub fn build_with_fake_sysroot(spec: &str) -> Self {
        let source = ProjectSourceFixture::build_with_fake_sysroot(spec);
        let project = source.build_project();
        Self { source, project }
    }

    pub fn source(&self) -> &ProjectSourceFixture {
        &self.source
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn path(&self, relative_path: &str) -> PathBuf {
        self.source.path(relative_path)
    }

    pub fn markers(&self) -> &FixtureMarkers {
        self.source.markers()
    }

    pub fn package_slot_by_name(&self, package_name: &str) -> PackageSlot {
        Self::package_slot_by_name_in(self.project.workspace(), package_name)
    }

    pub fn apply_saved_fixture(&mut self, spec: &str) -> AnalysisChangeSummary {
        let saved_files = self.source.write_fixture_files(spec);
        let changes = saved_files
            .files()
            .iter()
            .map(|file| SavedFileChange::fs_path(self.path(file.relative_path())))
            .collect::<Vec<_>>();
        self.project
            .apply_changes(changes)
            .expect("fixture save changes should apply as one source generation")
    }

    pub fn file_id_for_path_in(snapshot: ProjectSnapshot<'_>, path: &Path) -> FileId {
        let path = path
            .canonicalize()
            .expect("fixture path should canonicalize");
        FileId::for_path(&snapshot.query_context().interner(), &path)
            .expect("fixture file identity")
    }

    pub fn package_slot_by_name_in(
        workspace: &WorkspaceMetadata,
        package_name: &str,
    ) -> PackageSlot {
        workspace
            .packages()
            .iter()
            .enumerate()
            .find_map(|(idx, package)| (package.name == package_name).then_some(PackageSlot(idx)))
            .unwrap_or_else(|| panic!("fixture package {package_name} should exist"))
    }
}

impl Project {
    /// Keep fixture observations inside the same operation used by application requests.
    pub(crate) fn inspect<R>(&self, observe: impl FnOnce(ProjectSnapshot<'_>) -> R) -> R {
        self.with_snapshot(rg_std::CancellationToken::new(), |snapshot| {
            Ok(observe(snapshot))
        })
        .expect("fixture query operation should succeed")
    }
}
