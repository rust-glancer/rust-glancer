use std::{
    collections::HashMap,
    fmt::{self, Write as _},
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

use expect_test::Expect;
use rg_analysis::{
    CompletionApplicability, CompletionClientCapabilities, CompletionItem, CompletionQuery,
    WorkspaceSymbol,
};
use rg_body_ir::BodyAutoderef;
use rg_def_map::PackageSlot;
use rg_ir_model::TargetRef;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::{FileId, ParseDb};
use rg_ty::IndexedTyExt;
use rg_workspace::WorkspaceMetadata;
use test_fixture::{CrateFixture, FixtureMarkers, fixture_crate_with_markers};

use crate::{
    AnalysisChangeSummary, DirtyFileChange, FileContext, PackageResidencyPolicy, Project,
    SavedFileChange,
};

pub(super) struct HostFixture {
    fixture: CrateFixture,
    markers: FixtureMarkers,
    host: Project,
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

impl HostFixture {
    pub(super) fn build(spec: &str) -> Self {
        Self::build_with_package_residency_policy(spec, PackageResidencyPolicy::default())
    }

    pub(super) fn build_with_package_residency_policy(
        spec: &str,
        package_residency_policy: PackageResidencyPolicy,
    ) -> Self {
        let (fixture, markers) = fixture_crate_with_markers(spec);
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        let host = Project::builder(workspace)
            .package_residency_policy(package_residency_policy)
            .build()
            .expect("analysis project should build")
            .into_project();

        Self {
            fixture,
            markers,
            host,
        }
    }

    pub(super) fn file_id_for_path(&self, relative_path: &str) -> FileId {
        file_id_for_path(
            self.host.snapshot().parse_db(),
            &self.fixture.path(relative_path),
        )
    }

    pub(super) fn remove_cache_namespace(&self) {
        match fs::remove_dir_all(self.host.state.cache_store.root()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!(
                "fixture cache namespace {} should be removable: {error}",
                self.host.state.cache_store.root().display(),
            ),
        }
    }

    pub(super) fn corrupt_package_cache_artifact(&self, package_name: &str) {
        let path = self.package_cache_artifact_path(package_name);

        fs::write(&path, b"not a package cache artifact").unwrap_or_else(|error| {
            panic!(
                "fixture package cache artifact {} should be writable: {error}",
                path.display(),
            )
        });
    }

    pub(super) fn remove_package_cache_artifact(&self, package_name: &str) {
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

    pub(super) fn package_cache_artifact_exists(&self, package_name: &str) -> bool {
        self.package_cache_artifact_path(package_name).exists()
    }

    pub(super) fn document_symbol_names(&self, relative_path: &str) -> Vec<String> {
        let snapshot = self.host.snapshot();
        let contexts = snapshot
            .file_contexts_for_path(self.fixture.path(relative_path))
            .expect("fixture path should resolve to file contexts");
        let targets = contexts
            .iter()
            .flat_map(|context| context.targets.iter().copied())
            .collect::<Vec<_>>();
        let analysis = snapshot
            .analysis_for_targets(&targets)
            .expect("fixture analysis should materialize");
        let mut names = Vec::new();

        for context in contexts {
            for target in context.targets {
                for symbol in analysis
                    .document_symbols(target, context.file)
                    .expect("fixture document symbols should resolve")
                {
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
            .host
            .snapshot()
            .full_analysis()
            .expect("fixture analysis should construct before lazy package load");

        match analysis.workspace_symbols(query) {
            Ok(_) => panic!("fixture workspace symbol query should fail"),
            Err(error) => format!("{error:#}"),
        }
    }

    pub(super) fn dirty_overlay(&self, relative_path: &str, text: &str) -> Project {
        self.host
            .dirty_overlay([DirtyFileChange::new(self.fixture.path(relative_path), text)])
            .expect("fixture dirty overlay should build")
            .expect("fixture dirty overlay should touch a known file")
    }

    fn package_cache_artifact_path(&self, package_name: &str) -> PathBuf {
        let package = package_slot_by_name(self.host.snapshot().parse_db(), package_name);
        let header = self
            .host
            .state
            .cache_plan
            .artifact_header(package, &self.host.state.package_source_fingerprints)
            .expect("fixture package should have a cache artifact header");
        self.host
            .state
            .cache_store
            .package_artifact_path(&header.package)
    }

    pub(super) fn check(&self, observations: &[HostObservation<'_>], expect: Expect) {
        let actual = self.render(observations);
        expect.assert_eq(&format!("{}\n", actual.trim_end()));
    }

    pub(super) fn render(&self, observations: &[HostObservation<'_>]) -> String {
        self.render_project(&self.host, observations)
    }

    pub(super) fn render_project(
        &self,
        project: &Project,
        observations: &[HostObservation<'_>],
    ) -> String {
        self.render_observations(project, None, observations)
    }

    pub(super) fn render_dirty_project(
        &self,
        project: &Project,
        dirty_text: &str,
        observations: &[HostObservation<'_>],
    ) -> String {
        self.render_observations(project, Some(dirty_text), observations)
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
        let saved_files = self.fixture.write_fixture_files(spec);
        let mut summary = AnalysisChangeSummary {
            changed_files: Vec::new(),
            affected_packages: Vec::new(),
            changed_targets: Vec::new(),
        };

        for file in saved_files.files() {
            let change = SavedFileChange::new(self.fixture.path(file.relative_path()));
            let change_summary = self
                .host
                .apply_change(change)
                .expect("fixture save change should apply");
            merge_change_summary(&mut summary, change_summary);
        }

        summary
    }

    fn render_save_result(
        &self,
        summary: &AnalysisChangeSummary,
        observations: &[HostObservation<'_>],
    ) -> String {
        let mut dump = self.render_change_summary(summary);
        let observations = self.render_observations(&self.host, None, observations);
        if !observations.is_empty() {
            writeln!(&mut dump).expect("string writes should not fail");
            dump.push_str(&observations);
        }
        dump
    }

    fn render_change_summary(&self, summary: &AnalysisChangeSummary) -> String {
        let mut dump = String::new();

        self.render_changed_files(&self.host, &summary.changed_files, &mut dump);
        writeln!(&mut dump).expect("string writes should not fail");
        self.render_affected_packages(&self.host, &summary.affected_packages, &mut dump);
        writeln!(&mut dump).expect("string writes should not fail");
        self.render_changed_targets(&self.host, &summary.changed_targets, &mut dump);

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

    fn render_changed_targets(&self, project: &Project, targets: &[TargetRef], dump: &mut String) {
        writeln!(dump, "changed targets").expect("string writes should not fail");

        let mut labels = targets
            .iter()
            .map(|target| self.render_target_ref(project, *target))
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
        dirty_text: Option<&str>,
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
                HostObservation::BodyIrStats { label } => {
                    self.render_body_ir_stats(project, label, &mut dump);
                }
                HostObservation::CompletionsAt {
                    label,
                    relative_path,
                    offset,
                } => {
                    self.render_completions_at(
                        project,
                        label,
                        relative_path,
                        *offset,
                        dirty_text,
                        &mut dump,
                    );
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
                self.render_target_ref(project, symbol.target),
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
                .targets
                .iter()
                .map(|target| self.render_target_ref(project, *target))
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

        let marker = self.markers.position(marker);
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
        writeln!(dump, "- def-map targets {}", stats.def_map.target_count)
            .expect("string writes should not fail");
        writeln!(
            dump,
            "- semantic targets {}",
            stats.semantic_ir.target_count
        )
        .expect("string writes should not fail");
        writeln!(dump, "- body targets {}", stats.body_ir.target_count)
            .expect("string writes should not fail");
    }

    fn render_body_ir_stats(&self, project: &Project, label: &str, dump: &mut String) {
        let stats = project.snapshot().stats();

        writeln!(dump, "body ir stats `{label}`").expect("string writes should not fail");
        writeln!(dump, "- targets {}", stats.body_ir.target_count)
            .expect("string writes should not fail");
        writeln!(dump, "- bodies {}", stats.body_ir.body_count)
            .expect("string writes should not fail");
    }

    fn render_completions_at(
        &self,
        project: &Project,
        label: &str,
        relative_path: &str,
        offset: usize,
        dirty_text: Option<&str>,
        dump: &mut String,
    ) {
        writeln!(dump, "completions at `{label}`").expect("string writes should not fail");

        let snapshot = project.snapshot();
        let contexts = snapshot
            .file_contexts_for_path(self.fixture.path(relative_path))
            .expect("fixture path should resolve to file contexts");
        let targets = contexts
            .iter()
            .flat_map(|context| context.targets.iter().copied())
            .collect::<Vec<_>>();
        let analysis = snapshot
            .analysis_for_targets(&targets)
            .expect("fixture completion analysis should materialize");
        let mut completions = Vec::new();
        let offset = offset
            .try_into()
            .expect("fixture completion offset should fit into u32");

        for context in contexts {
            for target in context.targets {
                let mut query = CompletionQuery::new(target, context.file, offset)
                    .with_client_capabilities(
                        CompletionClientCapabilities::default().with_snippet_support(true),
                    );
                if let Some(text) = dirty_text {
                    query = query.with_source_text(text);
                }
                for item in analysis
                    .completions_at(query)
                    .expect("fixture completions should resolve")
                {
                    if !completions.contains(&item) {
                        completions.push(item);
                    }
                }
            }
        }

        completions.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then(left.kind.cmp(&right.kind))
                .then(left.applicability.cmp(&right.applicability))
        });

        if completions.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for item in completions {
            writeln!(dump, "- {}", Self::render_completion_item(&item))
                .expect("string writes should not fail");
        }
    }

    fn render_completion_item(item: &CompletionItem) -> String {
        if matches!(item.applicability, CompletionApplicability::Known) {
            return format!("{} {}", item.kind, item.label);
        }

        format!("{} {} ({})", item.kind, item.label, item.applicability)
    }

    fn workspace_symbol_key(
        &self,
        project: &Project,
        symbol: &WorkspaceSymbol,
    ) -> (String, String, String, String) {
        (
            symbol.kind.to_string(),
            symbol.name.clone(),
            self.render_target_ref(project, symbol.target),
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
        let package = self.package(project, symbol.target.package);
        let path = package
            .file_path(symbol.file_id)
            .expect("workspace symbol file should be parsed");
        self.display_path(path)
    }

    fn render_target_ref(&self, project: &Project, target_ref: TargetRef) -> String {
        let package = self.package(project, target_ref.package);
        let target = package
            .target(target_ref.target)
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
    BodyIrStats {
        label: &'a str,
    },
    CompletionsAt {
        label: &'a str,
        relative_path: &'a str,
        offset: usize,
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

    pub(super) fn body_ir_stats(label: &'a str) -> Self {
        Self::BodyIrStats { label }
    }

    pub(super) fn completions_at(label: &'a str, relative_path: &'a str, offset: usize) -> Self {
        Self::CompletionsAt {
            label,
            relative_path,
            offset,
        }
    }
}

/// Removes `$name$` cursor markers and reports their byte offsets in the cleaned text.
pub(super) fn parse_dirty_text(text: &str) -> (String, HashMap<String, usize>) {
    let mut clean = String::new();
    let mut cursors = HashMap::new();
    let mut remaining = text;

    while let Some(start) = remaining.find('$') {
        clean.push_str(&remaining[..start]);
        let after_start = &remaining[start + "$".len()..];
        let end = after_start
            .find('$')
            .expect("dirty text cursor should end with `$`");
        let cursor = &after_start[..end];
        assert!(
            !cursor.is_empty(),
            "dirty text cursor should have a non-empty name"
        );
        assert!(
            cursors.insert(cursor.to_string(), clean.len()).is_none(),
            "dirty text cursor `{cursor}` should be unique"
        );
        remaining = &after_start[end + "$".len()..];
    }

    clean.push_str(remaining);
    (clean, cursors)
}

fn file_id_for_path(parse: &ParseDb, path: &Path) -> FileId {
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

fn nominal_type_names_at(
    host: &Project,
    package_name: &str,
    path: &Path,
    offset: u32,
) -> Vec<String> {
    let snapshot = host.snapshot();
    let package_slot = package_slot_by_name(snapshot.parse_db(), package_name);
    let file_id = file_id_for_path(snapshot.parse_db(), path);
    let target = snapshot
        .targets_for_file(package_slot, file_id)
        .expect("fixture target lookup should start")
        .into_iter()
        .next()
        .expect("fixture file should be owned by a target");
    let analysis = snapshot
        .analysis_for_targets(&[target])
        .expect("fixture analysis should materialize");
    let Some(ty) = analysis
        .type_at(target, file_id, offset)
        .expect("fixture type query should resolve")
    else {
        return Vec::new();
    };

    let semantic_ir = host.state.semantic_ir.read_txn(unexpected_package_loader());
    let def_map = host.state.def_map.read_txn(unexpected_package_loader());
    let mut names = Vec::new();
    for candidate in BodyAutoderef::peel_references(&ty) {
        for ty in candidate.ty().as_nominals() {
            let Some(target_ref) = ty.def.origin.as_target_ref() else {
                continue;
            };
            let Some(local_def) = semantic_ir
                .items(target_ref)
                .expect("fixture semantic IR should load while rendering nominal types")
                .expect("Item store must exist")
                .semantic_item_view(ty.def.into())
                .and_then(|view| view.local_def())
            else {
                continue;
            };
            let Some(target_ref) = local_def.origin.as_target_ref() else {
                continue;
            };
            let Some(local_def) = def_map
                .def_map(target_ref)
                .expect("fixture def-map should load while rendering nominal types")
                .and_then(|def_map| def_map.local_def(local_def.local_def))
            else {
                continue;
            };
            names.push(local_def.name.to_string());
        }
    }
    names
}

fn package_slot_by_name(parse: &ParseDb, package_name: &str) -> PackageSlot {
    parse
        .packages()
        .iter()
        .enumerate()
        .find_map(|(idx, package)| {
            (package.package_name() == package_name).then_some(PackageSlot(idx))
        })
        .unwrap_or_else(|| panic!("fixture package {package_name} should be parsed"))
}

fn push_document_symbol_names(symbol: &rg_analysis::DocumentSymbol, names: &mut Vec<String>) {
    names.push(symbol.name.clone());
    for child in &symbol.children {
        push_document_symbol_names(child, names);
    }
}

fn unexpected_package_loader<T: 'static>() -> PackageLoader<'static, T> {
    PackageLoader::new(UnexpectedPackageLoader(PhantomData))
}

struct UnexpectedPackageLoader<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for UnexpectedPackageLoader<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnexpectedPackageLoader").finish()
    }
}

impl<T> LoadPackage<T> for UnexpectedPackageLoader<T> {
    fn load(&self, package: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "resident project fixture should not load offloaded package {}",
            package.0,
        )
    }
}
