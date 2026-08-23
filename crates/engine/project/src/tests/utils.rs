use std::{fmt::Write as _, path::Path};

use expect_test::Expect;
use rg_analysis::WorkspaceSymbol;
use rg_def_map::PackageSlot;
use rg_ir_model::CrateRef;
use rg_package_store::PackageLoader;
use rg_parse::FileId;

use crate::{
    AnalysisChangeSummary, FileContext, PackageResidencyPolicy, Project, testonly::ProjectFixture,
};

pub(super) struct HostFixture {
    fixture: ProjectFixture,
}

impl HostFixture {
    pub(super) fn build(spec: &str) -> Self {
        Self::build_with_package_residency_policy(spec, PackageResidencyPolicy::default())
    }

    pub(super) fn build_with_sysroot(spec: &str) -> Self {
        Self {
            fixture: ProjectFixture::build_with_sysroot(spec),
        }
    }

    pub(super) fn build_with_package_residency_policy(
        spec: &str,
        package_residency_policy: PackageResidencyPolicy,
    ) -> Self {
        Self {
            fixture: ProjectFixture::build_with_package_residency_policy(
                spec,
                package_residency_policy,
            ),
        }
    }

    pub(super) fn file_id_for_path(&self, relative_path: &str) -> FileId {
        self.fixture.file_id_for_path(relative_path)
    }

    pub(super) fn remove_package_cache_artifacts(&self) {
        self.fixture.remove_package_cache_artifacts();
    }

    pub(super) fn corrupt_package_cache_artifact(&self, package_name: &str) {
        self.fixture.corrupt_package_cache_artifact(package_name);
    }

    pub(super) fn remove_package_cache_artifact(&self, package_name: &str) {
        self.fixture.remove_package_cache_artifact(package_name);
    }

    pub(super) fn package_cache_artifact_exists(&self, package_name: &str) -> bool {
        self.fixture.package_cache_artifact_exists(package_name)
    }

    pub(super) fn document_symbol_names(&self, relative_path: &str) -> Vec<String> {
        let snapshot = self.fixture.project().snapshot();
        let contexts = snapshot
            .file_contexts_for_path(self.fixture.path(relative_path))
            .expect("fixture path should resolve to file contexts");
        let targets = contexts
            .iter()
            .flat_map(|context| context.crates.iter().copied())
            .collect::<Vec<_>>();
        let analysis = snapshot
            .analysis_for_crates(&targets)
            .expect("fixture analysis should materialize");
        let mut names = Vec::new();

        for context in contexts {
            for target in context.crates {
                let outline = analysis
                    .document_symbols(target, context.file)
                    .expect("fixture document symbols should resolve");
                for symbol in outline.symbols {
                    push_document_symbol_names(&symbol, &mut names);
                }
            }
        }

        names.sort();
        names.dedup();
        names
    }

    pub(super) fn workspace_symbols_error(&self, query: &str) -> String {
        let analysis = self
            .fixture
            .project()
            .snapshot()
            .full_analysis()
            .expect("fixture analysis should construct before lazy package load");

        match analysis.workspace_symbols(query) {
            Ok(_) => panic!("fixture workspace symbol query should fail"),
            Err(error) => format!("{error:#}"),
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
        self.render_affected_packages(
            self.fixture.project(),
            &summary.affected_packages,
            &mut dump,
        );
        writeln!(&mut dump).expect("string writes should not fail");
        self.render_changed_crates(self.fixture.project(), &summary.changed_crates, &mut dump);

        dump
    }

    fn render_changed_files(
        &self,
        project: &Project,
        changed_files: &[crate::ChangedFile],
        dump: &mut String,
    ) {
        writeln!(dump, "changed files").expect("string writes should not fail");

        let mut files = changed_files
            .iter()
            .map(|changed_file| {
                let package = self.package(project, changed_file.package);
                let path = package
                    .file_path(changed_file.file)
                    .expect("changed file should have a parsed path");
                (package.package_name().to_string(), self.display_path(path))
            })
            .collect::<Vec<_>>();
        files.sort();

        if files.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for (package_name, path) in files {
            writeln!(dump, "- {package_name} {path}").expect("string writes should not fail");
        }
    }

    fn render_affected_packages(
        &self,
        project: &Project,
        packages: &[PackageSlot],
        dump: &mut String,
    ) {
        writeln!(dump, "affected packages").expect("string writes should not fail");

        let mut names = packages
            .iter()
            .map(|slot| self.package(project, *slot).package_name().to_string())
            .collect::<Vec<_>>();
        names.sort();

        if names.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for name in names {
            writeln!(dump, "- {name}").expect("string writes should not fail");
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
                    self.render_file_contexts(project, label, relative_path, &mut dump);
                }
                HostObservation::TypeNamesAt {
                    label,
                    package,
                    marker,
                } => {
                    self.render_type_names_at(project, label, package, marker, &mut dump);
                }
                HostObservation::ResidentStats { label } => {
                    self.render_resident_stats(project, label, &mut dump);
                }
            }
        }

        dump
    }

    fn render_workspace_symbols(&self, project: &Project, query: &str, dump: &mut String) {
        writeln!(dump, "workspace symbols `{query}`").expect("string writes should not fail");

        let snapshot = project.snapshot();
        let mut symbols = snapshot
            .full_analysis()
            .expect("fixture analysis should materialize")
            .workspace_symbols(query)
            .expect("fixture workspace symbols should resolve");
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

    fn render_file_contexts(
        &self,
        project: &Project,
        label: &str,
        relative_path: &str,
        dump: &mut String,
    ) {
        writeln!(dump, "file contexts `{label}`").expect("string writes should not fail");

        let mut contexts = project
            .snapshot()
            .file_contexts_for_path(self.fixture.path(relative_path))
            .expect("fixture path should resolve to file contexts");
        contexts.sort_by(|left, right| {
            self.file_context_key(project, left)
                .cmp(&self.file_context_key(project, right))
        });

        if contexts.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for context in contexts {
            let package = self.package(project, context.package);
            let path = package
                .file_path(context.file)
                .expect("file context should have a parsed path");
            let mut targets = context
                .crates
                .iter()
                .map(|target| self.render_crate_ref(project, *target))
                .collect::<Vec<_>>();
            targets.sort();

            writeln!(
                dump,
                "- {} {} -> {}",
                package.package_name(),
                self.display_path(path),
                targets.join(", ")
            )
            .expect("string writes should not fail");
        }
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
        let mut names = nominal_type_names_at(project, package_name, &path, marker.offset);
        names.sort();

        if names.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for name in names {
            writeln!(dump, "- {name}").expect("string writes should not fail");
        }
    }

    fn render_resident_stats(&self, project: &Project, label: &str, dump: &mut String) {
        let stats = project.snapshot().stats();

        writeln!(dump, "resident stats `{label}`").expect("string writes should not fail");
        writeln!(dump, "- def-map crates {}", stats.def_map.crate_count)
            .expect("string writes should not fail");
        writeln!(dump, "- semantic crates {}", stats.semantic_ir.crate_count)
            .expect("string writes should not fail");
        writeln!(dump, "- body crates {}", stats.body_ir.crate_count)
            .expect("string writes should not fail");
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

    fn file_context_key(&self, project: &Project, context: &FileContext) -> (String, String) {
        let package = self.package(project, context.package);
        let path = package
            .file_path(context.file)
            .expect("file context should have a parsed path");
        (package.package_name().to_string(), self.display_path(path))
    }

    fn symbol_path(&self, project: &Project, symbol: &WorkspaceSymbol) -> String {
        let package = self.package(project, symbol.crate_ref.package);
        let path = package
            .file_path(symbol.file_id)
            .expect("workspace symbol file should be parsed");
        self.display_path(path)
    }

    fn render_crate_ref(&self, project: &Project, crate_ref: CrateRef) -> String {
        let package = self.package(project, crate_ref.package);
        // Crate ids are allocated in parsed Cargo-target order. Use that stable source shape here
        // because the semantic payload may have been offloaded by the time the fixture is rendered.
        let target = package
            .targets()
            .get(crate_ref.crate_id.0)
            .expect("target should exist while rendering host fixture");
        format!("{}[{}]", package.package_name(), target.kind)
    }

    fn package<'a>(&self, project: &'a Project, package: PackageSlot) -> &'a rg_parse::Package {
        project
            .snapshot()
            .parse_db()
            .package(package.0)
            .expect("fixture package should exist")
    }

    fn display_path(&self, path: &Path) -> String {
        let display_root = self.fixture.path("");
        let root = display_root
            .canonicalize()
            .expect("fixture root should canonicalize");

        path.strip_prefix(&root)
            .or_else(|_| path.strip_prefix(&display_root))
            .unwrap_or(path)
            .display()
            .to_string()
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
    ResidentStats {
        label: &'a str,
    },
}

impl<'a> HostObservation<'a> {
    pub(super) fn workspace_symbols(query: &'a str) -> Self {
        Self::WorkspaceSymbols { query }
    }

    pub(super) fn file_contexts(label: &'a str, relative_path: &'a str) -> Self {
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

    pub(super) fn resident_stats(label: &'a str) -> Self {
        Self::ResidentStats { label }
    }
}

fn nominal_type_names_at(
    host: &Project,
    package_name: &str,
    path: &Path,
    offset: u32,
) -> Vec<String> {
    let snapshot = host.snapshot();
    let package_slot = ProjectFixture::package_slot_by_name_in(snapshot.parse_db(), package_name);
    let file_id = ProjectFixture::file_id_for_path_in(snapshot.parse_db(), path);
    let target = snapshot
        .crates_for_file(package_slot, file_id)
        .expect("fixture target lookup should start")
        .into_iter()
        .next()
        .expect("fixture file should be owned by a target");
    let analysis = snapshot
        .analysis_for_crates(&[target])
        .expect("fixture analysis should materialize");
    let Some(ty) = analysis
        .type_at(target, file_id, offset)
        .expect("fixture type query should resolve")
    else {
        return Vec::new();
    };

    let semantic_ir = host
        .state
        .semantic_ir
        .read_txn(PackageLoader::resident_only("resident project fixture"));
    let def_map = host
        .state
        .def_map
        .read_txn(PackageLoader::resident_only("resident project fixture"));
    let mut names = Vec::new();
    for ty in ty.nominal_type_defs() {
        let Some(crate_ref) = ty.origin.as_crate_ref() else {
            continue;
        };
        let Some(local_def) = semantic_ir
            .items(crate_ref)
            .expect("fixture semantic IR should load while rendering nominal types")
            .expect("Item store must exist")
            .semantic_item_view(ty.into())
            .and_then(|view| view.local_def())
        else {
            continue;
        };
        let Some(crate_ref) = local_def.origin.as_crate_ref() else {
            continue;
        };
        let Some(local_def) = def_map
            .def_map(crate_ref)
            .expect("fixture def-map should load while rendering nominal types")
            .and_then(|def_map| def_map.local_def(local_def.local_def))
        else {
            continue;
        };
        names.push(local_def.name.to_string());
    }
    names
}

fn push_document_symbol_names(symbol: &rg_analysis::DocumentSymbol, names: &mut Vec<String>) {
    names.push(symbol.name.clone());
    for child in &symbol.children {
        push_document_symbol_names(child, names);
    }
}
