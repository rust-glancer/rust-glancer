mod inline;

use std::{fmt::Write as _, path::Path};

use expect_test::Expect;
use rg_body_ir::{BodyReader, ExprData, ExprKind};
use rg_ir_model::{BodySource, CrateRef, FileId, PackageSlot, Span};
use rg_ir_view::{IndexedViewDb, testonly::render_indexed_type};
use rg_parse::SourceReader;
use rg_semantic_ir::{
    complete_source_ownership,
    testonly::{SemanticIrFixture, SemanticIrView},
};
use rg_workspace::{TargetKind, testonly::WorkspaceFixture};
use test_fixture::{
    CrateFixture, FixtureMarkers, fixture_crate, fixture_crate_with_markers,
    fixture_path_for_snapshot,
};

use crate::{
    Analysis, CodeAction, CodeActionKind, CodeActionQuery, CodeActionTrigger,
    CompletionApplicability, CompletionClientCapabilities, CompletionInsertText, CompletionItem,
    CompletionKind, CompletionQuery, HoverInfo, InlayHint, NavigationTarget, NavigationTargetKind,
    ReferenceLocation, ReferenceQuery as AnalysisReferenceQuery, ReferenceSearchFile, RenameEdit,
    RenameResult, RenameTarget, SymbolAt, WorkspaceSymbol,
};

/// Queries written beside their markers are removed before the fixture reaches analysis.
pub(super) fn check_analysis(fixture: &str, expect: Expect) {
    let fixture = inline::InlineAnalysisFixture::parse(fixture);
    let db = AnalysisFixtureDb::build_from_crate(CrateFixture::from_files(fixture.files));
    AnalysisQuerySnapshot::check(db, &fixture.markers, &fixture.queries, expect);
}

pub(super) fn check_analysis_with_fake_sysroot(fixture: &str, expect: Expect) {
    let fixture = inline::InlineAnalysisFixture::parse(fixture);
    let db = AnalysisFixtureDb::build_with_fake_sysroot(CrateFixture::from_files(fixture.files));
    AnalysisQuerySnapshot::check(db, &fixture.markers, &fixture.queries, expect);
}

pub(super) fn check_analysis_queries(fixture: &str, queries: &[AnalysisQuery], expect: Expect) {
    let (fixture, markers) = fixture_crate_with_markers(fixture);
    let db = AnalysisFixtureDb::build_from_crate(fixture);
    AnalysisQuerySnapshot::check(db, &markers, queries, expect);
}

pub(super) fn check_analysis_queries_with_fake_sysroot(
    fixture: &str,
    queries: &[AnalysisQuery],
    expect: Expect,
) {
    let (fixture, markers) = fixture_crate_with_markers(fixture);
    let db = AnalysisFixtureDb::build_with_fake_sysroot(fixture);
    AnalysisQuerySnapshot::check(db, &markers, queries, expect);
}

pub(super) fn check_workspace_symbols(fixture: &str, query: &str, expect: Expect) {
    AnalysisFixtureDb::build_from_crate(fixture_crate(fixture)).inspect(|db| {
        let renderer = AnalysisSymbolSnapshot { db };
        let title = format!("workspace symbols `{query}`");
        let actual = test_fixture::testonly::render_responses([(
            title.as_str(),
            renderer.render_workspace_symbols(query),
        )]);
        expect.assert_eq(&actual);
    });
}

pub(super) fn check_inlay_hints(fixture: &str, query: InlayHintsQuery, expect: Expect) {
    let db = AnalysisFixtureDb::build_from_crate(fixture_crate(fixture));
    assert_inlay_hints(db, &query, expect);
}

pub(super) fn check_inlay_hints_with_fake_sysroot(
    fixture: &str,
    query: InlayHintsQuery,
    expect: Expect,
) {
    let db = AnalysisFixtureDb::build_with_fake_sysroot(fixture_crate(fixture));
    assert_inlay_hints(db, &query, expect);
}

fn assert_inlay_hints(mut fixture: AnalysisFixtureDb, query: &InlayHintsQuery, expect: Expect) {
    fixture.inspect(|db| {
        let renderer = AnalysisSymbolSnapshot { db };
        let actual = test_fixture::testonly::render_responses([(
            query.title,
            renderer.render_inlay_hints(query),
        )]);
        expect.assert_eq(&actual);
    });
}

pub(super) struct AnalysisQuery {
    title: String,
    marker: String,
    target: AnalysisTarget,
    kind: AnalysisQueryKind,
    completion_label_prefix: Option<String>,
}

impl AnalysisQuery {
    pub(super) fn symbol(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::SymbolAt)
    }

    pub(super) fn resolve(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::ResolveSymbol)
    }

    pub(super) fn goto(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoDefinition)
    }

    pub(super) fn goto_type(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoTypeDefinition)
    }

    pub(super) fn goto_impl(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoImplementation)
    }

    pub(super) fn ty(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::TypeAt)
    }

    pub(super) fn complete(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAt)
    }

    pub(super) fn code_actions(title: &str, marker: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::CodeActions(CodeActionTrigger::Invoked),
        )
    }

    pub(super) fn automatic_code_actions(title: &str, marker: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::CodeActions(CodeActionTrigger::Automatic),
        )
    }

    pub(super) fn complete_verbose(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAtVerbose)
    }

    /// Run completion as if the saved fixture text were the document captured for this request.
    pub(super) fn complete_with_source(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAtWithSource)
    }

    pub(super) fn complete_verbose_with_source(title: &str, marker: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::CompletionsAtVerboseWithSource,
        )
    }

    /// Render only keyword rows from a full request-local completion query.
    pub(super) fn complete_keywords_with_source(title: &str, marker: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::CompletionKeywordsAtWithSource,
        )
    }

    pub(super) fn complete_keywords_verbose_with_source(title: &str, marker: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::CompletionKeywordsAtVerboseWithSource,
        )
    }

    pub(super) fn hover(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::Hover)
    }

    pub(super) fn references(title: &str, marker: &str, query: ReferenceQuery) -> Self {
        Self::new(title, marker, AnalysisQueryKind::References(query))
    }

    pub(super) fn prepare_rename(title: &str, marker: &str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::PrepareRename)
    }

    pub(super) fn rename(title: &str, marker: &str, new_name: &str) -> Self {
        Self::new(
            title,
            marker,
            AnalysisQueryKind::Rename(new_name.to_string()),
        )
    }

    pub(super) fn in_bin(mut self, package_name: &str) -> Self {
        self.target = AnalysisTarget::bin(package_name);
        self
    }

    pub(super) fn in_lib(mut self, package_name: &str) -> Self {
        self.target = AnalysisTarget::lib_package(package_name);
        self
    }

    /// Restrict a completion snapshot to the rows that an editor would keep for this prefix.
    pub(super) fn matching(mut self, label_prefix: &str) -> Self {
        self.completion_label_prefix = Some(label_prefix.to_string());
        self
    }

    fn new(title: &str, marker: &str, kind: AnalysisQueryKind) -> Self {
        Self {
            title: title.to_string(),
            marker: marker.to_string(),
            target: AnalysisTarget::lib(),
            kind,
            completion_label_prefix: None,
        }
    }
}

pub(super) struct InlayHintsQuery {
    title: &'static str,
    path: &'static str,
    target: AnalysisTarget,
}

impl InlayHintsQuery {
    pub(super) fn new(title: &'static str, path: &'static str) -> Self {
        Self {
            title,
            path,
            target: AnalysisTarget::lib(),
        }
    }

    pub(super) fn in_bin(mut self, package_name: &'static str) -> Self {
        self.target = AnalysisTarget::bin(package_name);
        self
    }

    pub(super) fn in_lib(mut self, package_name: &'static str) -> Self {
        self.target = AnalysisTarget::lib_package(package_name);
        self
    }
}

#[derive(Debug, Clone)]
struct AnalysisTarget {
    package_name: Option<String>,
    kind: TargetKind,
}

impl AnalysisTarget {
    fn lib() -> Self {
        Self {
            package_name: None,
            kind: TargetKind::Lib,
        }
    }

    fn lib_package(package_name: &str) -> Self {
        Self {
            package_name: Some(package_name.to_string()),
            kind: TargetKind::Lib,
        }
    }

    fn bin(package_name: &str) -> Self {
        Self {
            package_name: Some(package_name.to_string()),
            kind: TargetKind::Bin,
        }
    }

    fn matches_package(&self, package_name: &str) -> bool {
        self.package_name
            .as_deref()
            .is_none_or(|expected| expected == package_name)
    }
}

#[derive(Debug, Clone)]
enum AnalysisQueryKind {
    SymbolAt,
    ResolveSymbol,
    GotoDefinition,
    GotoTypeDefinition,
    GotoImplementation,
    References(ReferenceQuery),
    PrepareRename,
    Rename(String),
    TypeAt,
    CodeActions(CodeActionTrigger),
    CompletionsAt,
    CompletionsAtVerbose,
    CompletionsAtWithSource,
    CompletionsAtVerboseWithSource,
    CompletionKeywordsAtWithSource,
    CompletionKeywordsAtVerboseWithSource,
    Hover,
}

#[derive(Debug, Clone)]
pub(super) struct ReferenceQuery {
    include_declaration: bool,
    scope: ReferenceQueryScope,
}

impl ReferenceQuery {
    pub(super) fn all() -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::AllIncludedTargets,
        }
    }

    pub(super) fn current_target() -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::CurrentTarget,
        }
    }

    pub(super) fn current_file() -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::CurrentFile,
        }
    }

    pub(super) fn files(paths: &[&str]) -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::Files(
                paths.iter().map(|path| (*path).to_string()).collect(),
            ),
        }
    }

    pub(super) fn libs(packages: &[&str]) -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::LibTargets(
                packages
                    .iter()
                    .map(|package| (*package).to_string())
                    .collect(),
            ),
        }
    }

    pub(super) fn without_declaration(mut self) -> Self {
        self.include_declaration = false;
        self
    }
}

#[derive(Debug, Clone)]
enum ReferenceQueryScope {
    AllIncludedTargets,
    CurrentTarget,
    CurrentFile,
    Files(Vec<String>),
    LibTargets(Vec<String>),
}

struct AnalysisFixtureDb(SemanticIrFixture);

impl AnalysisFixtureDb {
    fn build_from_crate(fixture: CrateFixture) -> Self {
        Self(SemanticIrFixture::from_workspace(
            WorkspaceFixture::from_crate(fixture),
        ))
    }

    fn build_with_fake_sysroot(fixture: CrateFixture) -> Self {
        Self(SemanticIrFixture::from_workspace(
            WorkspaceFixture::from_crate(fixture).with_fake_sysroot(),
        ))
    }

    fn inspect<R>(&mut self, observe: impl FnOnce(AnalysisFixture<'_>) -> R) -> R {
        self.0
            .inspect(|semantic| observe(AnalysisFixture { semantic }))
    }
}

struct AnalysisFixture<'db> {
    semantic: SemanticIrView<'db>,
}

impl<'db> AnalysisFixture<'db> {
    fn analysis(&self) -> Analysis<'db> {
        Analysis::new(self.view_db(), self.semantic.workspace())
    }

    fn view_db(&self) -> IndexedViewDb<'db> {
        IndexedViewDb::new(
            self.semantic.db(),
            self.semantic.definitions(),
            self.semantic.items(),
            self.bodies(),
            self.semantic.db().services().cancellation.clone(),
        )
    }

    fn bodies(&self) -> BodyReader<'db> {
        BodyReader::new(self.semantic.db(), self.semantic.declarations())
    }

    /// Renders source locations relative to their Cargo package instead of exposing temporary
    /// fixture roots or host-native path separators in snapshots.
    fn render_file_path(&self, package: PackageSlot, file_id: FileId) -> String {
        let path = SourceReader::new(self.semantic.db())
            .path(file_id)
            .expect("snapshot source path should load");
        let package_root = self.semantic.workspace().packages()[package.0].root_dir();

        // Cargo-generated sources may sit outside their package root. Their build directory is
        // temporary and does not help a query snapshot, so preserve the old filename-only shape.
        let relative = path.strip_prefix(package_root).unwrap_or_else(|_| {
            Path::new(
                path.file_name()
                    .expect("snapshot source path should have a file name"),
            )
        });
        fixture_path_for_snapshot(relative)
    }

    fn target_and_file_for_path(
        &self,
        selected: &AnalysisTarget,
        path: &str,
    ) -> (CrateRef, FileId) {
        let mut matches = Vec::new();
        let normalized_path = path.trim_start_matches('/');
        let sources = SourceReader::new(self.semantic.db());
        for (owner, target) in self.semantic.workspace().targets() {
            let package = &self.semantic.workspace().packages()[owner.package.0];
            if !selected.matches_package(&package.name) || target.kind != selected.kind {
                continue;
            }
            let ownership = complete_source_ownership(self.semantic.db(), owner)
                .expect("fixture source ownership should resolve");
            for file in ownership.files() {
                if sources
                    .path(file)
                    .expect("fixture file path")
                    .ends_with(normalized_path)
                {
                    matches.push((owner, file));
                }
            }
        }

        assert_eq!(
            matches.len(),
            1,
            "path `{path}` should identify exactly one file owned by one {} target",
            selected.kind
        );
        matches.pop().expect("one match should be present")
    }

    fn target_for(&self, selected: &AnalysisTarget) -> CrateRef {
        let mut matches = Vec::new();

        for (krate, target) in self.semantic.workspace().targets() {
            let package = &self.semantic.workspace().packages()[krate.package.0];
            if selected.matches_package(&package.name) && target.kind == selected.kind {
                matches.push(krate);
            }
        }

        assert_eq!(
            matches.len(),
            1,
            "target selection should identify exactly one {} target",
            selected.kind
        );
        matches.pop().expect("one match should be present")
    }

    fn all_targets(&self) -> &[CrateRef] {
        self.semantic.targets()
    }
}

struct AnalysisQuerySnapshot<'a, 'db> {
    db: AnalysisFixture<'db>,
    markers: &'a FixtureMarkers,
    queries: &'a [AnalysisQuery],
}

impl AnalysisQuerySnapshot<'_, '_> {
    fn check(
        mut fixture: AnalysisFixtureDb,
        markers: &FixtureMarkers,
        queries: &[AnalysisQuery],
        expect: Expect,
    ) {
        fixture.inspect(|db| {
            let renderer = AnalysisQuerySnapshot {
                db,
                markers,
                queries,
            };
            expect.assert_eq(&renderer.render());
        });
    }

    fn render(&self) -> String {
        test_fixture::testonly::render_responses(
            self.queries
                .iter()
                .map(|query| (query.title.as_str(), self.render_query(query))),
        )
    }

    fn render_query(&self, query: &AnalysisQuery) -> String {
        let (target, file_id, offset) = self.query_location(query);
        let mut dump = String::new();
        match &query.kind {
            AnalysisQueryKind::SymbolAt => {
                self.render_symbol(
                    self.db
                        .analysis()
                        .symbol_at(target, file_id, offset)
                        .expect("fixture symbol query should resolve"),
                    file_id,
                    &mut dump,
                );
            }
            AnalysisQueryKind::ResolveSymbol => {
                let Some(symbol) = self
                    .db
                    .analysis()
                    .symbol_at(target, file_id, offset)
                    .expect("fixture symbol query should resolve")
                else {
                    self.render_targets(Vec::new(), &mut dump);
                    return dump;
                };
                self.render_targets(
                    self.db
                        .analysis()
                        .resolve_symbol(symbol)
                        .expect("fixture symbol resolution should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::GotoDefinition => {
                self.render_targets(
                    self.db
                        .analysis()
                        .goto_definition(target, file_id, offset)
                        .expect("fixture goto query should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::GotoTypeDefinition => {
                self.render_targets(
                    self.db
                        .analysis()
                        .goto_type_definition(target, file_id, offset)
                        .expect("fixture goto type query should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::GotoImplementation => {
                self.render_targets(
                    self.db
                        .analysis()
                        .goto_implementation(target, file_id, offset)
                        .expect("fixture goto implementation query should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::References(reference_options) => {
                let references = match &reference_options.scope {
                    ReferenceQueryScope::AllIncludedTargets => {
                        let use_site_targets = self.db.all_targets();
                        let reference_query = AnalysisReferenceQuery::find_references(
                            use_site_targets,
                            reference_options.include_declaration,
                        );
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture references query should resolve")
                    }
                    ReferenceQueryScope::CurrentTarget => {
                        let use_site_targets = [target];
                        let reference_query = AnalysisReferenceQuery::find_references(
                            &use_site_targets,
                            reference_options.include_declaration,
                        );
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture scoped references query should resolve")
                    }
                    ReferenceQueryScope::CurrentFile => {
                        let reference_query = AnalysisReferenceQuery::file_scoped(target, file_id);
                        let reference_query = if reference_options.include_declaration {
                            reference_query
                        } else {
                            reference_query.without_declarations()
                        };
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture file-scoped references query should resolve")
                    }
                    ReferenceQueryScope::Files(paths) => {
                        let search_files = paths
                            .iter()
                            .map(|path| {
                                let (target, file_id) =
                                    self.db.target_and_file_for_path(&query.target, path);
                                ReferenceSearchFile {
                                    crate_ref: target,
                                    file_id,
                                }
                            })
                            .collect::<Vec<_>>();
                        let reference_query = AnalysisReferenceQuery::find_references_in_files(
                            &search_files,
                            reference_options.include_declaration,
                        );
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture file-list references query should resolve")
                    }
                    ReferenceQueryScope::LibTargets(packages) => {
                        let use_site_targets = packages
                            .iter()
                            .map(|package| {
                                self.db.target_for(&AnalysisTarget::lib_package(package))
                            })
                            .collect::<Vec<_>>();
                        let reference_query = AnalysisReferenceQuery::find_references(
                            &use_site_targets,
                            reference_options.include_declaration,
                        );
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture scoped references query should resolve")
                    }
                };
                self.render_references(references, &mut dump);
            }
            AnalysisQueryKind::PrepareRename => {
                let rename_target = self
                    .db
                    .analysis()
                    .prepare_rename(target, file_id, offset)
                    .expect("fixture prepare rename query should resolve");
                self.render_rename_target(rename_target, target.package, &mut dump);
            }
            AnalysisQueryKind::Rename(new_name) => {
                let use_site_targets = self.db.all_targets();
                let reference_query =
                    AnalysisReferenceQuery::find_references(use_site_targets, true);
                let rename_result = self
                    .db
                    .analysis()
                    .rename(target, file_id, offset, new_name, reference_query)
                    .expect("fixture rename query should resolve");
                self.render_rename_result(rename_result, target.package, &mut dump);
            }
            AnalysisQueryKind::TypeAt => {
                let ty = self
                    .db
                    .analysis()
                    .type_at(target, file_id, offset)
                    .expect("fixture type query should resolve");
                writeln!(
                    dump,
                    "{}",
                    ty.as_ref()
                        .map(|ty| render_indexed_type(&self.db.view_db(), ty))
                        .unwrap_or_else(|| "<none>".to_string())
                )
                .expect("string writes should not fail");
            }
            AnalysisQueryKind::CodeActions(trigger) => {
                let source_text = SourceReader::new(self.db.semantic.db())
                    .text(file_id)
                    .expect("code action source text should load");
                let actions = self
                    .db
                    .analysis()
                    .code_actions(
                        CodeActionQuery::new(
                            target,
                            file_id,
                            Span {
                                start: offset,
                                end: offset,
                            },
                            &source_text,
                        )
                        .with_trigger(*trigger),
                    )
                    .expect("fixture code action query should resolve");
                self.render_code_actions(actions, &source_text, &mut dump);
            }
            AnalysisQueryKind::CompletionsAt => {
                let completion_query = CompletionQuery::new(target, file_id, offset)
                    .with_client_capabilities(CompletionClientCapabilities {
                        snippet_support: true,
                    });
                let mut completions = self
                    .db
                    .analysis()
                    .completions_at(completion_query)
                    .expect("fixture completion query should resolve");
                Self::filter_completion_labels(query, &mut completions);
                self.render_completions(completions, &mut dump);
            }
            AnalysisQueryKind::CompletionsAtVerbose => {
                let completion_query = CompletionQuery::new(target, file_id, offset)
                    .with_client_capabilities(CompletionClientCapabilities {
                        snippet_support: true,
                    });
                let mut completions = self
                    .db
                    .analysis()
                    .completions_at(completion_query)
                    .expect("fixture completion query should resolve");
                Self::filter_completion_labels(query, &mut completions);
                self.render_completions_verbose(completions, &mut dump);
            }
            AnalysisQueryKind::CompletionsAtWithSource
            | AnalysisQueryKind::CompletionsAtVerboseWithSource
            | AnalysisQueryKind::CompletionKeywordsAtWithSource
            | AnalysisQueryKind::CompletionKeywordsAtVerboseWithSource => {
                let source_text = SourceReader::new(self.db.semantic.db())
                    .text(file_id)
                    .expect("completion source text should load");
                let completion_query = CompletionQuery::new(target, file_id, offset)
                    .with_source_text(&source_text)
                    .with_client_capabilities(CompletionClientCapabilities {
                        snippet_support: true,
                    });
                let mut completions = self
                    .db
                    .analysis()
                    .completions_at(completion_query)
                    .expect("fixture completion query should resolve");
                Self::filter_completion_labels(query, &mut completions);
                if matches!(
                    query.kind,
                    AnalysisQueryKind::CompletionKeywordsAtWithSource
                        | AnalysisQueryKind::CompletionKeywordsAtVerboseWithSource
                ) {
                    completions.retain(|completion| completion.kind == CompletionKind::Keyword);
                }
                if matches!(
                    query.kind,
                    AnalysisQueryKind::CompletionsAtVerboseWithSource
                        | AnalysisQueryKind::CompletionKeywordsAtVerboseWithSource
                ) {
                    self.render_completions_verbose(completions, &mut dump);
                } else {
                    self.render_completions(completions, &mut dump);
                }
            }
            AnalysisQueryKind::Hover => {
                self.render_hover(
                    self.db
                        .analysis()
                        .hover(target, file_id, offset)
                        .expect("fixture hover query should resolve"),
                    file_id,
                    &mut dump,
                );
            }
        }

        dump
    }

    fn filter_completion_labels(query: &AnalysisQuery, completions: &mut Vec<CompletionItem>) {
        if let Some(prefix) = &query.completion_label_prefix {
            completions.retain(|completion| completion.label.starts_with(prefix));
        }
    }

    fn query_location(&self, query: &AnalysisQuery) -> (CrateRef, FileId, u32) {
        let marker = self.markers.position(&query.marker);
        let (target, file_id) = self
            .db
            .target_and_file_for_path(&query.target, &marker.path);

        (target, file_id, marker.offset)
    }

    fn render_symbol(&self, symbol: Option<SymbolAt>, file_id: FileId, dump: &mut String) {
        let Some(symbol) = symbol else {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        };

        match symbol {
            SymbolAt::FunctionBody { body } => {
                let body = body.body_ir();
                let source = self
                    .db
                    .bodies()
                    .body(body)
                    .expect("saved body query should succeed")
                    .expect("body should exist while rendering analysis symbol")
                    .source();
                writeln!(
                    dump,
                    "body @ {}",
                    self.render_source_span(source.file_id, source.span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Declaration { declaration, span } => {
                let targets = self
                    .db
                    .analysis()
                    .resolve_symbol(SymbolAt::Declaration { declaration, span })
                    .expect("fixture symbol resolution should resolve");
                let label = targets
                    .first()
                    .map(|target| format!("{} {}", target.kind, target.name))
                    .unwrap_or_else(|| "declaration <unresolved>".to_string());
                writeln!(dump, "{label} @ {}", self.render_source_span(file_id, span))
                    .expect("string writes should not fail");
            }
            SymbolAt::Expr { expr } => {
                let body = expr.body_ir();
                let bodies = self.db.bodies();
                let body_view = bodies
                    .body(body)
                    .expect("saved body query should succeed")
                    .expect("body should exist while rendering analysis symbol");
                let expr_data = body_view
                    .expr(expr.expr_id())
                    .expect("expr id should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "{}",
                    self.render_expr_symbol(expr_data, body_view.source_at(expr_data.source))
                )
                .expect("string writes should not fail");
            }
            SymbolAt::TypePath {
                ref type_path,
                span,
            } => {
                writeln!(
                    dump,
                    "type path {} @ {}",
                    type_path.path(),
                    self.render_source_span(file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::ValuePath { ref path, span, .. } => {
                writeln!(
                    dump,
                    "value path {path} @ {}",
                    self.render_source_span(file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::RecordField {
                ref owner,
                ref key,
                span,
                ..
            } => {
                writeln!(
                    dump,
                    "record field {owner}::{} @ {}",
                    key.declaration_label(),
                    self.render_source_span(file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::UsePath { ref path, span, .. } => {
                writeln!(
                    dump,
                    "path {path} @ {}",
                    self.render_source_span(file_id, span)
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_expr_symbol(&self, expr: &ExprData, source: BodySource) -> String {
        let label = match &expr.kind {
            ExprKind::Block { .. } => "block".to_string(),
            ExprKind::Path { path } => format!("path {path}"),
            ExprKind::Call { .. } => "call".to_string(),
            ExprKind::Tuple { .. } => "tuple".to_string(),
            ExprKind::Array { .. } => "array".to_string(),
            ExprKind::RepeatArray { .. } => "repeat_array".to_string(),
            ExprKind::Index { .. } => "index".to_string(),
            ExprKind::Range { kind, .. } => {
                let kind = kind
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("range {kind}")
            }
            ExprKind::Cast { ty, .. } => {
                let ty = ty
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("cast as {ty}")
            }
            ExprKind::Unary { op, .. } => {
                let op = op
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("unary {op}")
            }
            ExprKind::Binary { op, .. } => {
                let op = op
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("binary {op}")
            }
            ExprKind::Assign { op, .. } => {
                let op = op
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("assign {op}")
            }
            ExprKind::Match { .. } => "match".to_string(),
            ExprKind::If { .. } => "if".to_string(),
            ExprKind::Let { .. } => "let".to_string(),
            ExprKind::Closure { .. } => "closure".to_string(),
            ExprKind::Loop { .. } => "loop".to_string(),
            ExprKind::While { .. } => "while".to_string(),
            ExprKind::For { .. } => "for".to_string(),
            ExprKind::Break { .. } => "break".to_string(),
            ExprKind::Continue { .. } => "continue".to_string(),
            ExprKind::MethodCall { method_name, .. } => {
                format!("method_call {method_name}")
            }
            ExprKind::Field { field, .. } => {
                let field = field
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("field {field}")
            }
            ExprKind::Record { path, .. } => {
                let path = path
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("record {path}")
            }
            ExprKind::Wrapper { kind, .. } => format!("wrapper {kind}"),
            ExprKind::BuiltinMacro { kind, .. } => format!("builtin_macro {kind}"),
            ExprKind::Literal { kind } => {
                format!("literal {kind} {}", self.render_source_text(source))
            }
            ExprKind::Underscore => "underscore".to_string(),
            ExprKind::Yield { .. } => "yield".to_string(),
            ExprKind::Yeet { .. } => "yeet".to_string(),
            ExprKind::Become { .. } => "become".to_string(),
            ExprKind::Unknown { .. } => {
                format!("unknown {}", self.render_source_text(source))
            }
        };

        format!(
            "expr {label} @ {}",
            self.render_source_span(source.file_id, source.span)
        )
    }

    fn render_targets(&self, mut targets: Vec<NavigationTarget>, dump: &mut String) {
        targets.sort_by_key(|target| {
            (
                target.kind,
                target.name.clone(),
                target.crate_ref.package.0,
                target.crate_ref.crate_id.0,
                target.file_id,
                target.span.map(|span| span.start),
            )
        });

        if targets.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        }

        for target in targets {
            let label = if target.kind == NavigationTargetKind::Impl {
                target.name
            } else {
                format!("{} {}", target.kind, target.name)
            };
            writeln!(
                dump,
                "{label} @ {}",
                self.render_optional_span(target.file_id, target.span)
            )
            .expect("string writes should not fail");
        }
    }

    fn completion_kind_tag(kind: CompletionKind) -> &'static str {
        match kind {
            CompletionKind::Attribute => "at",
            CompletionKind::Const => "ct",
            CompletionKind::Enum => "en",
            CompletionKind::EnumVariant => "ev",
            CompletionKind::Field => "fd",
            CompletionKind::Function => "fn",
            CompletionKind::InherentMethod => "im",
            CompletionKind::Keyword => "kw",
            CompletionKind::Label => "lb",
            CompletionKind::Lifetime => "lt",
            CompletionKind::Macro => "ma",
            CompletionKind::Module => "md",
            CompletionKind::PrimitiveType => "pt",
            CompletionKind::Postfix => "po",
            CompletionKind::Static => "sc",
            CompletionKind::Struct => "st",
            CompletionKind::Trait => "tt",
            CompletionKind::TraitMethod => "tm",
            CompletionKind::TypeParameter => "tp",
            CompletionKind::TypeAlias => "ta",
            CompletionKind::Union => "un",
            CompletionKind::Variable => "vr",
            CompletionKind::Value => "vl",
        }
    }

    fn render_completions(&self, mut completions: Vec<CompletionItem>, dump: &mut String) {
        completions.sort_by_key(|completion| {
            (
                completion.label.clone(),
                completion.kind,
                completion.applicability,
            )
        });

        if completions.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        }

        for completion in completions {
            if completion.applicability == CompletionApplicability::Known {
                writeln!(
                    dump,
                    "{} {}",
                    Self::completion_kind_tag(completion.kind),
                    completion.label
                )
                .expect("string writes should not fail");
            } else {
                writeln!(
                    dump,
                    "{} {} ({})",
                    Self::completion_kind_tag(completion.kind),
                    completion.label,
                    completion.applicability
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_code_actions(&self, actions: Vec<CodeAction>, source: &str, dump: &mut String) {
        for action in actions {
            if !dump.is_empty() {
                writeln!(dump).expect("string writes should not fail");
            }
            let kind = match action.kind {
                CodeActionKind::QuickFix => "quickfix",
                CodeActionKind::RefactorRewrite => "refactor.rewrite",
            };
            writeln!(dump, "{kind} {}", action.title).expect("string writes should not fail");
            writeln!(dump, "  preferred: {}", action.is_preferred)
                .expect("string writes should not fail");

            // Apply from the end so every span remains in the captured source coordinate space.
            let mut edits = action.edits.iter().collect::<Vec<_>>();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.replace.start));
            let mut result = source.to_string();
            for edit in edits {
                result.replace_range(
                    usize::try_from(edit.replace.start)
                        .expect("fixture edit start should fit usize")
                        ..usize::try_from(edit.replace.end)
                            .expect("fixture edit end should fit usize"),
                    &edit.new_text,
                );
            }
            writeln!(dump, "  result:").expect("string writes should not fail");
            for line in result.lines() {
                if line.is_empty() {
                    writeln!(dump).expect("string writes should not fail");
                } else {
                    writeln!(dump, "    {line}").expect("string writes should not fail");
                }
            }
        }
    }

    fn render_completions_verbose(&self, mut completions: Vec<CompletionItem>, dump: &mut String) {
        completions.sort_by_key(|completion| completion.sort_text.clone());

        if completions.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        }

        for completion in completions {
            writeln!(dump, "{} {}", completion.kind, completion.label)
                .expect("string writes should not fail");
            if let Some(detail) = &completion.detail {
                writeln!(dump, "  detail: {detail}").expect("string writes should not fail");
            }
            if let Some(docs) = &completion.documentation {
                writeln!(dump, "  docs: {docs}").expect("string writes should not fail");
            }
            if let Some(filter_text) = &completion.filter_text {
                writeln!(dump, "  filter: {filter_text}").expect("string writes should not fail");
            }
            writeln!(dump, "  sort: {}", completion.sort_text)
                .expect("string writes should not fail");
            if let Some(edit) = completion.edit {
                let span = edit.replace;
                writeln!(dump, "  replace: {}..{}", span.start, span.end)
                    .expect("string writes should not fail");
            }
            for edit in &completion.additional_edits {
                writeln!(
                    dump,
                    "  additional: {}..{} => {:?}",
                    edit.replace.start, edit.replace.end, edit.new_text
                )
                .expect("string writes should not fail");
            }
            match &completion.insert_text {
                CompletionInsertText::Plain => {}
                CompletionInsertText::Text(text) => {
                    writeln!(dump, "  insert: {text}").expect("string writes should not fail");
                }
                CompletionInsertText::Snippet(snippet) => {
                    writeln!(dump, "  snippet: {snippet}").expect("string writes should not fail");
                }
            }
        }
    }

    fn render_references(&self, references: Vec<ReferenceLocation>, dump: &mut String) {
        if references.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        }

        for reference in references {
            writeln!(
                dump,
                "`{}` @ {}",
                self.render_source_text_for_span(reference.file_id, reference.span),
                self.render_file_span(
                    reference.crate_ref.package,
                    reference.file_id,
                    reference.span,
                )
            )
            .expect("string writes should not fail");
        }
    }

    fn render_rename_target(
        &self,
        target: Option<RenameTarget>,
        package: PackageSlot,
        dump: &mut String,
    ) {
        let Some(target) = target else {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "`{}` @ {}",
            target.placeholder,
            self.render_file_span(package, target.file_id, target.span)
        )
        .expect("string writes should not fail");
    }

    fn render_rename_result(
        &self,
        result: Option<RenameResult>,
        package: PackageSlot,
        dump: &mut String,
    ) {
        let Some(result) = result else {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "target `{}` @ {}",
            result.target.placeholder,
            self.render_file_span(package, result.target.file_id, result.target.span)
        )
        .expect("string writes should not fail");

        let mut edits = result.edits;
        edits.sort_by_key(|edit| {
            (
                edit.crate_ref.package.0,
                edit.crate_ref.crate_id.0,
                edit.file_id,
                edit.span.start,
            )
        });

        for edit in edits {
            self.render_rename_edit(edit, dump);
        }
    }

    fn render_rename_edit(&self, edit: RenameEdit, dump: &mut String) {
        writeln!(
            dump,
            "`{}` -> `{}` @ {}",
            edit.old_text,
            edit.new_text,
            self.render_file_span(edit.crate_ref.package, edit.file_id, edit.span)
        )
        .expect("string writes should not fail");
    }

    fn render_hover(&self, hover: Option<HoverInfo>, file_id: FileId, dump: &mut String) {
        let Some(hover) = hover else {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "range: {}",
            self.render_optional_span(file_id, hover.range)
        )
        .expect("string writes should not fail");
        for block in hover.blocks {
            writeln!(dump, "block:").expect("string writes should not fail");
            writeln!(dump, "  kind: {}", block.kind).expect("string writes should not fail");
            if let Some(path) = block.path {
                writeln!(dump, "  path: {path}").expect("string writes should not fail");
            }
            if let Some(signature) = block.signature {
                writeln!(dump, "  signature:").expect("string writes should not fail");
                for line in signature.lines() {
                    writeln!(dump, "    {line}").expect("string writes should not fail");
                }
            }
            if let Some(ty) = block.ty {
                writeln!(dump, "  type: {ty}").expect("string writes should not fail");
            }
            if let Some(docs) = block.docs {
                writeln!(dump, "  docs:").expect("string writes should not fail");
                for line in docs.lines() {
                    writeln!(dump, "    {line}").expect("string writes should not fail");
                }
            }
        }
    }

    fn render_optional_span(&self, file_id: FileId, span: Option<Span>) -> String {
        span.map(|span| self.render_source_span(file_id, span))
            .unwrap_or_else(|| "<root>".to_string())
    }

    fn render_source_span(&self, file_id: FileId, span: Span) -> String {
        let line_column = SourceReader::new(self.db.semantic.db())
            .line_index(file_id)
            .expect("span file line index should load while rendering analysis query")
            .line_column_span(span);
        format!(
            "{}:{}-{}:{}",
            line_column.start.line + 1,
            line_column.start.column + 1,
            line_column.end.line + 1,
            line_column.end.column + 1,
        )
    }

    fn render_source_text(&self, source: BodySource) -> String {
        self.render_source_text_for_span(source.file_id, source.span)
    }

    fn render_source_text_for_span(&self, file_id: FileId, span: Span) -> String {
        SourceReader::new(self.db.semantic.db())
            .text_for_span(file_id, span)
            .expect("analysis source text should load")
            .unwrap_or_else(|| "<invalid>".to_string())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn render_file_span(&self, package: PackageSlot, file_id: FileId, span: Span) -> String {
        format!(
            "{}:{}",
            self.render_file_path(package, file_id),
            self.render_source_span(file_id, span)
        )
    }

    fn render_file_path(&self, package: PackageSlot, file_id: FileId) -> String {
        let metadata = self
            .db
            .semantic
            .workspace()
            .packages()
            .get(package.0)
            .expect("reference package should exist while rendering file path");
        let relative_path = self.db.render_file_path(package, file_id);

        if self.db.semantic.workspace().packages().len() > 1 {
            format!("{}/{relative_path}", metadata.name)
        } else {
            relative_path
        }
    }
}

struct AnalysisSymbolSnapshot<'db> {
    db: AnalysisFixture<'db>,
}

impl AnalysisSymbolSnapshot<'_> {
    fn render_workspace_symbols(&self, query: &str) -> String {
        let symbols = self
            .db
            .analysis()
            .workspace_symbols(query)
            .expect("fixture workspace symbols should resolve");
        let mut dump = String::new();

        if symbols.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return dump;
        }

        for symbol in symbols {
            self.render_workspace_symbol(&symbol, &mut dump);
        }
        dump
    }

    fn render_inlay_hints(&self, query: &InlayHintsQuery) -> String {
        let (target, file_id) = self.db.target_and_file_for_path(&query.target, query.path);
        let text = SourceReader::new(self.db.semantic.db())
            .text(file_id)
            .expect("fixture source");
        let range = Span::full_file(text.len() as u32);
        let hints = self
            .db
            .analysis()
            .inlay_hints(target, file_id, range, None)
            .expect("fixture inlay hints should resolve");
        let mut dump = String::new();

        if hints.is_empty() {
            writeln!(dump, "<none>").expect("string writes should not fail");
            return dump;
        }

        for hint in hints {
            self.render_inlay_hint(&hint, &mut dump);
        }
        dump
    }

    fn render_workspace_symbol(&self, symbol: &WorkspaceSymbol, dump: &mut String) {
        let container = symbol
            .container_name
            .as_ref()
            .map(|container| format!(" in {container}"))
            .unwrap_or_default();

        writeln!(
            dump,
            "{} {}{} @ {} {}",
            symbol.kind,
            symbol.name,
            container,
            self.render_crate_ref(symbol.crate_ref),
            self.render_file_span(symbol.crate_ref.package, symbol.file_id, symbol.span)
        )
        .expect("string writes should not fail");
    }

    fn render_inlay_hint(&self, hint: &InlayHint, dump: &mut String) {
        writeln!(
            dump,
            "`{}` @ {}",
            hint.label,
            self.render_source_span(hint.file_id, hint.span)
        )
        .expect("string writes should not fail");
    }

    fn render_crate_ref(&self, crate_ref: CrateRef) -> String {
        let package = self
            .db
            .semantic
            .workspace()
            .packages()
            .get(crate_ref.package.0)
            .expect("target package should exist while rendering workspace symbol");
        let target = self
            .db
            .semantic
            .workspace()
            .target(crate_ref)
            .expect("fixture target should exist");

        format!("{}[{}]", package.name, target.kind)
    }

    fn render_file_span(
        &self,
        package: PackageSlot,
        file_id: FileId,
        span: Option<Span>,
    ) -> String {
        let file = self.render_file_path(package, file_id);
        match span {
            Some(span) => format!("{file}:{}", self.render_source_span(file_id, span)),
            None => format!("{file}:<root>"),
        }
    }

    fn render_file_path(&self, package: PackageSlot, file_id: FileId) -> String {
        self.db.render_file_path(package, file_id)
    }

    fn render_source_span(&self, file_id: FileId, span: Span) -> String {
        let line_column = SourceReader::new(self.db.semantic.db())
            .line_index(file_id)
            .expect("span file line index should load while rendering analysis symbol")
            .line_column_span(span);
        format!(
            "{}:{}-{}:{}",
            line_column.start.line + 1,
            line_column.start.column + 1,
            line_column.end.line + 1,
            line_column.end.column + 1,
        )
    }
}
