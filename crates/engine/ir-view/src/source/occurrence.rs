//! Source occurrences for symbol and reference queries.
//!
//! The scanners below read source facts from DefMap, Semantic IR, and Body IR, then normalize them
//! into one facade-owned occurrence vocabulary. Analysis decides what each occurrence means for
//! navigation or references.
//!
//! A single source expression can expose more than one useful fact. In `Action::Start`, for
//! example, `Action` is a type path while `Start` is a value path. The facade keeps those semantic
//! inputs distinct instead of forcing scanners to choose a navigation result.

use anyhow::Context as _;
use rg_body_ir::ExprKind;
use rg_def_map::BodyMacroExpander;
use rg_ir_model::{
    BodyBindingRef, BodyKey, CrateRef, FieldKey, FileId, GenericDefRef, ModuleRef, Path,
    SourceLocation, SourceLocationKind, Span,
    identity::{DeclarationRef, ExprRef, FunctionBodyRef, LexicalScopeRef},
};
use rg_item_tree::TypeRef;
use rg_semantic_ir::{SourceSelection, TypePathContext};
use rg_syntax::SourceFile;

use super::scan::{
    BindingSurface, BodyCursorScanner, BodySourceCandidate, BodySourceScanner,
    DefinitionSourceCandidate, DefinitionSourceScanner, RecordFieldKeySurface,
    SignatureSourceCandidate, SignatureSourceScanner, ValueReferenceSource, ValueReferenceSurface,
};
use crate::{IndexedViewDb, item::declaration::DeclarationView, source::ReferenceSourceSelection};

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

/// How an occurrence relates to macro input, independently of the syntax needed to rewrite it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedSourceProvenance {
    /// A direct source occurrence, including the written macro invocation used for navigation.
    Direct,
    /// A copied input token. Several semantic occurrences can occupy the same written span.
    MacroArgument {
        /// Body lowering checks for duplicated tokens, including copies it cannot lower.
        /// `None` means that the expansion data cannot establish how often the input was used.
        duplicated: Option<bool>,
    },
}

impl IndexedSourceProvenance {
    pub fn from_exact_location(location: SourceLocation) -> Self {
        debug_assert!(
            location.is_exact(),
            "indexed occurrences have exact locations"
        );
        if location.kind == SourceLocationKind::MacroArgument {
            Self::MacroArgument { duplicated: None }
        } else {
            Self::Direct
        }
    }
}

/// Source syntax shape that may need query-specific handling after semantic resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedSourceSurface {
    /// A name inside a format literal. Its spelling follows format syntax (`{type}`), even when
    /// the Rust declaration needs a raw identifier. Provenance separately controls editability.
    FormatCapture,
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
    provenance: IndexedSourceProvenance,
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
        IndexedSourceProvenance,
    ) {
        (
            self.fact,
            self.crate_ref,
            self.file_id,
            self.span,
            self.role,
            self.surface,
            self.provenance,
        )
    }

    /// Build a plain declaration occurrence.
    fn declaration(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        location: SourceLocation,
    ) -> Self {
        Self::declaration_with_surface(
            fact,
            crate_ref,
            file_id,
            location,
            IndexedSourceSurface::Plain,
        )
    }

    /// Build a declaration occurrence with special source-surface handling.
    fn declaration_with_surface(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        location: SourceLocation,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span: location.span,
            role: IndexedSourceRole::Declaration,
            surface,
            provenance: IndexedSourceProvenance::from_exact_location(location),
        }
    }

    /// Build a plain reference occurrence.
    fn reference(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        location: SourceLocation,
    ) -> Self {
        Self::reference_with_surface(
            fact,
            crate_ref,
            file_id,
            location,
            IndexedSourceSurface::Plain,
        )
    }

    /// Build a reference occurrence with special source-surface handling.
    fn reference_with_surface(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        location: SourceLocation,
        surface: IndexedSourceSurface,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span: location.span,
            role: IndexedSourceRole::Reference,
            surface,
            provenance: IndexedSourceProvenance::from_exact_location(location),
        }
    }

    /// Build an occurrence that is neither a declaration nor a reference.
    fn structural(
        fact: IndexedSourceFact,
        crate_ref: CrateRef,
        file_id: FileId,
        location: SourceLocation,
    ) -> Self {
        Self {
            fact,
            crate_ref,
            file_id,
            span: location.span,
            role: IndexedSourceRole::Structural,
            surface: IndexedSourceSurface::Plain,
            provenance: IndexedSourceProvenance::from_exact_location(location),
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
    TypePath(IndexedTypePath),
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
    Signature(IndexedSignatureTypeScope),
    Body(LexicalScopeRef),
}

/// Semantic owner of a type path written in an item signature.
///
/// The type-path context resolves module names and the `Self` binding; the generic owner identifies
/// the type and const parameters inherited by this particular declaration. For example, the cursor in
/// `impl<T> Wrapper<T> { fn map<U>(_: U$0) {} }` needs the function owner to see `U`, while its
/// type-path context supplies the impl's module and `Self` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedSignatureTypeScope {
    context: TypePathContext,
    generic_owner: GenericDefRef,
}

impl IndexedSignatureTypeScope {
    pub(crate) fn new(context: TypePathContext, generic_owner: GenericDefRef) -> Self {
        Self {
            context,
            generic_owner,
        }
    }

    pub fn context(self) -> TypePathContext {
        self.context
    }

    pub fn generic_owner(self) -> GenericDefRef {
        self.generic_owner
    }
}

/// One type path with the data needed for name lookup and type lowering.
///
/// Every occurrence keeps the compact path used by name resolution. A cursor scan also keeps the
/// original `TypeRef`, so a type query can lower written generic arguments such as `<User>`.
/// Whole-file scans omit that extra copy because references and rename only need the compact path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedTypePath {
    scope: IndexedTypePathScope,
    path: Path,
    type_ref: Option<TypeRef>,
}

impl IndexedTypePath {
    fn new(scope: IndexedTypePathScope, path: Path, type_ref: Option<TypeRef>) -> Self {
        Self {
            scope,
            path,
            type_ref,
        }
    }

    pub fn scope(&self) -> IndexedTypePathScope {
        self.scope
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn type_ref(&self) -> Option<&TypeRef> {
        self.type_ref.as_ref()
    }
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

    /// Find caller-written format templates through macro expansion, without loading bodies.
    pub fn format_string_spans(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        syntax: &SourceFile,
        range: Option<Span>,
    ) -> anyhow::Result<Vec<Span>> {
        let config = self.db.crate_config(crate_ref)?;
        BodyMacroExpander::new(&self.db.def_map, self.db.cancellation().clone())
            .format_string_spans(&config, crate_ref, file, syntax, range)
    }

    /// Returns the named source spelling selected by an expression, when the expression has one.
    ///
    /// Expression-backed occurrences intentionally expose only `ExprRef`, so callers that need a
    /// cheap source label can ask the view to inspect the lowered expression. For example, this
    /// returns `user` for a path expression, `push` for `items.push(value)`, `name` for
    /// `user.name`, and `User` for `model::User { name }`; literals and operators return `None`.
    pub fn expr_source_label(&self, expr: ExprRef) -> anyhow::Result<Option<String>> {
        let Some(body_data) = self.db.body_ir.body(expr.body_ir())? else {
            return Ok(None);
        };
        let Some(expr_data) = body_data.expr(expr.expr_id()) else {
            return Ok(None);
        };

        let label = match &expr_data.kind {
            ExprKind::Path { path }
            | ExprKind::Record {
                path: Some(path), ..
            } => path
                .as_def_map_path()
                .and_then(|path| path.last_segment_label()),
            ExprKind::MethodCall { method_name, .. } => Some(method_name.to_string()),
            ExprKind::Field { field, .. } => field.as_ref().map(|field| field.declaration_label()),
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
        for candidate in BodyCursorScanner::new(self.db, crate_ref, file_id, offset).scan()? {
            if let Some(occurrence) = self
                .body_occurrence(crate_ref, candidate, Some(file_id))
                .context("convert body occurrence at cursor")?
            {
                occurrences.push(occurrence);
            }
        }
        occurrences.extend(self.declaration_occurrences(crate_ref, file_id, Some(offset))?);
        Ok(occurrences)
    }

    /// Return declaration occurrences using the active declaration source indexes.
    ///
    /// With no offset, collect the whole file for consumers that associate several declarations.
    /// Body IR is excluded so documentation lookup can find owners without inferring expressions.
    pub(crate) fn declaration_occurrences(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: Option<u32>,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();
        for candidate in match offset {
            Some(offset) => DefinitionSourceScanner::at(self.db, crate_ref, file_id, offset),
            None => DefinitionSourceScanner::in_crate(self.db, crate_ref, Some(file_id)),
        }
        .scan()?
        {
            if let Some(occurrence) = Self::definition_occurrence(crate_ref, candidate) {
                occurrences.push(occurrence);
            }
        }
        for candidate in match offset {
            Some(offset) => SignatureSourceScanner::at(self.db, crate_ref, file_id, offset),
            None => SignatureSourceScanner::in_crate(self.db, crate_ref, Some(file_id)),
        }
        .scan()?
        {
            if let Some(occurrence) =
                self.signature_occurrence(crate_ref, candidate, Some(file_id))?
            {
                occurrences.push(occurrence);
            }
        }
        Ok(occurrences)
    }

    /// Collect source occurrences from a crate for reference and rename searches.
    ///
    /// A copied argument contributes each expanded occurrence at the same written range.
    /// The caller can limit the scan to one file or use search hints to select body owners.
    pub fn occurrences_in_crate(
        &self,
        crate_ref: CrateRef,
        file_id: Option<FileId>,
        selection: Option<&ReferenceSourceSelection>,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();

        for candidate in DefinitionSourceScanner::in_crate(self.db, crate_ref, file_id).scan()? {
            if let Some(occurrence) = Self::definition_occurrence(crate_ref, candidate) {
                occurrences.push(occurrence);
            }
        }
        // Broad searches discover owners from the file and spelling hints. Local searches
        // already have their owner keys and enter the same body scanner directly below.
        let files = match file_id {
            Some(file) => vec![file],
            None => self.db.body_ir.body_files(crate_ref)?,
        };
        let macro_definitions =
            selection.and_then(|selection| selection.macro_definitions.as_deref());
        for file in files {
            rg_std::check_cancel!(self.db, "body scan file");
            let selected = selection
                .map(|selection| selection.owners(self.db, crate_ref, file))
                .transpose()?;
            let owners: Vec<_> = self
                .db
                .body_ir
                .owners(crate_ref, SourceSelection::AllInFile(file))?
                .filter(|owner| {
                    selected
                        .as_ref()
                        .is_none_or(|selected| selected.contains(&owner.owner))
                })
                .map(|owner| BodyKey {
                    krate: crate_ref,
                    owner: owner.owner,
                })
                .collect();
            let scanner = BodySourceScanner::new(self.db, file_id, &owners);
            let candidates = match macro_definitions {
                Some(definitions) => scanner.macro_calls_to(definitions)?,
                None => scanner.scan()?,
            };
            for candidate in candidates {
                if let Some(occurrence) = self
                    .body_occurrence(crate_ref, candidate, file_id)
                    .context("convert body occurrence")?
                {
                    occurrences.push(occurrence);
                }
            }
        }
        for candidate in SignatureSourceScanner::in_crate(self.db, crate_ref, file_id).scan()? {
            if let Some(occurrence) = self
                .signature_occurrence(crate_ref, candidate, file_id)
                .context("convert signature occurrence")?
            {
                occurrences.push(occurrence);
            }
        }

        Ok(occurrences)
    }

    /// Local identity selects the complete owning worklist, including nested declarations and
    /// included source. The reference query applies any requested file restriction to the result.
    pub fn occurrences_in_body(
        &self,
        owner: BodyKey,
    ) -> anyhow::Result<Vec<IndexedSourceOccurrence>> {
        let mut occurrences = Vec::new();
        for candidate in BodySourceScanner::new(self.db, None, &[owner]).scan()? {
            if let Some(occurrence) = self.body_occurrence(owner.krate, candidate, None)? {
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
        Some(match candidate {
            DefinitionSourceCandidate::Def {
                def,
                file_id,
                location,
            } => {
                let declaration = DeclarationRef::try_from_def(def)?;
                IndexedSourceOccurrence::declaration(
                    IndexedSourceFact::Declaration(declaration),
                    crate_ref,
                    file_id,
                    location,
                )
            }
            DefinitionSourceCandidate::UsePath {
                module,
                path,
                file_id,
                location,
            } => IndexedSourceOccurrence::reference(
                IndexedSourceFact::UsePath { module, path },
                crate_ref,
                file_id,
                location,
            ),
            DefinitionSourceCandidate::ImportAlias {
                module,
                path,
                file_id,
                location,
            } => IndexedSourceOccurrence::structural(
                IndexedSourceFact::UsePath { module, path },
                crate_ref,
                file_id,
                location,
            ),
        })
    }

    /// Convert a Semantic IR scanner candidate into a source occurrence.
    #[rg_std::cancelable("signature occurrence conversion", token = self.db)]
    fn signature_occurrence(
        &self,
        crate_ref: CrateRef,
        candidate: SignatureSourceCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let occurrence = match candidate {
            SignatureSourceCandidate::Field { field, location } => {
                let declaration = DeclarationRef::from(field);
                self.declaration_occurrence(declaration, crate_ref, location, fallback_file_id)?
            }
            SignatureSourceCandidate::Function { function, location } => {
                let declaration = DeclarationRef::from(function);
                self.declaration_occurrence(declaration, crate_ref, location, fallback_file_id)?
            }
            SignatureSourceCandidate::EnumVariant { variant, location } => {
                let declaration = DeclarationRef::from(variant);
                self.declaration_occurrence(declaration, crate_ref, location, fallback_file_id)?
            }
            SignatureSourceCandidate::TypePath {
                scope,
                path,
                type_ref,
                file_id,
                location,
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::TypePath(IndexedTypePath::new(
                    IndexedTypePathScope::Signature(IndexedSignatureTypeScope::new(
                        scope.context,
                        scope.generic_owner,
                    )),
                    path,
                    type_ref.map(|type_ref| *type_ref),
                )),
                crate_ref,
                file_id,
                location,
            )),
        };

        Ok(occurrence)
    }

    /// Convert a Body IR scanner candidate into a source occurrence.
    #[rg_std::cancelable("body occurrence conversion", token = self.db)]
    fn body_occurrence(
        &self,
        crate_ref: CrateRef,
        candidate: BodySourceCandidate,
        fallback_file_id: Option<FileId>,
    ) -> anyhow::Result<Option<IndexedSourceOccurrence>> {
        let location = candidate.location();
        // The location tells us whether the name came from macro input. The source map tells
        // us whether the input was copied more than once. Look up format captures for direct
        // expressions too, since their replacement spelling follows the format string.
        let body_sources = match &candidate {
            BodySourceCandidate::Expr { body, .. } => self.db.body_ir.body_sources(*body)?,
            BodySourceCandidate::MacroCall { body, .. }
            | BodySourceCandidate::Binding { body, .. }
            | BodySourceCandidate::RecordFieldKey { body, .. }
            | BodySourceCandidate::ValueReference { body, .. }
            | BodySourceCandidate::TypePath { body, .. }
                if location.kind == SourceLocationKind::MacroArgument =>
            {
                self.db.body_ir.body_sources(*body)?
            }
            _ => None,
        };
        let source_map = body_sources.map(|sources| sources.provenance());
        let format_capture = matches!(&candidate, BodySourceCandidate::Expr { expr, .. }
            if source_map.is_some_and(|map| map.is_format_capture(*expr, location.span)));
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
                    location,
                ))
            }
            BodySourceCandidate::Binding {
                body,
                binding,
                file_id,
                surface,
                ..
            } => {
                let declaration = DeclarationRef::body_binding(BodyBindingRef { body, binding });
                let surface = match surface {
                    BindingSurface::Plain => IndexedSourceSurface::Plain,
                    BindingSurface::RecordPatShorthand {
                        key,
                        field_span,
                        pat_span,
                        binding_name_span,
                    } => IndexedSourceSurface::RecordPatShorthandBinding {
                        key,
                        field_span,
                        pat_span,
                        binding_name_span,
                    },
                };
                Some(IndexedSourceOccurrence::declaration_with_surface(
                    IndexedSourceFact::Declaration(declaration),
                    crate_ref,
                    file_id,
                    location,
                    surface,
                ))
            }
            BodySourceCandidate::Expr {
                body,
                expr,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::Expr(ExprRef::new(body, expr)),
                crate_ref,
                file_id,
                location,
            )),
            BodySourceCandidate::MacroCall {
                definition,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::Declaration(DeclarationRef::local_def(definition)),
                crate_ref,
                file_id,
                location,
            )),
            BodySourceCandidate::LocalDeclaration { declaration, .. } => {
                self.declaration_occurrence(declaration, crate_ref, location, fallback_file_id)?
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
                    location,
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
                    fact, crate_ref, file_id, location, surface,
                ))
            }
            BodySourceCandidate::TypePath {
                body,
                scope,
                path,
                type_ref,
                file_id,
                ..
            } => Some(IndexedSourceOccurrence::reference(
                IndexedSourceFact::TypePath(IndexedTypePath::new(
                    IndexedTypePathScope::Body(LexicalScopeRef::new(body, scope)),
                    path,
                    type_ref,
                )),
                crate_ref,
                file_id,
                location,
            )),
        };

        Ok(occurrence.map(|mut occurrence| {
            if location.kind == SourceLocationKind::MacroArgument {
                occurrence.provenance = IndexedSourceProvenance::MacroArgument {
                    duplicated: source_map
                        .and_then(|map| map.macro_argument_duplication(location.span)),
                };
            }
            if format_capture {
                occurrence.surface = IndexedSourceSurface::FormatCapture;
            }
            occurrence
        }))
    }

    /// Build a declaration occurrence using declaration data for file ownership.
    fn declaration_occurrence(
        &self,
        declaration: DeclarationRef,
        scan_target: CrateRef,
        location: SourceLocation,
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
            location,
        )))
    }
}
