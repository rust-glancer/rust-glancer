//! Reference search over the facts already held by the analysis graph.
//!
//! The initial references implementation intentionally scans known source facts instead of building
//! a separate index. That keeps the feature aligned with goto/hover resolution: every candidate is
//! normalized through the same entity resolver before we compare declaration identities.

use rg_body_ir::{
    BodyCursorCandidate, BodyItemRef, BodyRef, ResolvedEnumVariantRef, ResolvedFieldRef,
    ResolvedFunctionRef,
};
use rg_def_map::{DefMapCursorCandidate, LocalDefRef, ModuleRef, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    ConstRef, SemanticCursorCandidate, StaticRef, TraitRef, TypeAliasRef, TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        query::navigation::SymbolResolver,
        resolve::entity::{EntityResolver, ResolvedEntity},
    },
    model::{ReferenceLocation, SymbolAt},
};

/// Options for a source reference lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceQuery<'a> {
    search_scope: ReferenceSearchScope<'a>,
    declaration_policy: ReferenceDeclarationPolicy,
}

impl<'a> ReferenceQuery<'a> {
    /// Returns a query for explicit find-references requests.
    pub fn find_references(search_targets: &'a [TargetRef], include_declarations: bool) -> Self {
        let declaration_policy = if include_declarations {
            ReferenceDeclarationPolicy::IncludeUnscoped
        } else {
            ReferenceDeclarationPolicy::Exclude
        };

        Self {
            search_scope: ReferenceSearchScope::Targets(search_targets),
            declaration_policy,
        }
    }

    /// Returns a query scoped to one file inside one target.
    pub fn file_scoped(target: TargetRef, file_id: FileId) -> Self {
        Self {
            search_scope: ReferenceSearchScope::File { target, file_id },
            declaration_policy: ReferenceDeclarationPolicy::IncludeInSearchScope,
        }
    }

    /// Removes declaration locations from this query.
    pub fn without_declarations(mut self) -> Self {
        self.declaration_policy = ReferenceDeclarationPolicy::Exclude;
        self
    }

    fn search_scope(self) -> ReferenceSearchScope<'a> {
        self.search_scope
    }

    fn includes_declarations(self) -> bool {
        !matches!(self.declaration_policy, ReferenceDeclarationPolicy::Exclude)
    }

    fn accepts_declaration(self, target: TargetRef, file_id: FileId) -> bool {
        match self.declaration_policy {
            ReferenceDeclarationPolicy::Exclude => false,
            ReferenceDeclarationPolicy::IncludeUnscoped => true,
            ReferenceDeclarationPolicy::IncludeInSearchScope => match self.search_scope {
                ReferenceSearchScope::Targets(targets) => targets.contains(&target),
                ReferenceSearchScope::File {
                    target: selected_target,
                    file_id: selected_file_id,
                } => selected_target == target && selected_file_id == file_id,
            },
        }
    }
}

/// Source surface scanned for reference use-sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceSearchScope<'a> {
    /// Scans all source candidates inside the listed targets.
    Targets(&'a [TargetRef]),
    /// Scans source candidates in one file inside one target.
    File { target: TargetRef, file_id: FileId },
}

/// How declaration locations should relate to the reference search surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceDeclarationPolicy {
    /// Do not return declaration locations.
    Exclude,
    /// Return declarations even when they are outside `ReferenceSearchScope`.
    IncludeUnscoped,
    /// Return declarations only when they are inside `ReferenceSearchScope`.
    IncludeInSearchScope,
}

pub(crate) struct ReferenceResolver<'a, 'db, 'scope> {
    analysis: &'a Analysis<'db>,
    query: ReferenceQuery<'scope>,
}

impl<'a, 'db, 'scope> ReferenceResolver<'a, 'db, 'scope> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, query: ReferenceQuery<'scope>) -> Self {
        Self { analysis, query }
    }

    /// Finds references for the symbol under `offset` by scanning the requested use-site surface.
    ///
    /// Declaration locations are projected from the selected symbol before use-site scanning when
    /// requested, using the resolver's declaration scope policy.
    pub(crate) fn references(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        let Some(symbol) = self.analysis.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let subjects = self.subjects_for_symbol(symbol.clone())?;
        if subjects.is_empty() {
            return Ok(Vec::new());
        }

        let mut locations = Vec::new();
        if self.query.includes_declarations() {
            self.push_selected_declarations(symbol, &mut locations)?;
        }

        for candidate in self.reference_candidates()? {
            let candidate_subjects = self.subjects_for_symbol(candidate.symbol)?;
            if candidate_subjects
                .iter()
                .any(|candidate| subjects.contains(candidate))
            {
                locations.push(ReferenceLocation {
                    target: candidate.target,
                    file_id: candidate.file_id,
                    span: candidate.span,
                });
            }
        }

        locations.sort_by_key(|location| {
            (
                location.target.package.0,
                location.target.target.0,
                location.file_id.0,
                location.span.text.start,
                location.span.text.end,
            )
        });
        locations.dedup();
        Ok(locations)
    }

    fn push_selected_declarations(
        &self,
        symbol: SymbolAt,
        locations: &mut Vec<ReferenceLocation>,
    ) -> anyhow::Result<()> {
        for target in SymbolResolver::new(self.analysis).resolve_symbol(symbol)? {
            let Some(span) = target.span else {
                continue;
            };
            if !self
                .query
                .accepts_declaration(target.target, target.file_id)
            {
                continue;
            }
            locations.push(ReferenceLocation {
                target: target.target,
                file_id: target.file_id,
                span,
            });
        }
        Ok(())
    }

    fn subjects_for_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<ReferenceSubject>> {
        let entities = EntityResolver::new(self.analysis).entities_for_symbol(symbol)?;
        let mut subjects = Vec::new();
        for entity in entities {
            let subject = ReferenceSubject::from_entity(entity);
            if !subjects.contains(&subject) {
                subjects.push(subject);
            }
        }
        Ok(subjects)
    }

    fn reference_candidates(&self) -> anyhow::Result<Vec<ReferenceCandidate>> {
        let mut candidates = Vec::new();
        let mut visited = Vec::new();

        match self.query.search_scope() {
            ReferenceSearchScope::Targets(targets) => {
                for target in targets {
                    let scan = ReferenceScanTarget {
                        target: *target,
                        file_id: None,
                    };
                    if visited.contains(&scan) {
                        continue;
                    }
                    visited.push(scan);
                    self.push_scan_target_candidates(scan, &mut candidates)?;
                }
            }
            ReferenceSearchScope::File { target, file_id } => {
                self.push_scan_target_candidates(
                    ReferenceScanTarget {
                        target,
                        file_id: Some(file_id),
                    },
                    &mut candidates,
                )?;
            }
        }

        Ok(candidates)
    }

    fn push_scan_target_candidates(
        &self,
        scan: ReferenceScanTarget,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        self.push_def_map_candidates(scan, candidates)?;
        self.push_body_candidates(scan, candidates)?;
        self.push_semantic_candidates(scan, candidates)?;
        Ok(())
    }

    fn push_def_map_candidates(
        &self,
        scan: ReferenceScanTarget,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for candidate in self
            .analysis
            .def_map
            .source_candidates(scan.target, scan.file_id)?
        {
            let candidate = match candidate {
                DefMapCursorCandidate::Def { def, file_id, span } => {
                    if !self.query.includes_declarations() {
                        continue;
                    }
                    ReferenceCandidate {
                        symbol: SymbolAt::Def { def, span },
                        target: scan.target,
                        file_id,
                        span,
                    }
                }
                DefMapCursorCandidate::UsePath {
                    module,
                    path,
                    file_id,
                    span,
                } => ReferenceCandidate {
                    symbol: SymbolAt::UsePath { module, path, span },
                    target: scan.target,
                    file_id,
                    span,
                },
            };
            candidates.push(candidate);
        }

        Ok(())
    }

    fn push_semantic_candidates(
        &self,
        scan: ReferenceScanTarget,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for candidate in self
            .analysis
            .semantic_ir
            .signature_source_candidates(scan.target, scan.file_id)?
        {
            let Some(candidate) = self.semantic_reference_candidate(scan.target, candidate)? else {
                continue;
            };
            candidates.push(candidate);
        }
        Ok(())
    }

    fn semantic_reference_candidate(
        &self,
        target: TargetRef,
        candidate: SemanticCursorCandidate,
    ) -> anyhow::Result<Option<ReferenceCandidate>> {
        let candidate = match candidate {
            SemanticCursorCandidate::Field { field, span } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.semantic_ir.field_data(field)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Field { field, span },
                    target,
                    file_id: data.file_id,
                    span,
                }
            }
            SemanticCursorCandidate::Function { function, span } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.semantic_ir.function_data(function)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Function { function, span },
                    target,
                    file_id: data.source.file_id,
                    span,
                }
            }
            SemanticCursorCandidate::EnumVariant { variant, span } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.semantic_ir.enum_variant_data(variant)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::EnumVariant { variant, span },
                    target,
                    file_id: data.file_id,
                    span,
                }
            }
            SemanticCursorCandidate::TypePath {
                context,
                path,
                file_id,
                span,
            } => ReferenceCandidate {
                symbol: SymbolAt::TypePath {
                    context,
                    path,
                    span,
                },
                target,
                file_id,
                span,
            },
        };

        Ok(Some(candidate))
    }

    fn push_body_candidates(
        &self,
        scan: ReferenceScanTarget,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for candidate in self
            .analysis
            .body_ir
            .source_candidates(scan.target, scan.file_id)?
        {
            let Some(candidate) = self.body_reference_candidate(scan.target, candidate)? else {
                continue;
            };
            candidates.push(candidate);
        }
        Ok(())
    }

    fn body_reference_candidate(
        &self,
        target: TargetRef,
        candidate: BodyCursorCandidate,
    ) -> anyhow::Result<Option<ReferenceCandidate>> {
        let span = candidate.span();
        let candidate = match candidate {
            BodyCursorCandidate::Body { .. } => return Ok(None),
            BodyCursorCandidate::Binding { body, binding, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(body_data) = self.analysis.body_ir.body_data(body)? else {
                    return Ok(None);
                };
                let Some(data) = body_data.binding(binding) else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Binding { body, binding },
                    target,
                    file_id: data.source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::Expr { body, expr, .. } => {
                let Some(body_data) = self.analysis.body_ir.body_data(body)? else {
                    return Ok(None);
                };
                let Some(data) = body_data.expr(expr) else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Expr { body, expr },
                    target,
                    file_id: data.source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::LocalItem { item, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(body_data) = self.analysis.body_ir.body_data(item.body)? else {
                    return Ok(None);
                };
                let Some(data) = body_data.local_item(item.item) else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalItem { item, span },
                    target,
                    file_id: data.name_source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::LocalValueItem { item, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(body_data) = self.analysis.body_ir.body_data(item.body)? else {
                    return Ok(None);
                };
                let Some(data) = body_data.local_value_item(item.item) else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalValueItem { item, span },
                    target,
                    file_id: data.name_source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::LocalField { field, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.body_ir.local_field_data(field)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalField { field, span },
                    target,
                    file_id: data.item.source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::LocalEnumVariant { variant, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.body_ir.local_enum_variant_data(variant)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalEnumVariant { variant, span },
                    target,
                    file_id: data.item.source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::LocalFunction { function, .. } => {
                if !self.query.includes_declarations() {
                    return Ok(None);
                }
                let Some(data) = self.analysis.body_ir.local_function_data(function)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalFunction { function, span },
                    target,
                    file_id: data.name_source.file_id,
                    span,
                }
            }
            BodyCursorCandidate::TypePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => ReferenceCandidate {
                symbol: SymbolAt::BodyPath {
                    body,
                    scope,
                    path,
                    span,
                },
                target,
                file_id,
                span,
            },
            BodyCursorCandidate::ValuePath {
                body,
                scope,
                path,
                file_id,
                ..
            } => ReferenceCandidate {
                symbol: SymbolAt::BodyValuePath {
                    body,
                    scope,
                    path,
                    span,
                },
                target,
                file_id,
                span,
            },
        };

        Ok(Some(candidate))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReferenceScanTarget {
    target: TargetRef,
    file_id: Option<FileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferenceCandidate {
    symbol: SymbolAt,
    target: TargetRef,
    file_id: FileId,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReferenceSubject {
    Module(ModuleRef),
    TypeDef(TypeDefRef),
    Trait(TraitRef),
    Function(ResolvedFunctionRef),
    Field(ResolvedFieldRef),
    EnumVariant(ResolvedEnumVariantRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
    LocalBinding {
        body: BodyRef,
        binding: rg_body_ir::BindingId,
    },
    LocalItem(BodyItemRef),
    LocalValueItem(rg_body_ir::BodyValueItemRef),
    LocalDef(LocalDefRef),
}

impl ReferenceSubject {
    fn from_entity(entity: ResolvedEntity) -> Self {
        match entity {
            ResolvedEntity::Module { module, .. } => Self::Module(module),
            ResolvedEntity::TypeDef(ty) => Self::TypeDef(ty),
            ResolvedEntity::Trait(trait_ref) => Self::Trait(trait_ref),
            ResolvedEntity::Function(function) => Self::Function(function),
            ResolvedEntity::Field(field) => Self::Field(field),
            ResolvedEntity::EnumVariant(variant) => Self::EnumVariant(variant),
            ResolvedEntity::TypeAlias(type_alias) => Self::TypeAlias(type_alias),
            ResolvedEntity::Const(const_ref) => Self::Const(const_ref),
            ResolvedEntity::Static(static_ref) => Self::Static(static_ref),
            ResolvedEntity::LocalBinding { body, binding } => Self::LocalBinding { body, binding },
            ResolvedEntity::LocalItem(item) => Self::LocalItem(item),
            ResolvedEntity::LocalValueItem(item) => Self::LocalValueItem(item),
            ResolvedEntity::LocalDef(local_def) => Self::LocalDef(local_def),
        }
    }
}
