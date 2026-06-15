use std::fmt::Write as _;

use expect_test::Expect;

use crate::{
    Analysis, CompletionApplicability, CompletionClientCapabilities, CompletionInsertText,
    CompletionItem, CompletionQuery, DocumentSymbol, HoverInfo, InlayHint, NavigationTarget,
    ReferenceLocation, ReferenceQuery as AnalysisReferenceQuery, RenameEdit, RenameResult,
    RenameTarget, SourceTextView, SymbolAt, WorkspaceSymbol,
};
use rg_def_map::testonly::DefMapFixture;
use rg_ir_model::{BodySource, ExprData, ExprKind, PackageSlot, TargetRef};
use rg_ir_view::testonly::ViewFixture;
use rg_parse::{FileId, ParseDb, Span};
use rg_semantic_ir::testonly::SemanticIrFixture;
use rg_ty::{GenericArg, NominalTy, OpaqueTraitBound, Ty};
use rg_workspace::{SysrootSources, TargetKind, WorkspaceMetadata};
use test_fixture::{CrateFixture, FixtureMarkers, fixture_crate, fixture_crate_with_markers};

pub(super) fn check_analysis_queries(fixture: &str, queries: &[AnalysisQuery], expect: Expect) {
    let (fixture, markers) = fixture_crate_with_markers(fixture);
    let db = AnalysisFixtureDb::build_from_crate(fixture);
    let renderer = AnalysisQuerySnapshot::new(&db, markers, queries);
    let actual = format!("{}\n", renderer.render().trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_analysis_queries_with_sysroot(
    fixture: &str,
    queries: &[AnalysisQuery],
    expect: Expect,
) {
    let (fixture, markers) = fixture_crate_with_markers(fixture);
    let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
        .expect("fixture sysroot should be complete");
    let workspace = WorkspaceMetadata::for_tests(fixture.metadata())
        .expect("fixture workspace metadata should build")
        .with_sysroot_sources(Some(sysroot));
    let db = AnalysisFixtureDb::build_from_crate_with_workspace(fixture, workspace);
    let renderer = AnalysisQuerySnapshot::new(&db, markers, queries);
    let actual = format!("{}\n", renderer.render().trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_document_symbols(fixture: &str, query: DocumentSymbolsQuery, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build_from_crate(fixture);
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_document_symbols(&query).trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_workspace_symbols(fixture: &str, query: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build_from_crate(fixture);
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_workspace_symbols(query).trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_inlay_hints(fixture: &str, query: InlayHintsQuery, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build_from_crate(fixture);
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_inlay_hints(&query).trim_end());
    expect.assert_eq(&actual);
}

pub(super) struct AnalysisQuery {
    title: &'static str,
    marker: &'static str,
    target: AnalysisTarget,
    kind: AnalysisQueryKind,
}

impl AnalysisQuery {
    pub(super) fn symbol(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::SymbolAt)
    }

    pub(super) fn resolve(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::ResolveSymbol)
    }

    pub(super) fn goto(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoDefinition)
    }

    pub(super) fn goto_type(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoTypeDefinition)
    }

    pub(super) fn goto_impl(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::GotoImplementation)
    }

    pub(super) fn ty(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::TypeAt)
    }

    pub(super) fn complete(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAt)
    }

    pub(super) fn complete_verbose(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAtVerbose)
    }

    pub(super) fn hover(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::Hover)
    }

    pub(super) fn references(
        title: &'static str,
        marker: &'static str,
        query: ReferenceQuery,
    ) -> Self {
        Self::new(title, marker, AnalysisQueryKind::References(query))
    }

    pub(super) fn prepare_rename(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::PrepareRename)
    }

    pub(super) fn rename(
        title: &'static str,
        marker: &'static str,
        new_name: &'static str,
    ) -> Self {
        Self::new(title, marker, AnalysisQueryKind::Rename(new_name))
    }

    pub(super) fn in_bin(mut self, package_name: &'static str) -> Self {
        self.target = AnalysisTarget::bin(package_name);
        self
    }

    pub(super) fn in_lib(mut self, package_name: &'static str) -> Self {
        self.target = AnalysisTarget::lib_package(package_name);
        self
    }

    fn new(title: &'static str, marker: &'static str, kind: AnalysisQueryKind) -> Self {
        Self {
            title,
            marker,
            target: AnalysisTarget::lib(),
            kind,
        }
    }
}

pub(super) struct DocumentSymbolsQuery {
    title: &'static str,
    path: &'static str,
    target: AnalysisTarget,
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

impl DocumentSymbolsQuery {
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
}

#[derive(Debug, Clone)]
struct AnalysisTarget {
    package_name: Option<&'static str>,
    kind: TargetKind,
}

impl AnalysisTarget {
    fn lib() -> Self {
        Self {
            package_name: None,
            kind: TargetKind::Lib,
        }
    }

    fn lib_package(package_name: &'static str) -> Self {
        Self {
            package_name: Some(package_name),
            kind: TargetKind::Lib,
        }
    }

    fn bin(package_name: &'static str) -> Self {
        Self {
            package_name: Some(package_name),
            kind: TargetKind::Bin,
        }
    }

    fn matches_package(&self, package_name: &str) -> bool {
        self.package_name
            .is_none_or(|expected| expected == package_name)
    }
}

#[derive(Debug, Clone, Copy)]
enum AnalysisQueryKind {
    SymbolAt,
    ResolveSymbol,
    GotoDefinition,
    GotoTypeDefinition,
    GotoImplementation,
    References(ReferenceQuery),
    PrepareRename,
    Rename(&'static str),
    TypeAt,
    CompletionsAt,
    CompletionsAtVerbose,
    Hover,
}

#[derive(Debug, Clone, Copy)]
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

    pub(super) fn libs(packages: &'static [&'static str]) -> Self {
        Self {
            include_declaration: true,
            scope: ReferenceQueryScope::LibTargets(packages),
        }
    }

    pub(super) fn without_declaration(mut self) -> Self {
        self.include_declaration = false;
        self
    }
}

#[derive(Debug, Clone, Copy)]
enum ReferenceQueryScope {
    AllIncludedTargets,
    CurrentTarget,
    CurrentFile,
    LibTargets(&'static [&'static str]),
}

struct AnalysisFixtureDb {
    fixture: ViewFixture,
}

impl AnalysisFixtureDb {
    fn build_from_crate(fixture: CrateFixture) -> Self {
        let workspace = WorkspaceMetadata::for_tests(fixture.metadata())
            .expect("fixture workspace metadata should build");
        Self::build_from_crate_with_workspace(fixture, workspace)
    }

    fn build_from_crate_with_workspace(
        fixture: CrateFixture,
        workspace: WorkspaceMetadata,
    ) -> Self {
        let def_map = DefMapFixture::build_from_crate(fixture, workspace);
        let semantic_ir = SemanticIrFixture::build_from_def_map(def_map);
        Self {
            fixture: ViewFixture::build_from_semantic_ir(semantic_ir),
        }
    }

    fn analysis(&self) -> Analysis<'_> {
        Analysis::new(
            self.fixture.view_db(),
            SourceTextView::new(self.fixture.parse_db()),
        )
    }

    fn parse_db(&self) -> &ParseDb {
        self.fixture.parse_db()
    }

    fn target_and_file_for_path(
        &self,
        selected: &AnalysisTarget,
        path: &str,
    ) -> (TargetRef, FileId) {
        let mut matches = Vec::new();
        let normalized_path = path.trim_start_matches('/');

        for (package_slot, package) in self.parse_db().packages().iter().enumerate() {
            if !selected.matches_package(package.package_name()) {
                continue;
            }

            for target in package
                .targets()
                .iter()
                .filter(|target| target.kind == selected.kind)
            {
                let Some(file_id) = package
                    .parsed_files()
                    .find(|file| file.path().ends_with(normalized_path))
                    .map(|file| file.file_id())
                else {
                    continue;
                };

                let target_ref = TargetRef {
                    package: PackageSlot(package_slot),
                    target: target.id,
                };
                if self.target_owns_file(target_ref, file_id) {
                    matches.push((target_ref, file_id));
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

    fn target_for(&self, selected: &AnalysisTarget) -> TargetRef {
        let mut matches = Vec::new();

        for (package_slot, package) in self.parse_db().packages().iter().enumerate() {
            if !selected.matches_package(package.package_name()) {
                continue;
            }

            for target in package
                .targets()
                .iter()
                .filter(|target| target.kind == selected.kind)
            {
                matches.push(TargetRef {
                    package: PackageSlot(package_slot),
                    target: target.id,
                });
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

    fn all_targets(&self) -> Vec<TargetRef> {
        self.parse_db()
            .packages()
            .iter()
            .enumerate()
            .flat_map(|(package_slot, package)| {
                package.targets().iter().map(move |target| TargetRef {
                    package: PackageSlot(package_slot),
                    target: target.id,
                })
            })
            .collect()
    }

    fn target_owns_file(&self, target: TargetRef, file_id: FileId) -> bool {
        self.fixture.target_owns_file(target, file_id)
    }
}

struct AnalysisQuerySnapshot<'a> {
    db: &'a AnalysisFixtureDb,
    markers: FixtureMarkers,
    queries: &'a [AnalysisQuery],
}

impl<'a> AnalysisQuerySnapshot<'a> {
    fn new(
        db: &'a AnalysisFixtureDb,
        markers: FixtureMarkers,
        queries: &'a [AnalysisQuery],
    ) -> Self {
        Self {
            db,
            markers,
            queries,
        }
    }

    fn render(&self) -> String {
        self.queries
            .iter()
            .map(|query| self.render_query(query).trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn render_query(&self, query: &AnalysisQuery) -> String {
        let (target, file_id, offset) = self.query_location(query);
        let mut dump = query.title.to_string();
        match query.kind {
            AnalysisQueryKind::SymbolAt => {
                self.render_symbol(
                    self.db
                        .analysis()
                        .symbol_at(target, file_id, offset)
                        .expect("fixture symbol query should resolve"),
                    target.package,
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
            AnalysisQueryKind::References(query) => {
                let references = match query.scope {
                    ReferenceQueryScope::AllIncludedTargets => {
                        let use_site_targets = self.db.all_targets();
                        let reference_query = AnalysisReferenceQuery::find_references(
                            &use_site_targets,
                            query.include_declaration,
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
                            query.include_declaration,
                        );
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture scoped references query should resolve")
                    }
                    ReferenceQueryScope::CurrentFile => {
                        let reference_query = AnalysisReferenceQuery::file_scoped(target, file_id);
                        let reference_query = if query.include_declaration {
                            reference_query
                        } else {
                            reference_query.without_declarations()
                        };
                        self.db
                            .analysis()
                            .references(target, file_id, offset, reference_query)
                            .expect("fixture file-scoped references query should resolve")
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
                            query.include_declaration,
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
                    AnalysisReferenceQuery::find_references(&use_site_targets, true);
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
                    "\n- {}",
                    ty.as_ref()
                        .map(|ty| self.render_ty(ty))
                        .unwrap_or_else(|| "<none>".to_string())
                )
                .expect("string writes should not fail");
            }
            AnalysisQueryKind::CompletionsAt => {
                let query = CompletionQuery::new(target, file_id, offset).with_client_capabilities(
                    CompletionClientCapabilities::default().with_snippet_support(true),
                );
                self.render_completions(
                    self.db
                        .analysis()
                        .completions_at(query)
                        .expect("fixture completion query should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::CompletionsAtVerbose => {
                let query = CompletionQuery::new(target, file_id, offset).with_client_capabilities(
                    CompletionClientCapabilities::default().with_snippet_support(true),
                );
                self.render_completions_verbose(
                    self.db
                        .analysis()
                        .completions_at(query)
                        .expect("fixture completion query should resolve"),
                    &mut dump,
                );
            }
            AnalysisQueryKind::Hover => {
                self.render_hover(
                    self.db
                        .analysis()
                        .hover(target, file_id, offset)
                        .expect("fixture hover query should resolve"),
                    target.package,
                    file_id,
                    &mut dump,
                );
            }
        }

        dump
    }

    fn query_location(&self, query: &AnalysisQuery) -> (TargetRef, FileId, u32) {
        let marker = self.markers.position(query.marker);
        let (target, file_id) = self
            .db
            .target_and_file_for_path(&query.target, &marker.path);

        (target, file_id, marker.offset)
    }

    fn render_symbol(
        &self,
        symbol: Option<SymbolAt>,
        package: PackageSlot,
        file_id: FileId,
        dump: &mut String,
    ) {
        let Some(symbol) = symbol else {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        };

        match symbol {
            SymbolAt::FunctionBody { body } => {
                let body = body.body_ir();
                let source = self
                    .db
                    .fixture
                    .resident_body_source(body)
                    .expect("body source should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "\n- body @ {}",
                    self.render_source_span(body.target.package, source.file_id, source.span)
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
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Expr { expr } => {
                let body = expr.body_ir();
                let expr_data = self
                    .db
                    .fixture
                    .resident_expr(body, expr.expr_id())
                    .expect("expr id should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "\n- {}",
                    self.render_expr_symbol(body.target.package, expr_data)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::TypePath { ref path, span, .. } => {
                writeln!(
                    dump,
                    "\n- type path {path} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::ValuePath { ref path, span, .. } => {
                writeln!(
                    dump,
                    "\n- value path {path} @ {}",
                    self.render_source_span(package, file_id, span)
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
                    "\n- record field {owner}::{} @ {}",
                    key.declaration_label(),
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::UsePath { ref path, span, .. } => {
                writeln!(
                    dump,
                    "\n- path {path} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_expr_symbol(&self, package: PackageSlot, expr: &ExprData) -> String {
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
            ExprKind::Literal { kind } => {
                format!(
                    "literal {kind} {}",
                    self.render_source_text(package, expr.source)
                )
            }
            ExprKind::Underscore => "underscore".to_string(),
            ExprKind::Yield { .. } => "yield".to_string(),
            ExprKind::Yeet { .. } => "yeet".to_string(),
            ExprKind::Become { .. } => "become".to_string(),
            ExprKind::Unknown { .. } => {
                format!("unknown {}", self.render_source_text(package, expr.source))
            }
        };

        format!(
            "expr {label} @ {}",
            self.render_source_span(package, expr.source.file_id, expr.source.span)
        )
    }

    fn render_targets(&self, mut targets: Vec<NavigationTarget>, dump: &mut String) {
        targets.sort_by_key(|target| {
            (
                target.kind,
                target.name.clone(),
                target.target.package.0,
                target.target.target.0,
                target.file_id.0,
                target.span.map(|span| span.text.start),
            )
        });

        if targets.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        }

        writeln!(dump).expect("string writes should not fail");
        for target in targets {
            let label = if target.kind == crate::NavigationTargetKind::Impl {
                target.name
            } else {
                format!("{} {}", target.kind, target.name)
            };
            writeln!(
                dump,
                "- {label} @ {}",
                self.render_optional_span(target.target.package, target.file_id, target.span)
            )
            .expect("string writes should not fail");
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
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        }

        writeln!(dump).expect("string writes should not fail");
        for completion in completions {
            if completion.applicability == CompletionApplicability::Known {
                writeln!(dump, "- {} {}", completion.kind, completion.label)
                    .expect("string writes should not fail");
            } else {
                writeln!(
                    dump,
                    "- {} {} ({})",
                    completion.kind, completion.label, completion.applicability
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_completions_verbose(&self, mut completions: Vec<CompletionItem>, dump: &mut String) {
        completions.sort_by_key(|completion| completion.sort_text.clone());

        if completions.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        }

        writeln!(dump).expect("string writes should not fail");
        for completion in completions {
            writeln!(dump, "- {} {}", completion.kind, completion.label)
                .expect("string writes should not fail");
            if let Some(detail) = &completion.detail {
                writeln!(dump, "  detail: {detail}").expect("string writes should not fail");
            }
            if let Some(docs) = &completion.documentation {
                writeln!(dump, "  docs: {docs}").expect("string writes should not fail");
            }
            writeln!(dump, "  sort: {}", completion.sort_text)
                .expect("string writes should not fail");
            if let Some(edit) = completion.edit {
                let span = edit.replace;
                writeln!(dump, "  replace: {}..{}", span.text.start, span.text.end)
                    .expect("string writes should not fail");
            }
            if let CompletionInsertText::Snippet(snippet) = &completion.insert_text {
                writeln!(dump, "  snippet: {snippet}").expect("string writes should not fail");
            }
        }
    }

    fn render_references(&self, references: Vec<ReferenceLocation>, dump: &mut String) {
        if references.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        }

        writeln!(dump).expect("string writes should not fail");
        for reference in references {
            writeln!(
                dump,
                "- `{}` @ {}",
                self.render_source_text_for_span(
                    reference.target.package,
                    reference.file_id,
                    reference.span,
                ),
                self.render_file_span(reference.target.package, reference.file_id, reference.span,)
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
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "\n- `{}` @ {}",
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
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "\n- target `{}` @ {}",
            result.target.placeholder,
            self.render_file_span(package, result.target.file_id, result.target.span)
        )
        .expect("string writes should not fail");

        let mut edits = result.edits;
        edits.sort_by_key(|edit| {
            (
                edit.target.package.0,
                edit.target.target.0,
                edit.file_id.0,
                edit.span.text.start,
            )
        });

        for edit in edits {
            self.render_rename_edit(edit, dump);
        }
    }

    fn render_rename_edit(&self, edit: RenameEdit, dump: &mut String) {
        writeln!(
            dump,
            "- `{}` -> `{}` @ {}",
            edit.old_text,
            edit.new_text,
            self.render_file_span(edit.target.package, edit.file_id, edit.span)
        )
        .expect("string writes should not fail");
    }

    fn render_hover(
        &self,
        hover: Option<HoverInfo>,
        package: PackageSlot,
        file_id: FileId,
        dump: &mut String,
    ) {
        let Some(hover) = hover else {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return;
        };

        writeln!(
            dump,
            "\n- range: {}",
            self.render_optional_span(package, file_id, hover.range)
        )
        .expect("string writes should not fail");
        for block in hover.blocks {
            writeln!(dump, "- block:").expect("string writes should not fail");
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

    fn render_ty(&self, ty: &Ty) -> String {
        match ty {
            Ty::Unit => "()".to_string(),
            Ty::Never => "!".to_string(),
            Ty::Primitive(primitive) => primitive.label().to_string(),
            Ty::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|ty| self.render_ty(ty))
                    .collect::<Vec<_>>();
                let suffix = if fields.len() == 1 { "," } else { "" };
                format!("({}{suffix})", fields.join(", "))
            }
            Ty::Array { inner, len } => format!(
                "[{}; {}]",
                self.render_ty(inner),
                len.as_deref().unwrap_or("<unknown>")
            ),
            Ty::Slice(inner) => format!("[{}]", self.render_ty(inner)),
            Ty::Syntax(ty) => format!("syntax {ty}"),
            Ty::Reference { mutability, inner } => {
                format!("{}{}", mutability.render_prefix(), self.render_ty(inner))
            }
            Ty::Opaque { bounds } => {
                let mut bounds = bounds
                    .iter()
                    .map(|bound| self.render_opaque_bound(bound))
                    .collect::<Vec<_>>();
                bounds.sort();
                format!("impl {}", bounds.join(" + "))
            }
            Ty::Nominal(ty) => format!("nominal {}", self.render_body_nominal_ty(ty)),
            Ty::SelfTy(ty) => format!("Self {}", self.render_body_nominal_ty(ty)),
            Ty::Unknown => "<unknown>".to_string(),
        }
    }

    fn render_opaque_bound(&self, bound: &OpaqueTraitBound) -> String {
        format!(
            "{}{}",
            self.db.fixture.render_trait_ref(bound.trait_ref),
            self.render_generic_args(&bound.args)
        )
    }

    fn render_body_nominal_ty(&self, ty: &NominalTy) -> String {
        format!(
            "{}{}",
            self.db.fixture.render_type_def_ref(ty.def),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_generic_args(&self, args: &[GenericArg]) -> String {
        if args.is_empty() {
            return String::new();
        }

        format!(
            "<{}>",
            args.iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.render_ty(ty),
            GenericArg::Lifetime(lifetime) => lifetime.clone(),
            GenericArg::Const(value) => value.clone(),
            GenericArg::FnTraitArgs { params, ret } => {
                let params = params
                    .iter()
                    .map(|ty| self.render_ty(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut text = format!("({params})");
                if !matches!(ret.as_ref(), Ty::Unit) {
                    text.push_str(" -> ");
                    text.push_str(&self.render_ty(ret));
                }
                text
            }
            GenericArg::AssocType { name, ty } => match ty {
                Some(ty) => format!("{name} = {}", self.render_ty(ty)),
                None => name.to_string(),
            },
            GenericArg::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }

    fn render_optional_span(
        &self,
        package: PackageSlot,
        file_id: FileId,
        span: Option<Span>,
    ) -> String {
        span.map(|span| self.render_source_span(package, file_id, span))
            .unwrap_or_else(|| "<root>".to_string())
    }

    fn render_source_span(&self, package: PackageSlot, file_id: FileId, span: Span) -> String {
        let line_column = span.line_column(
            self.db
                .parse_db()
                .package(package.0)
                .expect("span package should exist while rendering analysis query")
                .parsed_file(file_id)
                .expect("span file should exist while rendering analysis query")
                .line_index()
                .expect("span file line index should load while rendering analysis query"),
        );
        format!(
            "{}:{}-{}:{}",
            line_column.start.line + 1,
            line_column.start.column + 1,
            line_column.end.line + 1,
            line_column.end.column + 1,
        )
    }

    fn render_source_text(&self, package: PackageSlot, source: BodySource) -> String {
        self.render_source_text_for_span(package, source.file_id, source.span)
    }

    fn render_source_text_for_span(
        &self,
        package: PackageSlot,
        file_id: FileId,
        span: Span,
    ) -> String {
        let parsed_file = self
            .db
            .parse_db()
            .package(package.0)
            .expect("span package should exist while rendering analysis query text")
            .parsed_file(file_id)
            .expect("span file should exist while rendering analysis query text");

        parsed_file
            .text_for_span(span)
            .unwrap_or_else(|| "<invalid>".to_string())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn render_file_span(&self, package: PackageSlot, file_id: FileId, span: Span) -> String {
        format!(
            "{}:{}",
            self.render_file_path(package, file_id),
            self.render_source_span(package, file_id, span)
        )
    }

    fn render_file_path(&self, package: PackageSlot, file_id: FileId) -> String {
        let package = self
            .db
            .parse_db()
            .packages()
            .get(package.0)
            .expect("reference package should exist while rendering file path");
        let path = package
            .file_path(file_id)
            .expect("reference file should exist while rendering file path");
        let path = path.to_string_lossy();

        let relative_path = path
            .rfind("/src/")
            .map(|idx| path[idx + 1..].to_string())
            .unwrap_or_else(|| {
                path.rsplit('/')
                    .next()
                    .expect("path string should contain a file name")
                    .to_string()
            });

        if self.db.parse_db().packages().len() > 1 {
            format!("{}/{relative_path}", package.package_name())
        } else {
            relative_path
        }
    }
}

struct AnalysisSymbolSnapshot<'a> {
    db: &'a AnalysisFixtureDb,
}

impl<'a> AnalysisSymbolSnapshot<'a> {
    fn new(db: &'a AnalysisFixtureDb) -> Self {
        Self { db }
    }

    fn render_document_symbols(&self, query: &DocumentSymbolsQuery) -> String {
        let (target, file_id) = self.db.target_and_file_for_path(&query.target, query.path);
        let symbols = self
            .db
            .analysis()
            .document_symbols(target, file_id)
            .expect("fixture document symbols should resolve");
        let mut dump = query.title.to_string();

        if symbols.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return dump;
        }

        writeln!(dump).expect("string writes should not fail");
        self.render_document_symbol_list(target.package, &symbols, 0, &mut dump);
        dump
    }

    fn render_workspace_symbols(&self, query: &str) -> String {
        let symbols = self
            .db
            .analysis()
            .workspace_symbols(query)
            .expect("fixture workspace symbols should resolve");
        let mut dump = format!("workspace symbols `{query}`");

        if symbols.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return dump;
        }

        writeln!(dump).expect("string writes should not fail");
        for symbol in symbols {
            self.render_workspace_symbol(&symbol, &mut dump);
        }
        dump
    }

    fn render_inlay_hints(&self, query: &InlayHintsQuery) -> String {
        let (target, file_id) = self.db.target_and_file_for_path(&query.target, query.path);
        let hints = self
            .db
            .analysis()
            .inlay_hints(target, file_id, None)
            .expect("fixture inlay hints should resolve");
        let mut dump = query.title.to_string();

        if hints.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return dump;
        }

        writeln!(dump).expect("string writes should not fail");
        for hint in hints {
            self.render_inlay_hint(target.package, &hint, &mut dump);
        }
        dump
    }

    fn render_document_symbol_list(
        &self,
        package: PackageSlot,
        symbols: &[DocumentSymbol],
        depth: usize,
        dump: &mut String,
    ) {
        for symbol in symbols {
            self.render_document_symbol(package, symbol, depth, dump);
        }
    }

    fn render_document_symbol(
        &self,
        package: PackageSlot,
        symbol: &DocumentSymbol,
        depth: usize,
        dump: &mut String,
    ) {
        let indent = "  ".repeat(depth);
        let selection = if symbol.selection_span == symbol.span {
            String::new()
        } else {
            format!(
                " selection {}",
                self.render_source_span(package, symbol.file_id, symbol.selection_span)
            )
        };
        let label = if symbol.kind == crate::SymbolKind::Impl {
            symbol.name.clone()
        } else {
            format!("{} {}", symbol.kind, symbol.name)
        };

        writeln!(
            dump,
            "{indent}- {label} @ {}{}",
            self.render_source_span(package, symbol.file_id, symbol.span),
            selection
        )
        .expect("string writes should not fail");

        self.render_document_symbol_list(package, &symbol.children, depth + 1, dump);
    }

    fn render_workspace_symbol(&self, symbol: &WorkspaceSymbol, dump: &mut String) {
        let container = symbol
            .container_name
            .as_ref()
            .map(|container| format!(" in {container}"))
            .unwrap_or_default();

        writeln!(
            dump,
            "- {} {}{} @ {} {}",
            symbol.kind,
            symbol.name,
            container,
            self.render_target_ref(symbol.target),
            self.render_file_span(symbol.target.package, symbol.file_id, symbol.span)
        )
        .expect("string writes should not fail");
    }

    fn render_inlay_hint(&self, package: PackageSlot, hint: &InlayHint, dump: &mut String) {
        writeln!(
            dump,
            "- `{}` @ {}",
            hint.label,
            self.render_source_span(package, hint.file_id, hint.span)
        )
        .expect("string writes should not fail");
    }

    fn render_target_ref(&self, target_ref: TargetRef) -> String {
        let package = self
            .db
            .parse_db()
            .packages()
            .get(target_ref.package.0)
            .expect("target package should exist while rendering workspace symbol");
        let target = package
            .target(target_ref.target)
            .expect("target should exist while rendering workspace symbol");

        format!("{}[{}]", package.package_name(), target.kind)
    }

    fn render_file_span(
        &self,
        package: PackageSlot,
        file_id: FileId,
        span: Option<Span>,
    ) -> String {
        let file = self.render_file_path(package, file_id);
        match span {
            Some(span) => format!("{file}:{}", self.render_source_span(package, file_id, span)),
            None => format!("{file}:<root>"),
        }
    }

    fn render_file_path(&self, package: PackageSlot, file_id: FileId) -> String {
        let package = self
            .db
            .parse_db()
            .packages()
            .get(package.0)
            .expect("symbol package should exist while rendering file path");
        let path = package
            .file_path(file_id)
            .expect("symbol file should exist while rendering file path");
        let path = path.to_string_lossy();

        path.rfind("/src/")
            .map(|idx| path[idx + 1..].to_string())
            .unwrap_or_else(|| {
                path.rsplit('/')
                    .next()
                    .expect("path string should contain a file name")
                    .to_string()
            })
    }

    fn render_source_span(&self, package: PackageSlot, file_id: FileId, span: Span) -> String {
        let line_column = span.line_column(
            self.db
                .parse_db()
                .package(package.0)
                .expect("span package should exist while rendering analysis symbol")
                .parsed_file(file_id)
                .expect("span file should exist while rendering analysis symbol")
                .line_index()
                .expect("span file line index should load while rendering analysis symbol"),
        );
        format!(
            "{}:{}-{}:{}",
            line_column.start.line + 1,
            line_column.start.column + 1,
            line_column.end.line + 1,
            line_column.end.column + 1,
        )
    }
}
