use std::{fmt::Write as _, path::Path};

use anyhow::Context as _;
use expect_test::Expect;
use rg_analysis::WorkspaceSymbol;
use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_workspace::Package;
use test_fixture::fixture_path_for_snapshot;

use crate::{
    AnalysisChangeSummary, FileContext, Project, ProjectSnapshot, testonly::ProjectFixture,
};

impl ProjectSnapshot<'_> {
    // Fixture paths may use a different spelling from the source identity (for example, macOS
    // temporary paths can pass through a symlink). Editor callers select that identity upstream.
    pub(crate) fn file_context_for_path(
        &self,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<Option<FileContext>> {
        let path = path.as_ref();
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("canonicalize fixture source {}", path.display()))?;
        self.file_context_for_source_path(&canonical_path)
    }
}

pub(super) struct HostFixture {
    fixture: ProjectFixture,
}

impl HostFixture {
    pub(super) fn build(spec: &str) -> Self {
        Self {
            fixture: ProjectFixture::build(spec),
        }
    }

    pub(super) fn build_with_fake_sysroot(spec: &str) -> Self {
        Self {
            fixture: ProjectFixture::build_with_fake_sysroot(spec),
        }
    }

    pub(super) fn check(&self, observations: &[HostObservation<'_>], expect: Expect) {
        let actual = self.render(observations);
        expect.assert_eq(&format!("{}\n", actual.trim_end()));
    }

    pub(super) fn render(&self, observations: &[HostObservation<'_>]) -> String {
        self.render_project(self.fixture.project(), observations)
    }

    pub(super) fn render_project(
        &self,
        project: &Project,
        observations: &[HostObservation<'_>],
    ) -> String {
        self.render_observations(project, observations)
    }

    pub(super) fn check_save(
        &mut self,
        spec: &str,
        observations: &[HostObservation<'_>],
        expect: Expect,
    ) {
        let summary = self.save(spec);
        let actual = self.render_save_result(&summary, observations);
        expect.assert_eq(&format!("{}\n", actual.trim_end()));
    }

    fn save(&mut self, spec: &str) -> AnalysisChangeSummary {
        self.fixture.apply_saved_fixture(spec)
    }

    fn render_save_result(
        &self,
        summary: &AnalysisChangeSummary,
        observations: &[HostObservation<'_>],
    ) -> String {
        let mut dump = self.render_change_summary(summary);
        let observations = self.render_observations(self.fixture.project(), observations);
        if !observations.is_empty() {
            writeln!(&mut dump).expect("string writes should not fail");
            dump.push_str(&observations);
        }
        dump
    }

    fn render_change_summary(&self, summary: &AnalysisChangeSummary) -> String {
        let mut dump = String::new();

        self.render_changed_files(self.fixture.project(), &summary.changed_files, &mut dump);
        writeln!(&mut dump).expect("string writes should not fail");
        self.render_changed_crates(self.fixture.project(), &summary.changed_crates, &mut dump);

        dump
    }

    fn render_changed_files(&self, project: &Project, changed_files: &[FileId], dump: &mut String) {
        writeln!(dump, "changed files").expect("string writes should not fail");

        let mut files = changed_files
            .iter()
            .map(|changed_file| {
                let path = changed_file
                    .path(
                        &project
                            .queries
                            .lock()
                            .expect("project query runtime")
                            .interner(),
                    )
                    .expect("changed file has an interned path");
                self.display_path(&path)
            })
            .collect::<Vec<_>>();
        files.sort();

        if files.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for path in files {
            writeln!(dump, "- {path}").expect("string writes should not fail");
        }
    }

    fn render_changed_crates(&self, project: &Project, targets: &[CrateRef], dump: &mut String) {
        writeln!(dump, "changed targets").expect("string writes should not fail");

        let mut labels = targets
            .iter()
            .map(|target| self.render_crate_ref(project, *target))
            .collect::<Vec<_>>();
        labels.sort();

        if labels.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for label in labels {
            writeln!(dump, "- {label}").expect("string writes should not fail");
        }
    }

    fn render_observations(
        &self,
        project: &Project,
        observations: &[HostObservation<'_>],
    ) -> String {
        let mut dump = String::new();

        for (idx, observation) in observations.iter().enumerate() {
            if idx > 0 {
                writeln!(&mut dump).expect("string writes should not fail");
            }
            match observation {
                HostObservation::WorkspaceSymbols { query } => {
                    self.render_workspace_symbols(project, query, &mut dump);
                }
                HostObservation::FileContexts {
                    label,
                    relative_path,
                } => {
                    self.render_file_context(project, label, relative_path, &mut dump);
                }
                HostObservation::TypeNamesAt {
                    label,
                    package,
                    marker,
                } => {
                    self.render_type_names_at(project, label, package, marker, &mut dump);
                }
            }
        }

        dump
    }

    fn render_workspace_symbols(&self, project: &Project, query: &str, dump: &mut String) {
        writeln!(dump, "workspace symbols `{query}`").expect("string writes should not fail");

        let mut symbols = project.inspect(|snapshot| {
            snapshot
                .analysis()
                .workspace_symbols(query)
                .expect("fixture workspace symbols should resolve")
        });
        symbols.sort_by(|left, right| {
            self.workspace_symbol_key(project, left)
                .cmp(&self.workspace_symbol_key(project, right))
        });

        if symbols.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for symbol in symbols {
            let path = self.symbol_path(project, &symbol);
            writeln!(
                dump,
                "- {} {} @ {} {path}",
                symbol.kind,
                symbol.name,
                self.render_crate_ref(project, symbol.crate_ref),
            )
            .expect("string writes should not fail");
        }
    }

    fn render_file_context(
        &self,
        project: &Project,
        label: &str,
        relative_path: &str,
        dump: &mut String,
    ) {
        writeln!(dump, "file context `{label}`").expect("string writes should not fail");

        let Some(context) = project.inspect(|snapshot| {
            snapshot
                .file_context_for_path(self.fixture.path(relative_path))
                .expect("fixture path should resolve to a file context")
        }) else {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        };
        let package = self.package(project, context.crate_ref.package);
        let path = project.inspect(|snapshot| {
            snapshot
                .sources()
                .path(context.file)
                .expect("file path")
                .to_path_buf()
        });
        writeln!(
            dump,
            "- {} {} -> {}",
            package.name,
            self.display_path(&path),
            self.render_crate_ref(project, context.crate_ref)
        )
        .expect("string writes should not fail");
    }

    fn render_type_names_at(
        &self,
        project: &Project,
        label: &str,
        package_name: &str,
        marker: &str,
        dump: &mut String,
    ) {
        writeln!(dump, "type names at `{label}`").expect("string writes should not fail");

        let marker = self.fixture.markers().position(marker);
        let path = self.fixture.path(&marker.path);
        let mut names = Self::nominal_type_names_at(project, package_name, &path, marker.offset);
        names.sort();

        if names.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for name in names {
            writeln!(dump, "- {name}").expect("string writes should not fail");
        }
    }

    fn workspace_symbol_key(
        &self,
        project: &Project,
        symbol: &WorkspaceSymbol,
    ) -> (String, String, String, String) {
        (
            symbol.kind.to_string(),
            symbol.name.clone(),
            self.render_crate_ref(project, symbol.crate_ref),
            self.symbol_path(project, symbol),
        )
    }

    fn symbol_path(&self, project: &Project, symbol: &WorkspaceSymbol) -> String {
        project.inspect(|snapshot| {
            self.display_path(
                snapshot
                    .sources()
                    .path(symbol.file_id)
                    .expect("symbol path"),
            )
        })
    }

    fn render_crate_ref(&self, project: &Project, crate_ref: CrateRef) -> String {
        let package = self.package(project, crate_ref.package);
        let target = project
            .workspace()
            .target(crate_ref)
            .expect("fixture target");
        format!("{}[{}]", package.name, target.kind)
    }

    fn package<'a>(&self, project: &'a Project, package: PackageSlot) -> &'a Package {
        project
            .state
            .workspace()
            .packages()
            .get(package.0)
            .expect("fixture package should exist")
    }

    fn display_path(&self, path: &Path) -> String {
        let display_root = self.fixture.path("");
        let root = display_root
            .canonicalize()
            .expect("fixture root should canonicalize");

        let relative = path
            .strip_prefix(&root)
            .or_else(|_| path.strip_prefix(&display_root))
            .expect("project snapshot path should stay inside its fixture root");
        fixture_path_for_snapshot(relative)
    }

    fn nominal_type_names_at(
        host: &Project,
        package_name: &str,
        path: &Path,
        offset: u32,
    ) -> Vec<String> {
        host.inspect(|snapshot| {
            let package_slot =
                ProjectFixture::package_slot_by_name_in(snapshot.state.workspace(), package_name);
            let file_id = ProjectFixture::file_id_for_path_in(snapshot, path);
            let target = snapshot
                .crates_for_file(package_slot, file_id)
                .expect("fixture target lookup should start")
                .into_iter()
                .next()
                .expect("fixture file should be owned by a target");
            let analysis = snapshot.analysis();
            let Some(ty) = analysis
                .type_at(target, file_id, offset)
                .expect("fixture type query should resolve")
            else {
                return Vec::new();
            };

            let reader = rg_semantic_ir::ItemReader(snapshot.declarations());
            let items = rg_semantic_ir::ItemStoreQuery::new(&reader);
            ty.nominal_type_defs()
                .filter_map(|ty| items.type_def_name(ty).expect("fixture type name resolves"))
                .map(str::to_owned)
                .collect()
        })
    }
}

pub(super) enum HostObservation<'a> {
    WorkspaceSymbols {
        query: &'a str,
    },
    FileContexts {
        label: &'a str,
        relative_path: &'a str,
    },
    TypeNamesAt {
        label: &'a str,
        package: &'a str,
        marker: &'a str,
    },
}

impl<'a> HostObservation<'a> {
    pub(super) fn workspace_symbols(query: &'a str) -> Self {
        Self::WorkspaceSymbols { query }
    }

    pub(super) fn file_context(label: &'a str, relative_path: &'a str) -> Self {
        Self::FileContexts {
            label,
            relative_path,
        }
    }

    pub(super) fn type_names_at(label: &'a str, package: &'a str, marker: &'a str) -> Self {
        Self::TypeNamesAt {
            label,
            package,
            marker,
        }
    }
}
