use std::{
    fs,
    path::{Path, PathBuf},
};

use rg_def_map::PackageSlot;
use rg_parse::{FileId, ParseDb};
use rg_workspace::{WorkspaceLoweringConfig, WorkspaceMetadata};
use test_fixture::{CrateFixture, FixtureMarkers, FixtureSpec, fixture_crate_with_markers};

use crate::{
    AnalysisChangeSummary, DirtyFileChange, PackageResidencyPolicy, Project, SavedFileChange,
};

/// Materialized project fixture sources plus marker metadata.
pub struct ProjectSourceFixture {
    fixture: CrateFixture,
    markers: FixtureMarkers,
}

impl ProjectSourceFixture {
    pub fn build(spec: &str) -> Self {
        let (fixture, markers) = fixture_crate_with_markers(spec);
        Self { fixture, markers }
    }

    pub fn workspace_metadata(&self) -> WorkspaceMetadata {
        WorkspaceMetadata::for_tests(self.fixture.metadata(), WorkspaceLoweringConfig::default())
            .expect("fixture workspace metadata should build")
    }

    pub fn build_project(&self) -> Project {
        self.build_project_with_package_residency_policy(PackageResidencyPolicy::default())
    }

    pub fn build_project_with_package_residency_policy(
        &self,
        package_residency_policy: PackageResidencyPolicy,
    ) -> Project {
        Project::builder(self.workspace_metadata())
            .package_residency_policy(package_residency_policy)
            .build()
            .expect("analysis project should build")
            .into_project()
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
        Self::build_with_package_residency_policy(spec, PackageResidencyPolicy::default())
    }

    pub fn build_with_package_residency_policy(
        spec: &str,
        package_residency_policy: PackageResidencyPolicy,
    ) -> Self {
        let source = ProjectSourceFixture::build(spec);
        let project = source.build_project_with_package_residency_policy(package_residency_policy);
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

    pub fn file_id_for_path(&self, relative_path: &str) -> FileId {
        Self::file_id_for_path_in(self.project.state.parse_db(), &self.path(relative_path))
    }

    pub fn package_slot_by_name(&self, package_name: &str) -> PackageSlot {
        Self::package_slot_by_name_in(self.project.state.parse_db(), package_name)
    }

    pub fn dirty_overlay(&self, relative_path: &str, text: &str) -> Project {
        self.project
            .dirty_overlay([DirtyFileChange::new(self.path(relative_path), text)])
            .expect("fixture dirty overlay should build")
            .expect("fixture dirty overlay should touch a known file")
    }

    pub fn apply_saved_fixture(&mut self, spec: &str) -> AnalysisChangeSummary {
        let saved_files = self.source.write_fixture_files(spec);
        let mut summary = AnalysisChangeSummary {
            changed_files: Vec::new(),
            affected_packages: Vec::new(),
            changed_targets: Vec::new(),
        };

        for file in saved_files.files() {
            let change = SavedFileChange::new(self.path(file.relative_path()));
            let change_summary = self
                .project
                .apply_change(change)
                .expect("fixture save change should apply");
            Self::merge_change_summary(&mut summary, change_summary);
        }

        summary
    }

    pub fn remove_package_cache_artifacts(&self) {
        self.project
            .state
            .cache_store
            .clear_package_artifacts()
            .unwrap_or_else(|error| {
                panic!("fixture package cache artifacts should be removable: {error}")
            });
    }

    pub fn corrupt_package_cache_artifact(&self, package_name: &str) {
        let path = self.package_cache_artifact_path(package_name);

        fs::write(&path, b"not a package cache artifact").unwrap_or_else(|error| {
            panic!(
                "fixture package cache artifact {} should be writable: {error}",
                path.display(),
            )
        });
    }

    pub fn remove_package_cache_artifact(&self, package_name: &str) {
        let path = self.package_cache_artifact_path(package_name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!(
                "fixture package cache artifact {} should be removable: {error}",
                path.display(),
            ),
        }
    }

    pub fn package_cache_artifact_exists(&self, package_name: &str) -> bool {
        self.package_cache_artifact_path(package_name).exists()
    }

    pub(crate) fn package_cache_artifact_path(&self, package_name: &str) -> PathBuf {
        let package = self.package_slot_by_name(package_name);
        let header = self
            .project
            .state
            .cache_plan
            .artifact_header(package, &self.project.state.package_source_fingerprints)
            .expect("fixture package should have a cache artifact header");
        self.project
            .state
            .cache_store
            .package_artifact_path(&header.package)
    }

    pub fn file_id_for_path_in(parse: &ParseDb, path: &Path) -> FileId {
        let canonical_path = path
            .canonicalize()
            .expect("fixture source path should canonicalize");

        parse
            .packages()
            .iter()
            .flat_map(|package| package.parsed_files())
            .find(|file| file.path() == canonical_path.as_path())
            .unwrap_or_else(|| panic!("fixture file {} should be parsed", path.display()))
            .file_id()
    }

    pub fn package_slot_by_name_in(parse: &ParseDb, package_name: &str) -> PackageSlot {
        parse
            .packages()
            .iter()
            .enumerate()
            .find_map(|(idx, package)| {
                (package.package_name() == package_name).then_some(PackageSlot(idx))
            })
            .unwrap_or_else(|| panic!("fixture package {package_name} should be parsed"))
    }

    fn merge_change_summary(target: &mut AnalysisChangeSummary, source: AnalysisChangeSummary) {
        for changed_file in source.changed_files {
            if !target.changed_files.contains(&changed_file) {
                target.changed_files.push(changed_file);
            }
        }
        for package in source.affected_packages {
            if !target.affected_packages.contains(&package) {
                target.affected_packages.push(package);
            }
        }
        for target_ref in source.changed_targets {
            if !target.changed_targets.contains(&target_ref) {
                target.changed_targets.push(target_ref);
            }
        }
    }
}
