//! Generic source-position facts over the indexed storage layers.
//!
//! This view owns the mechanical aggregation of source facts from DefMap, Semantic IR, and Body IR.
//! Query modules still decide how to interpret those facts for a cursor, completion request, or
//! reference search.

use rg_body_ir::{
    BodyCursorCandidate, PathCompletionNamespace as BodyPathCompletionNamespace,
    UnqualifiedCompletionNamespace as BodyUnqualifiedCompletionNamespace,
};
use rg_def_map::{DefMapCursorCandidate, Path};
use rg_ir_model::{
    BodyBindingRef, ModuleRef, TargetRef,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{FieldKey, SemanticCursorCandidate, TypePathContext};

use crate::api::view::{IndexedViewDb, declaration::DeclarationView};

/// Why an indexed source occurrence exists in the scanned source surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexedSourceRole {
    Declaration,
    Reference,
    Structural,
}

/// One indexed source span that can be interpreted by analysis queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedSourceOccurrence {
    target: TargetRef,
    file_id: FileId,
    span: Span,
    role: IndexedSourceRole,
    fact: IndexedSourceFact,
}

impl IndexedSourceOccurrence {
    pub(crate) fn into_parts(
        self,
    ) -> (
        IndexedSourceFact,
        TargetRef,
        FileId,
        Span,
        IndexedSourceRole,
    ) {
        (self.fact, self.target, self.file_id, self.span, self.role)
    }

    fn declaration(
        fact: IndexedSourceFact,
        target: TargetRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Declaration,
        }
    }

    fn reference(fact: IndexedSourceFact, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Reference,
        }
    }

    fn structural(fact: IndexedSourceFact, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Structural,
        }
    }
}

/// Indexed fact occupying one source occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexedSourceFact {
    Declaration(DeclarationRef),
    FunctionBody(FunctionBodyRef),
    Expr(ExprRef),
    TypePath {
        scope: IndexedTypePathScope,
        path: Path,
    },
    ValuePath {
        scope: LexicalScopeRef,
        path: Path,
    },
    UsePath {
        module: ModuleRef,
        path: Path,
    },
}

/// Resolution context for a type-looking path in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexedTypePathScope {
    Signature(TypePathContext),
    Body(LexicalScopeRef),
}

/// Namespace expected by an indexed name site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexedNameNamespace {
    Types,
    Values,
}

impl From<BodyPathCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: BodyPathCompletionNamespace) -> Self {
        match namespace {
            BodyPathCompletionNamespace::Types => Self::Types,
            BodyPathCompletionNamespace::Values => Self::Values,
        }
    }
}

impl From<BodyUnqualifiedCompletionNamespace> for IndexedNameNamespace {
    fn from(namespace: BodyUnqualifiedCompletionNamespace) -> Self {
        match namespace {
            BodyUnqualifiedCompletionNamespace::Types => Self::Types,
            BodyUnqualifiedCompletionNamespace::Values => Self::Values,
        }
    }
}

/// Source site for member access after a dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexedMemberAccessSite {
    receiver: ExprRef,
    member_prefix_span: Span,
}

impl IndexedMemberAccessSite {
    pub(crate) fn receiver(self) -> ExprRef {
        self.receiver
    }

    pub(crate) fn member_prefix_span(self) -> Span {
        self.member_prefix_span
    }
}

/// Source site for a qualified path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedQualifiedPathSite {
    scope: IndexedQualifiedPathScope,
    qualifier: Path,
    member_prefix_span: Span,
}

impl IndexedQualifiedPathSite {
    pub(crate) fn scope(&self) -> IndexedQualifiedPathScope {
        self.scope
    }

    pub(crate) fn qualifier(&self) -> &Path {
        &self.qualifier
    }

    pub(crate) fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }
}

/// Resolution context for a qualified path source site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexedQualifiedPathScope {
    Body {
        scope: LexicalScopeRef,
        namespace: IndexedNameNamespace,
    },
    Import {
        module: ModuleRef,
    },
}

/// Source site for an unqualified name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedUnqualifiedNameSite {
    scope: IndexedUnqualifiedNameScope,
    member_prefix_span: Span,
}

impl IndexedUnqualifiedNameSite {
    pub(crate) fn scope(&self) -> &IndexedUnqualifiedNameScope {
        &self.scope
    }

    pub(crate) fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }
}

/// Resolution context for an unqualified name source site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexedUnqualifiedNameScope {
    Body {
        scope: LexicalScopeRef,
        namespace: IndexedNameNamespace,
        member_prefix: String,
        visible_bindings: usize,
    },
    Import {
        module: ModuleRef,
    },
}

/// Source site for record literal or pattern field names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedRecordFieldListSite {
    scope: LexicalScopeRef,
    owner: Path,
    member_prefix_span: Span,
    existing_fields: Vec<FieldKey>,
}

impl IndexedRecordFieldListSite {
    pub(crate) fn scope(&self) -> LexicalScopeRef {
        self.scope
    }

    pub(crate) fn owner(&self) -> &Path {
        &self.owner
    }

    pub(crate) fn member_prefix_span(&self) -> Span {
        self.member_prefix_span
    }

    pub(crate) fn existing_fields(&self) -> &[FieldKey] {
        &self.existing_fields
    }
}

pub(crate) struct SourceFactsView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceFactsView<'a, 'db> {
    pub(crate) fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn occurrences_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();

        for candidate in self
            .analysis
            .body_ir
            .cursor_candidates(target, file_id, offset)?
        {
            if let Some(occurrence) = self.body_occurrence(target, candidate, Some(file_id))? {
                occurrences.push(occurrence);
            }
        }
        for candidate in self
            .analysis
            .def_map
            .cursor_candidates(target, file_id, offset)?
        {
            occurrences.push(Self::def_map_occurrence(target, candidate));
        }
        for candidate in self
            .analysis
            .semantic_ir
            .signature_cursor_candidates(target, file_id, offset)?
        {
            if let Some(occurrence) = self.semantic_occurrence(target, candidate, Some(file_id))? {
                occurrences.push(occurrence);
            }
        }

        Ok(occurrences)
    }

    pub(crate) fn occurrences_in_target(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();

        for candidate in self.analysis.def_map.source_candidates(target, file_id)? {
            occurrences.push(Self::def_map_occurrence(target, candidate));
        }
        for candidate in self.analysis.body_ir.source_candidates(target, file_id)? {
            if let Some(occurrence) = self.body_occurrence(target, candidate, file_id)? {
                occurrences.push(occurrence);
            }
        }
        for candidate in self
            .analysis
            .semantic_ir
            .signature_source_candidates(target, file_id)?
        {
            if let Some(occurrence) = self.semantic_occurrence(target, candidate, file_id)? {
                occurrences.push(occurrence);
            }
        }

        Ok(occurrences)
    }

    pub(crate) fn member_access_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedMemberAccessSite>> {
        Ok(self
            .analysis
            .body_ir
            .dot_completion_site(target, file_id, offset)?
            .map(|site| IndexedMemberAccessSite {
                receiver: ExprRef::new(site.body, site.receiver),
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub(crate) fn body_qualified_path_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(self
            .analysis
            .body_ir
            .path_completion_site(target, file_id, offset)?
            .map(|site| IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Body {
                    scope: LexicalScopeRef::new(site.body, site.scope),
                    namespace: site.namespace.into(),
                },
                qualifier: site.qualifier,
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub(crate) fn import_qualified_path_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(self
            .analysis
            .def_map
            .path_completion_site(target, file_id, offset)?
            .map(|site| IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Import {
                    module: site.module,
                },
                qualifier: site.qualifier,
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub(crate) fn body_unqualified_name_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(self
            .analysis
            .body_ir
            .unqualified_completion_site(target, file_id, offset)?
            .map(|site| IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Body {
                    scope: LexicalScopeRef::new(site.body, site.scope),
                    namespace: site.namespace.into(),
                    member_prefix: site.member_prefix,
                    visible_bindings: site.visible_bindings,
                },
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub(crate) fn import_unqualified_name_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(self
            .analysis
            .def_map
            .unqualified_completion_site(target, file_id, offset)?
            .map(|site| IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Import {
                    module: site.module,
                },
                member_prefix_span: site.member_prefix_span,
            }))
    }

    pub(crate) fn record_field_list_site_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedRecordFieldListSite>> {
        Ok(self
            .analysis
            .body_ir
            .record_field_completion_site(target, file_id, offset)?
            .map(|site| IndexedRecordFieldListSite {
                scope: LexicalScopeRef::new(site.body, site.scope),
                owner: site.owner,
                member_prefix_span: site.member_prefix_span,
                existing_fields: site.existing_fields,
            }))
    }

    fn def_map_occurrence(
        target: TargetRef,
        candidate: DefMapCursorCandidate,
    ) -> IndexedSourceOccurrence {
        match candidate {
            DefMapCursorCandidate::Def { def, file_id, span } => {
                IndexedSourceOccurrence::declaration(
                    IndexedSourceFact::Declaration(DeclarationRef::from_def(def)),
                    target,
                    file_id,
                    span,
                )
            }
            DefMapCursorCandidate::UsePath {
                module,
                path,
                file_id,
                span,
            } => IndexedSourceOccurrence::reference(
                IndexedSourceFact::UsePath { module, path },
                target,
                file_id,
                span,
            ),
        }
    }

    fn semantic_occurrence(
        &self,
        target: TargetRef,
        candidate: SemanticCursorCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let occurrence = match candidate {
            SemanticCursorCandidate::Field { field, span } => {
                let declaration = DeclarationRef::semantic(field.into());
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            SemanticCursorCandidate::Function { function, span } => {
                let declaration = DeclarationRef::semantic(function.into());
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            SemanticCursorCandidate::EnumVariant { variant, span } => {
                let declaration = DeclarationRef::semantic(variant.into());
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            SemanticCursorCandidate::TypePath {
                context,
                path,
                file_id,
                span,
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::TypePath {
                    scope: IndexedTypePathScope::Signature(context),
                    path,
                },
                target,
                file_id,
                span,
            )),
        };

        Ok(occurrence)
    }

    fn body_occurrence(
        &self,
        target: TargetRef,
        candidate: BodyCursorCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let span = candidate.span();
        let occurrence = match candidate {
            BodyCursorCandidate::Body { body, .. } => {
                let file_id = match self.analysis.body_ir.body_data(body)? {
                    Some(data) => data.source().file_id,
                    None => {
                        let Some(file_id) = fallback_file_id else {
                            return Ok(None);
                        };
                        file_id
                    }
                };
                Some(IndexedSourceOccurrence::structural(
                    IndexedSourceFact::FunctionBody(FunctionBodyRef::from_body_ir(body)),
                    target,
                    file_id,
                    span,
                ))
            }
            BodyCursorCandidate::Binding { body, binding, .. } => {
                let declaration = DeclarationRef::body_binding(BodyBindingRef { body, binding });
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::Expr { body, expr, .. } => {
                let file_id = match self.analysis.body_ir.body_data(body)? {
                    Some(body_data) => match body_data.expr(expr) {
                        Some(data) => data.source.file_id,
                        None => {
                            let Some(file_id) = fallback_file_id else {
                                return Ok(None);
                            };
                            file_id
                        }
                    },
                    None => {
                        let Some(file_id) = fallback_file_id else {
                            return Ok(None);
                        };
                        file_id
                    }
                };
                Some(IndexedSourceOccurrence::reference(
                    IndexedSourceFact::Expr(ExprRef::new(body, expr)),
                    target,
                    file_id,
                    span,
                ))
            }
            BodyCursorCandidate::LocalItem { item, .. } => {
                let declaration = DeclarationRef::body_item(item);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalValueItem { item, .. } => {
                let declaration = DeclarationRef::body_value_item(item);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalField { field, .. } => {
                let declaration = DeclarationRef::body_field(field);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalEnumVariant { variant, .. } => {
                let declaration = DeclarationRef::body_enum_variant(variant);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalFunction { function, .. } => {
                let declaration = DeclarationRef::body_function(function);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::TypePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::TypePath {
                    scope: IndexedTypePathScope::Body(LexicalScopeRef::new(body, scope)),
                    path,
                },
                target,
                file_id,
                span,
            )),
            BodyCursorCandidate::ValuePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::ValuePath {
                    scope: LexicalScopeRef::new(body, scope),
                    path,
                },
                target,
                file_id,
                span,
            )),
        };

        Ok(occurrence)
    }

    fn declaration_occurrence(
        &self,
        declaration: DeclarationRef,
        scan_target: TargetRef,
        span: Span,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        // Some scanner families know only the selected span. Use the declaration projection for
        // canonical file ownership, and fall back to the cursor file for point lookups.
        let file_id = match DeclarationView::new(self.analysis).declaration(declaration)? {
            Some(declaration) => declaration.file_id(),
            None => {
                let Some(file_id) = fallback_file_id else {
                    return Ok(None);
                };
                file_id
            }
        };

        Ok(Some(IndexedSourceOccurrence::declaration(
            IndexedSourceFact::Declaration(declaration),
            scan_target,
            file_id,
            span,
        )))
    }
}
