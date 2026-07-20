//! Source occurrences for symbol and reference queries.
//!
//! The scanners below read source facts from DefMap, Semantic IR, and Body IR, then normalize them
//! into one facade-owned occurrence vocabulary. Analysis decides what each occurrence means for
//! navigation or references.
//!
//! A single source expression can expose more than one useful fact. In `Action::Start`, for
//! example, `Action` is a type path while `Start` is a value path. The facade keeps those semantic
//! inputs distinct instead of forcing scanners to choose a navigation result.

use rg_ir_model::{
    BodyBindingRef, CrateRef, FieldKey, ModuleRef, Path,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::TypePathContext;

use super::scan::{
    BindingSurface, BodyCursorScanner, BodySourceCandidate, BodySourceScanner,
    DefinitionSourceCandidate, DefinitionSourceScanner, RecordFieldKeySurface,
    SignatureSourceCandidate, SignatureSourceScanner, ValueReferenceSource, ValueReferenceSurface,
};
use crate::{IndexedViewDb, item::declaration::DeclarationView};

/// Why an indexed source occurrence exists in the scanned source surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedSourceRole {
    /// A spelling that introduces a symbol, such as `user` in `let user = input;`.
    Declaration,
    /// A spelling that refers to a symbol, such as `user` in `consume(user)`.
    Reference,
    /// Source ownership needed by an editor query, but not itself a symbol use.
    ///
    /// Function-body roots and import aliases use this role. Analysis can inspect them without
    /// counting them as declarations or references.
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
    crate_ref: CrateRef,
    file_id: FileId,
    span: Span,
    role: IndexedSourceRole,
    surface: IndexedSourceSurface,
    fact: IndexedSourceFact,
}

impl IndexedSourceOccurrence {
    /// Split the occurrence into transport-neutral parts.
    pub fn into_parts(
        self,
    ) -> (
        IndexedSourceFact,
        CrateRef,
        FileId,
        Span,
        IndexedSourceRole,
        IndexedSourceSurface,
    ) {
        (
            self.fact,
            self.crate_ref,
            self.file_id,
            self.span,
            self.role,
            self.surface,
        )
    }

    /// Build a plain declaration occurrence.
    fn declaration(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self::declaration_with_surface(fact, crate_ref, file_id, span, IndexedSourceSurface::Plain)
    }

    /// Build a declaration occurrence with special source-surface handling.
    fn declaration_with_surface(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        span: Span,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span,
            role: IndexedSourceRole::Declaration,
            surface,
        }
    }

    /// Build a plain reference occurrence.
    fn reference(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self::reference_with_surface(fact, crate_ref, file_id, span, IndexedSourceSurface::Plain)
    }

    /// Build a reference occurrence with special source-surface handling.
    fn reference_with_surface(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        span: Span,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span,
            role: IndexedSourceRole::Reference,
            surface,
        }
    }

    /// Build an occurrence that is neither a declaration nor a reference.
    fn structural(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        span: Span,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span,
            role: IndexedSourceRole::Structural,
            surface: IndexedSourceSurface::Plain,
        }
    }
}

/// Indexed fact occupying one source occurrence.
///
/// Facts retain stable engine identities or the minimum path context needed for later resolution.
/// They deliberately do not contain an eagerly resolved destination: different analysis queries
/// can apply their own visibility and fallback policy to the same source occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSourceFact {
    /// A declaration with an engine-wide stable identity.
    Declaration(DeclarationRef),
    /// The source range owned by one function body.
    FunctionBody(FunctionBodyRef),
    /// A lowered expression whose exact meaning is available from Body IR.
    Expr(ExprRef),
    /// A path written in a type position, together with its signature or body scope.
    TypePath {
        scope: IndexedTypePathScope,
        path: Path,
    },
    /// A value path without a dedicated lowered expression, most commonly a pattern path segment.
    ValuePath { scope: LexicalScopeRef, path: Path },
    /// A field key in a record expression or pattern.
    RecordField {
        scope: LexicalScopeRef,
        owner: Path,
        key: FieldKey,
    },
    /// One written prefix of an import path, resolved from the importing module.
    UsePath { module: ModuleRef, path: Path },
}

/// Resolution context for a type-looking path in source.
///
/// Signature paths need item generics and impl ownership; body paths need a lexical scope. Keeping
/// the two cases explicit prevents callers from pretending that every type spelling resolves from
/// a module alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedTypePathScope {
    Signature(TypePathContext),
    Body(LexicalScopeRef),
}

/// Finds declaration, reference, and structural source occurrences.
///
/// Point queries select the narrowest body-local source node at `$0` and retain any independently
/// meaningful path segment at that offset. Crate queries instead return every written occurrence:
///
/// ```text
/// let user = input;
/// consume(user$0); point query selects this `user`
///
/// // crate query retains the declaration, `input`, `consume`, and the second `user`
/// ```
pub struct SourceOccurrenceView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> SourceOccurrenceView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Returns the named source spelling selected by an expression, when the expression has one.
    ///
    /// Expression-backed occurrences intentionally expose only `ExprRef`, so callers that need a
    /// cheap source label can ask the view to inspect the lowered expression. For example, this
    /// returns `user` for a path expression, `push` for `items.push(value)`, and `name` for
    /// `user.name`; literals and operators return `None`.
    pub fn expr_source_label(&self, expr: ExprRef) -> anyhow::Result<Option<String>> {
        let Some(body_data) = self.db.body_ir.body(expr.body_ir())? else {
            return Ok(None);
        };
        let Some(expr_data) = body_data.expr(expr.expr_id()) else {
            return Ok(None);
        };

        let label = match &expr_data.kind {
            rg_body_ir::ExprKind::Path { path } => path
                .as_def_map_path()
                .and_then(|path| path.last_segment_label()),
            rg_body_ir::ExprKind::MethodCall { method_name, .. } => Some(method_name.to_string()),
            rg_body_ir::ExprKind::Field { field, .. } => {
                field.as_ref().map(|field| field.declaration_label())
            }
            _ => None,
        };
        Ok(label)
    }

    /// Return every semantic source interpretation that touches one cursor offset.
    ///
    /// The result is intentionally a vector rather than a single “best symbol”: the smallest
    /// body-local node and a path segment can provide different facts for analysis to interpret.
    pub fn occurrences_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();

        for candidate in
            BodyCursorScanner::new(&self.db.body_ir, crate_ref, file_id, offset).scan()?
        {
            if let Some(occurrence) = self.body_occurrence(crate_ref, candidate, Some(file_id))? {
                occurrences.push(occurrence);
            }
        }
        for candidate in
            DefinitionSourceScanner::at(&self.db.def_map, crate_ref, file_id, offset).scan()?
        {
            if let Some(occurrence) = Self::definition_occurrence(crate_ref, candidate) {
                occurrences.push(occurrence);
            }
        }
        for candidate in
            SignatureSourceScanner::at(&self.db.semantic_ir, crate_ref, file_id, offset).scan()?
        {
            if let Some(occurrence) =
                self.signature_occurrence(crate_ref, candidate, Some(file_id))?
            {
                occurrences.push(occurrence);
            }
        }

        Ok(occurrences)
    }

    /// Return every written occurrence in a crate, optionally restricted to one file.
    ///
    /// This is the source inventory used by project-wide reference and rename searches; generated
    /// expansion internals are represented by their editable invocation when possible.
    pub fn occurrences_in_crate(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();

        for candidate in
            DefinitionSourceScanner::in_crate(&self.db.def_map, crate_ref, file_id).scan()?
        {
            if let Some(occurrence) = Self::definition_occurrence(crate_ref, candidate) {
                occurrences.push(occurrence);
            }
        }
        for candidate in BodySourceScanner::new(&self.db.body_ir, crate_ref, file_id).scan()? {
            if let Some(occurrence) = self.body_occurrence(crate_ref, candidate, file_id)? {
                occurrences.push(occurrence);
            }
        }
        for candidate in
            SignatureSourceScanner::in_crate(&self.db.semantic_ir, crate_ref, file_id).scan()?
        {
            if let Some(occurrence) = self.signature_occurrence(crate_ref, candidate, file_id)? {
                occurrences.push(occurrence);
            }
        }

        Ok(occurrences)
    }

    /// Convert a DefMap scanner candidate into a source occurrence.
    fn definition_occurrence(
        crate_ref: CrateRef,
        candidate: DefinitionSourceCandidate,
    ) -> Option<IndexedSourceOccurrence> {
        match candidate {
            DefinitionSourceCandidate::Def { def, file_id, span } => {
                // TODO: Technically we are being defensive here, because enum variants
                // are not candidates. Probably we need a slightly better representation
                // and should get rid of option.
                let declaration = DeclarationRef::try_from_def(def)?;
                Some(IndexedSourceOccurrence::declaration(
                    IndexedSourceFact::Declaration(declaration),
                    crate_ref,
                    file_id,
                    span,
                ))
            }
            DefinitionSourceCandidate::UsePath {
                module,
                path,
                file_id,
                span,
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::UsePath { module, path },
                crate_ref,
                file_id,
                span,
            )),
            DefinitionSourceCandidate::ImportAlias {
                module,
                path,
                file_id,
                span,
            } => Some(IndexedSourceOccurrence::structural(
                IndexedSourceFact::UsePath { module, path },
                crate_ref,
                file_id,
                span,
            )),
        }
    }

    /// Convert a Semantic IR scanner candidate into a source occurrence.
    fn signature_occurrence(
        &self,
        crate_ref: CrateRef,
        candidate: SignatureSourceCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let occurrence = match candidate {
            SignatureSourceCandidate::Field { field, span } => {
                let declaration = DeclarationRef::from(field);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            SignatureSourceCandidate::Function { function, span } => {
                let declaration = DeclarationRef::from(function);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            SignatureSourceCandidate::EnumVariant { variant, span } => {
                let declaration = DeclarationRef::from(variant);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            SignatureSourceCandidate::TypePath {
                context,
                path,
                file_id,
                span,
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::TypePath {
                    scope: IndexedTypePathScope::Signature(context),
                    path,
                },
                crate_ref,
                file_id,
                span,
            )),
        };

        Ok(occurrence)
    }

    /// Convert a Body IR scanner candidate into a source occurrence.
    fn body_occurrence(
        &self,
        crate_ref: CrateRef,
        candidate: BodySourceCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let span = candidate.span();
        let occurrence = match candidate {
            BodySourceCandidate::Body { body, .. } => {
                let Some(data) = self.db.body_ir.body(body)? else {
                    return Ok(None);
                };
                if !data.source().is_written() {
                    return Ok(None);
                }
                let Some(_) = data.function_owner() else {
                    return Ok(None);
                };
                Some(IndexedSourceOccurrence::structural(
                    IndexedSourceFact::FunctionBody(FunctionBodyRef::from_body_ir(body)),
                    crate_ref,
                    data.source().file_id,
                    span,
                ))
            }
            BodySourceCandidate::Binding {
                body,
                binding,
                surface,
                ..
            } => {
                let declaration = DeclarationRef::body_binding(BodyBindingRef { body, binding });
                if let Some(body_data) = self.db.body_ir.body(body)?
                    && let Some(data) = body_data.binding(binding)
                    && !data.source.is_written()
                {
                    return Ok(None);
                }
                match surface {
                    BindingSurface::Plain => {
                        self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
                    }
                    BindingSurface::RecordPatShorthand {
                        key,
                        field_span,
                        pat_span,
                        binding_name_span,
                    } => {
                        let file_id = match self.db.body_ir.body(body)? {
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
                            crate_ref,
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
            BodySourceCandidate::Expr { body, expr, .. } => {
                let file_id = match self.db.body_ir.body(body)? {
                    Some(body_data) => match body_data.expr(expr) {
                        Some(data) if data.source.is_written() => data.source.file_id,
                        Some(_) => return Ok(None),
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
                    crate_ref,
                    file_id,
                    span,
                ))
            }
            BodySourceCandidate::MacroCall {
                definition,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::Declaration(DeclarationRef::local_def(definition)),
                crate_ref,
                file_id,
                span,
            )),
            BodySourceCandidate::LocalItem { item, .. } => {
                let declaration = DeclarationRef::from(item);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            BodySourceCandidate::LocalValueItem { item, .. } => {
                let declaration = DeclarationRef::from(item);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            BodySourceCandidate::LocalField { field, .. } => {
                let declaration = DeclarationRef::from(field);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            BodySourceCandidate::LocalEnumVariant { variant, .. } => {
                let declaration = DeclarationRef::from(variant);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            BodySourceCandidate::LocalFunction { function, .. } => {
                let declaration = DeclarationRef::from(function);
                self.declaration_occurrence(declaration, crate_ref, span, fallback_file_id)?
            }
            BodySourceCandidate::RecordFieldKey {
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
                    crate_ref,
                    file_id,
                    span,
                    surface,
                ))
            }
            BodySourceCandidate::ValueReference {
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
                    fact, crate_ref, file_id, span, surface,
                ))
            }
            BodySourceCandidate::TypePath {
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
                crate_ref,
                file_id,
                span,
            )),
        };

        Ok(occurrence)
    }

    /// Build a declaration occurrence using declaration data for file ownership.
    fn declaration_occurrence(
        &self,
        declaration: DeclarationRef,
        scan_target: CrateRef,
        span: Span,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        // Some scanner families know only the selected span. Use the declaration projection for
        // canonical file ownership, and fall back to the cursor file for point lookups.
        let file_id = match DeclarationView::new(self.db).declaration(declaration)? {
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
