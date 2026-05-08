use std::{fmt, fmt::Write as _, marker::PhantomData, sync::Arc};

use expect_test::Expect;

use crate::{
    Analysis, AnalysisReadTxn, CompletionApplicability, CompletionItem, DocumentSymbol, HoverInfo,
    NavigationTarget, SymbolAt, TypeHint, WorkspaceSymbol,
};
use rg_body_ir::{
    BodyGenericArg, BodyIrDb, BodyIrReadTxn, BodyItemRef, BodyLocalNominalTy, BodyNominalTy,
    BodyTy, ExprData, ExprKind,
};
use rg_def_map::{DefMapDb, ModuleRef, PackageSlot, TargetRef};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::{FileId, ParseDb, Span};
use rg_semantic_ir::{
    FunctionRef, ItemOwner, SemanticIrDb, SemanticIrReadTxn, TraitRef, TypeDefId, TypeDefRef,
};
use rg_workspace::{SysrootSources, TargetKind, WorkspaceMetadata};
use test_fixture::{FixtureMarkers, fixture_crate, fixture_crate_with_markers};

pub(super) fn check_analysis_queries(fixture: &str, queries: &[AnalysisQuery], expect: Expect) {
    let (fixture, markers) = fixture_crate_with_markers(fixture);
    let db = AnalysisFixtureDb::build(
        WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build"),
    );
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
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build")
        .with_sysroot_sources(Some(sysroot));
    let db = AnalysisFixtureDb::build(workspace);
    let renderer = AnalysisQuerySnapshot::new(&db, markers, queries);
    let actual = format!("{}\n", renderer.render().trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_document_symbols(fixture: &str, query: DocumentSymbolsQuery, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build(
        WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build"),
    );
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_document_symbols(&query).trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_workspace_symbols(fixture: &str, query: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build(
        WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build"),
    );
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_workspace_symbols(query).trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_type_hints(fixture: &str, query: TypeHintsQuery, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let db = AnalysisFixtureDb::build(
        WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build"),
    );
    let renderer = AnalysisSymbolSnapshot::new(&db);
    let actual = format!("{}\n", renderer.render_type_hints(&query).trim_end());
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

    pub(super) fn ty(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::TypeAt)
    }

    pub(super) fn complete(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::CompletionsAtDot)
    }

    pub(super) fn hover(title: &'static str, marker: &'static str) -> Self {
        Self::new(title, marker, AnalysisQueryKind::Hover)
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

pub(super) struct TypeHintsQuery {
    title: &'static str,
    path: &'static str,
    target: AnalysisTarget,
}

impl TypeHintsQuery {
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
    TypeAt,
    CompletionsAtDot,
    Hover,
}

struct AnalysisFixtureDb {
    parse: ParseDb,
    def_map: DefMapDb,
    semantic_ir: SemanticIrDb,
    body_ir: BodyIrDb,
}

impl AnalysisFixtureDb {
    fn build(workspace: WorkspaceMetadata) -> Self {
        let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
        let item_tree = ItemTreeDb::build(&mut parse).expect("fixture item tree db should build");
        let def_map = DefMapDb::builder(&workspace, &parse, &item_tree)
            .build()
            .expect("fixture def map db should build");
        let semantic_ir = SemanticIrDb::builder(&item_tree, &def_map)
            .build()
            .expect("fixture semantic ir db should build");
        let body_ir = BodyIrDb::builder(&parse, &def_map, &semantic_ir)
            .build()
            .expect("fixture body ir db should build");

        Self {
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }

    fn analysis(&self) -> Analysis<'_> {
        let txn = AnalysisReadTxn::from_phase_txns(
            self.def_map.read_txn(unexpected_package_loader()),
            self.semantic_ir.read_txn(unexpected_package_loader()),
            self.body_ir.read_txn(unexpected_package_loader()),
        );
        Analysis::new(&txn)
    }

    fn resident_def_map(&self, target: TargetRef) -> Option<&rg_def_map::DefMap> {
        self.def_map
            .resident_package(target.package)?
            .target(target.target)
    }

    fn resident_target_ir(&self, target: TargetRef) -> Option<&rg_semantic_ir::TargetIr> {
        self.semantic_ir
            .resident_package(target.package)?
            .target(target.target)
    }

    fn target_and_file_for_path(
        &self,
        selected: &AnalysisTarget,
        path: &str,
    ) -> (TargetRef, FileId) {
        let mut matches = Vec::new();
        let normalized_path = path.trim_start_matches('/');

        for (package_slot, package) in self.parse.packages().iter().enumerate() {
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

    fn target_owns_file(&self, target: TargetRef, file_id: FileId) -> bool {
        let def_map = self
            .resident_def_map(target)
            .expect("selected fixture target should have a def map");

        def_map
            .modules()
            .iter()
            .any(|module| module.origin.contains_file(file_id))
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
            "resident analysis fixture should not load offloaded package {}",
            package.0,
        )
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
            AnalysisQueryKind::CompletionsAtDot => {
                self.render_completions(
                    self.db
                        .analysis()
                        .completions_at_dot(target, file_id, offset)
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
            SymbolAt::Body { body } => {
                let body_ir = self.body_ir_txn();
                let body_data = body_ir
                    .body_data(body)
                    .expect("body ref should load while rendering analysis symbol")
                    .expect("body ref should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "\n- body @ {}",
                    self.render_source_span(
                        body.target.package,
                        body_data.source().file_id,
                        body_data.source().span
                    )
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Binding { body, binding } => {
                let body_ir = self.body_ir_txn();
                let body_data = body_ir
                    .body_data(body)
                    .expect("body ref should load while rendering analysis symbol")
                    .expect("body ref should exist while rendering analysis symbol");
                let binding_data = body_data
                    .binding(binding)
                    .expect("binding id should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "\n- binding {} {} @ {}",
                    binding_data.kind,
                    binding_data.name.as_deref().unwrap_or("<unsupported>"),
                    self.render_source_span(
                        body.target.package,
                        binding_data.source.file_id,
                        binding_data.source.span,
                    )
                )
                .expect("string writes should not fail");
            }
            SymbolAt::BodyPath {
                body,
                ref path,
                span,
                ..
            } => {
                writeln!(
                    dump,
                    "\n- body path {path} @ {}",
                    self.render_source_span(body.target.package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::BodyValuePath {
                body,
                ref path,
                span,
                ..
            } => {
                writeln!(
                    dump,
                    "\n- body value path {path} @ {}",
                    self.render_source_span(body.target.package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Def { def, span } => {
                let targets = self
                    .db
                    .analysis()
                    .resolve_symbol(SymbolAt::Def { def, span })
                    .expect("fixture symbol resolution should resolve");
                let label = targets
                    .first()
                    .map(|target| format!("{} {}", target.kind, target.name))
                    .unwrap_or_else(|| "def <unresolved>".to_string());
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Expr { body, expr } => {
                let body_ir = self.body_ir_txn();
                let body_data = body_ir
                    .body_data(body)
                    .expect("body ref should load while rendering analysis symbol")
                    .expect("body ref should exist while rendering analysis symbol");
                let expr_data = body_data
                    .expr(expr)
                    .expect("expr id should exist while rendering analysis symbol");
                writeln!(
                    dump,
                    "\n- {}",
                    self.render_expr_symbol(body.target.package, expr_data)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Field { field, span } => {
                let targets = self
                    .db
                    .analysis()
                    .resolve_symbol(SymbolAt::Field { field, span })
                    .expect("fixture symbol resolution should resolve");
                let label = targets
                    .first()
                    .map(|target| format!("{} {}", target.kind, target.name))
                    .unwrap_or_else(|| "field <unresolved>".to_string());
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::Function { function, span } => {
                let targets = self
                    .db
                    .analysis()
                    .resolve_symbol(SymbolAt::Function { function, span })
                    .expect("fixture symbol resolution should resolve");
                let label = targets
                    .first()
                    .map(|target| format!("{} {}", target.kind, target.name))
                    .unwrap_or_else(|| "fn <unresolved>".to_string());
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::EnumVariant { variant, span } => {
                let targets = self
                    .db
                    .analysis()
                    .resolve_symbol(SymbolAt::EnumVariant { variant, span })
                    .expect("fixture symbol resolution should resolve");
                let label = targets
                    .first()
                    .map(|target| format!("{} {}", target.kind, target.name))
                    .unwrap_or_else(|| "variant <unresolved>".to_string());
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::LocalItem { item, span } => {
                let label = self.render_body_item_ref(item);
                writeln!(
                    dump,
                    "\n- {label} @ {}",
                    self.render_source_span(item.body.target.package, file_id, span)
                )
                .expect("string writes should not fail");
            }
            SymbolAt::TypePath { ref path, span, .. }
            | SymbolAt::UsePath { ref path, span, .. } => {
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
            ExprKind::Match { .. } => "match".to_string(),
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
            ExprKind::Wrapper { kind, .. } => format!("wrapper {kind}"),
            ExprKind::Literal { kind } => {
                format!(
                    "literal {kind} {}",
                    self.render_source_text(package, expr.source)
                )
            }
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
            writeln!(
                dump,
                "- {} {} @ {}",
                target.kind,
                target.name,
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

    fn render_ty(&self, ty: &BodyTy) -> String {
        match ty {
            BodyTy::Unit => "()".to_string(),
            BodyTy::Never => "!".to_string(),
            BodyTy::Syntax(ty) => format!("syntax {ty}"),
            BodyTy::Reference(inner) => format!("&{}", self.render_ty(inner)),
            BodyTy::LocalNominal(items) => {
                let mut items = items
                    .iter()
                    .map(|ty| self.render_body_local_nominal_ty(ty))
                    .collect::<Vec<_>>();
                items.sort();
                format!("local nominal {}", items.join(" | "))
            }
            BodyTy::Nominal(types) => {
                let mut types = types
                    .iter()
                    .map(|ty| self.render_body_nominal_ty(ty))
                    .collect::<Vec<_>>();
                types.sort();
                format!("nominal {}", types.join(" | "))
            }
            BodyTy::SelfTy(types) => {
                let mut types = types
                    .iter()
                    .map(|ty| self.render_body_nominal_ty(ty))
                    .collect::<Vec<_>>();
                types.sort();
                format!("Self {}", types.join(" | "))
            }
            BodyTy::Unknown => "<unknown>".to_string(),
        }
    }

    fn render_body_local_nominal_ty(&self, ty: &BodyLocalNominalTy) -> String {
        format!(
            "{}{}",
            self.render_body_item_ref(ty.item),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_body_nominal_ty(&self, ty: &BodyNominalTy) -> String {
        format!(
            "{}{}",
            self.render_type_def_ref(ty.def),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_generic_args(&self, args: &[BodyGenericArg]) -> String {
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

    fn render_generic_arg(&self, arg: &BodyGenericArg) -> String {
        match arg {
            BodyGenericArg::Type(ty) => self.render_ty(ty),
            BodyGenericArg::Lifetime(lifetime) => lifetime.clone(),
            BodyGenericArg::Const(value) => value.clone(),
            BodyGenericArg::AssocType { name, ty } => match ty {
                Some(ty) => format!("{name} = {}", self.render_ty(ty)),
                None => name.to_string(),
            },
            BodyGenericArg::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }

    fn render_body_item_ref(&self, item_ref: BodyItemRef) -> String {
        let body_ir = self.body_ir_txn();
        let body = body_ir
            .body_data(item_ref.body)
            .expect("body item body should load while rendering analysis type")
            .expect("body item body should exist while rendering analysis type");
        let item = body
            .local_item(item_ref.item)
            .expect("body item id should exist while rendering analysis type");

        format!(
            "{} {}::{}",
            item.kind,
            self.render_function_ref(body.owner()),
            item.name
        )
    }

    fn render_type_def_ref(&self, ty: TypeDefRef) -> String {
        let target_ir = self
            .db
            .resident_target_ir(ty.target)
            .expect("target semantic IR should exist while rendering analysis type");

        match ty.id {
            TypeDefId::Struct(id) => {
                let data = target_ir
                    .items()
                    .struct_data(id)
                    .expect("struct id should exist while rendering analysis type");
                format!(
                    "struct {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            TypeDefId::Enum(id) => {
                let data = target_ir
                    .items()
                    .enum_data(id)
                    .expect("enum id should exist while rendering analysis type");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            TypeDefId::Union(id) => {
                let data = target_ir
                    .items()
                    .union_data(id)
                    .expect("union id should exist while rendering analysis type");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_function_ref(&self, function_ref: FunctionRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .function_data(function_ref)
            .expect("function ref should load while rendering analysis body item")
            .expect("function ref should exist while rendering analysis body item");
        let owner = match data.owner {
            ItemOwner::Module(module_ref) => self.render_module_ref(module_ref),
            ItemOwner::Trait(trait_id) => {
                let trait_data = semantic_ir
                    .trait_data(TraitRef {
                        target: function_ref.target,
                        id: trait_id,
                    })
                    .expect("trait owner should load while rendering analysis body item")
                    .expect("trait owner should exist while rendering analysis body item");
                format!(
                    "trait {}::{}",
                    self.render_module_ref(trait_data.owner),
                    trait_data.name
                )
            }
            ItemOwner::Impl(_) => "impl".to_string(),
        };

        format!("fn {owner}::{}", data.name)
    }

    fn semantic_ir_txn(&self) -> SemanticIrReadTxn<'_> {
        self.db.semantic_ir.read_txn(unexpected_package_loader())
    }

    fn body_ir_txn(&self) -> BodyIrReadTxn<'_> {
        self.db.body_ir.read_txn(unexpected_package_loader())
    }

    fn render_module_ref(&self, module_ref: ModuleRef) -> String {
        let package = self
            .db
            .parse
            .packages()
            .get(module_ref.target.package.0)
            .expect("package slot should exist while rendering analysis module");
        let target = package
            .target(module_ref.target.target)
            .expect("target id should exist while rendering analysis module");

        format!(
            "{}[{}]::{}",
            package.package_name(),
            target.kind,
            self.module_path(module_ref),
        )
    }

    fn module_path(&self, module_ref: ModuleRef) -> String {
        let module = self
            .db
            .resident_def_map(module_ref.target)
            .expect("target def map should exist while rendering analysis module path")
            .module(module_ref.module)
            .expect("module id should exist while rendering analysis module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(ModuleRef {
                    target: module_ref.target,
                    module: parent,
                });
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
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
                .parse
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

    fn render_source_text(&self, package: PackageSlot, source: rg_body_ir::BodySource) -> String {
        let parsed_file = self
            .db
            .parse
            .package(package.0)
            .expect("span package should exist while rendering analysis query text")
            .parsed_file(source.file_id)
            .expect("span file should exist while rendering analysis query text");

        parsed_file
            .text_for_span(source.span)
            .unwrap_or_else(|| "<invalid>".to_string())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
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

    fn render_type_hints(&self, query: &TypeHintsQuery) -> String {
        let (target, file_id) = self.db.target_and_file_for_path(&query.target, query.path);
        let hints = self
            .db
            .analysis()
            .type_hints(target, file_id, None)
            .expect("fixture type hints should resolve");
        let mut dump = query.title.to_string();

        if hints.is_empty() {
            writeln!(dump, "\n- <none>").expect("string writes should not fail");
            return dump;
        }

        writeln!(dump).expect("string writes should not fail");
        for hint in hints {
            self.render_type_hint(target.package, &hint, &mut dump);
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

    fn render_type_hint(&self, package: PackageSlot, hint: &TypeHint, dump: &mut String) {
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
            .parse
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
            .parse
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
                .parse
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
