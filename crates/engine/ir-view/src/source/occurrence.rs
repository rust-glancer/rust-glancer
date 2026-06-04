//! Source occurrences for symbol and reference queries.
//!
//! This adapter merges cursor/source candidates from DefMap, Semantic IR, and Body IR into one
//! occurrence vocabulary. Analysis decides what each occurrence means for navigation or refs.

use rg_body_ir::{
    BindingSurface, BodyCursorCandidate, RecordFieldKeySurface, ValueReferenceSource,
    ValueReferenceSurface,
};
use rg_def_map::DefMapCursorCandidate;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    BodyBindingRef, ModuleRef, TargetRef,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_ir_storage::{Path, TypePathContext};
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticCursorCandidate;

use crate::{IndexedViewDb, item::declaration::DeclarationView};

/// Why an indexed source occurrence exists in the scanned source surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedSourceRole {
    Declaration,
    Reference,
    Structural,
}

/// Source syntax shape that may need query-specific handling after semantic resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSourceSurface {
    /// Ordinary occurrence that can be rewritten by replacing the selected source span.
    ///
    /// For `let title = name;`, renaming the `name` reference edits only that token.
    Plain,
    /// Explicit record field key, e.g. `name` in `User { name: value }`.
    ///
    /// The key is a field reference, but the source spelling already has separate key and value
    /// syntax, so field rename still edits only the key token.
    RecordFieldKeyExplicit,
    /// Field-key side of record-expression shorthand, e.g. the field `name` in `User { name }`.
    ///
    /// Renaming the field has to expand the field to `title: name`, while renaming the local value
    /// is handled by the paired `RecordExprShorthandValue` occurrence.
    RecordExprShorthandFieldKey { field_span: Span },
    /// Field-key side of record-pattern shorthand, e.g. the field `name` in `User { ref name }`.
    ///
    /// Renaming the field rewrites the whole field to `title: ref name` so pattern modifiers and
    /// subpatterns stay intact.
    RecordPatShorthandFieldKey { field_span: Span, pat_span: Span },
    /// Value-reference side of record-expression shorthand, e.g. the local `name` in `User { name }`.
    ///
    /// Renaming the local value rewrites the field to `name: title` instead of changing the field
    /// key.
    RecordExprShorthandValue { key: FieldKey, field_span: Span },
    /// Binding-declaration side of record-pattern shorthand, e.g. the binding in `User { ref name }`.
    ///
    /// Renaming the binding rewrites the whole field to `name: ref title`, preserving the field key
    /// and any pattern syntax around the binding name.
    RecordPatShorthandBinding {
        key: FieldKey,
        field_span: Span,
        pat_span: Span,
        binding_name_span: Span,
    },
}

/// One indexed source span that can be interpreted by analysis queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSourceOccurrence {
    target: TargetRef,
    file_id: FileId,
    span: Span,
    role: IndexedSourceRole,
    surface: IndexedSourceSurface,
    fact: IndexedSourceFact,
}

impl IndexedSourceOccurrence {
    pub fn into_parts(
        self,
    ) -> (
        IndexedSourceFact,
        TargetRef,
        FileId,
        Span,
        IndexedSourceRole,
        IndexedSourceSurface,
    ) {
        (
            self.fact,
            self.target,
            self.file_id,
            self.span,
            self.role,
            self.surface,
        )
    }

    fn declaration(
        fact: IndexedSourceFact,
        target: TargetRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self::declaration_with_surface(fact, target, file_id, span, IndexedSourceSurface::Plain)
    }

    fn declaration_with_surface(
        fact: IndexedSourceFact,
        target: TargetRef,
        file_id: FileId,
        span: Span,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Declaration,
            surface,
        }
    }

    fn reference(fact: IndexedSourceFact, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self::reference_with_surface(fact, target, file_id, span, IndexedSourceSurface::Plain)
    }

    fn reference_with_surface(
        fact: IndexedSourceFact,
        target: TargetRef,
        file_id: FileId,
        span: Span,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Reference,
            surface,
        }
    }

    fn structural(fact: IndexedSourceFact, target: TargetRef, file_id: FileId, span: Span) -> Self {
        Self {
            fact,
            target,
            file_id,
            span,
            role: IndexedSourceRole::Structural,
            surface: IndexedSourceSurface::Plain,
        }
    }
}

/// Indexed fact occupying one source occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSourceFact {
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
    RecordField {
        scope: LexicalScopeRef,
        owner: Path,
        key: FieldKey,
    },
    UsePath {
        module: ModuleRef,
        path: Path,
    },
}

/// Resolution context for a type-looking path in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedTypePathScope {
    Signature(TypePathContext),
    Body(LexicalScopeRef),
}

pub struct SourceOccurrenceView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceOccurrenceView<'a, 'db> {
    pub fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub fn occurrences_at(
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

    pub fn occurrences_in_target(
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
            DefMapCursorCandidate::ImportAlias {
                module,
                path,
                file_id,
                span,
            } => IndexedSourceOccurrence::structural(
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
                let declaration = DeclarationRef::from(field);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            SemanticCursorCandidate::Function { function, span } => {
                let declaration = DeclarationRef::from(function);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            SemanticCursorCandidate::EnumVariant { variant, span } => {
                let declaration = DeclarationRef::from(variant);
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
                let Some(data) = self.analysis.body_ir.body_data(body)? else {
                    return Ok(None);
                };
                let Some(_) = data.function_owner() else {
                    return Ok(None);
                };
                Some(IndexedSourceOccurrence::structural(
                    IndexedSourceFact::FunctionBody(FunctionBodyRef::from_body_ir(body)),
                    target,
                    data.source().file_id,
                    span,
                ))
            }
            BodyCursorCandidate::Binding {
                body,
                binding,
                surface,
                ..
            } => {
                let declaration = DeclarationRef::body_binding(BodyBindingRef { body, binding });
                match surface {
                    BindingSurface::Plain => {
                        self.declaration_occurrence(declaration, target, span, fallback_file_id)?
                    }
                    BindingSurface::RecordPatShorthand {
                        key,
                        field_span,
                        pat_span,
                        binding_name_span,
                    } => {
                        let file_id = match self.analysis.body_ir.body_data(body)? {
                            Some(body_data) => match body_data.binding(binding) {
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
                        Some(IndexedSourceOccurrence::declaration_with_surface(
                            IndexedSourceFact::Declaration(declaration),
                            target,
                            file_id,
                            span,
                            IndexedSourceSurface::RecordPatShorthandBinding {
                                key,
                                field_span,
                                pat_span,
                                binding_name_span,
                            },
                        ))
                    }
                }
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
                let declaration = DeclarationRef::from(item);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalValueItem { item, .. } => {
                let declaration = DeclarationRef::from(item);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalField { field, .. } => {
                let declaration = DeclarationRef::from(field);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalEnumVariant { variant, .. } => {
                let declaration = DeclarationRef::from(variant);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::LocalFunction { function, .. } => {
                let declaration = DeclarationRef::from(function);
                self.declaration_occurrence(declaration, target, span, fallback_file_id)?
            }
            BodyCursorCandidate::RecordFieldKey {
                body,
                scope,
                owner,
                key,
                file_id,
                surface,
                ..
            } => {
                let surface = match surface {
                    RecordFieldKeySurface::Explicit => IndexedSourceSurface::RecordFieldKeyExplicit,
                    RecordFieldKeySurface::RecordExprShorthand { field_span } => {
                        IndexedSourceSurface::RecordExprShorthandFieldKey { field_span }
                    }
                    RecordFieldKeySurface::RecordPatShorthand {
                        field_span,
                        pat_span,
                    } => IndexedSourceSurface::RecordPatShorthandFieldKey {
                        field_span,
                        pat_span,
                    },
                };
                Some(IndexedSourceOccurrence::reference_with_surface(
                    IndexedSourceFact::RecordField {
                        scope: LexicalScopeRef::new(body, scope),
                        owner,
                        key,
                    },
                    target,
                    file_id,
                    span,
                    surface,
                ))
            }
            BodyCursorCandidate::ValueReference {
                body,
                scope,
                source,
                file_id,
                surface,
                ..
            } => {
                let fact = match source {
                    ValueReferenceSource::Expr(expr) => {
                        IndexedSourceFact::Expr(ExprRef::new(body, expr))
                    }
                    ValueReferenceSource::Path(path) => IndexedSourceFact::ValuePath {
                        scope: LexicalScopeRef::new(body, scope),
                        path,
                    },
                };
                let surface = match surface {
                    ValueReferenceSurface::Plain => IndexedSourceSurface::Plain,
                    ValueReferenceSurface::RecordExprShorthand { key, field_span } => {
                        IndexedSourceSurface::RecordExprShorthandValue { key, field_span }
                    }
                };
                Some(IndexedSourceOccurrence::reference_with_surface(
                    fact, target, file_id, span, surface,
                ))
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
