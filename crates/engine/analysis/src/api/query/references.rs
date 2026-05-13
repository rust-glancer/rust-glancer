//! Reference search over the facts already held by the analysis graph.
//!
//! The initial references implementation intentionally scans known source facts instead of building
//! a separate index. That keeps the feature aligned with goto/hover resolution: every candidate is
//! normalized through the same entity resolver before we compare declaration identities.

use rg_body_ir::{
    BodyCursorCandidate, BodyItemRef, BodyRef, ResolvedFieldRef, ResolvedFunctionRef,
};
use rg_def_map::{DefId, LocalDefRef, ModuleRef, Path, TargetRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::{
    ConstRef, EnumVariantRef, SemanticCursorCandidate, StaticRef, TraitRef, TypeAliasRef,
    TypeDefRef,
};

use crate::{
    api::{
        Analysis,
        query::navigation::SymbolResolver,
        query::symbols::shared,
        resolve::entity::{EntityResolver, ResolvedEntity},
    },
    model::{ReferenceLocation, SymbolAt},
};

pub(crate) struct ReferenceResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> ReferenceResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Finds references for the symbol under `offset` by scanning the provided use-site targets.
    ///
    /// Declaration locations are projected from the selected symbol before use-site scanning when
    /// requested, so callers can keep declarations visible even when their search surface excludes
    /// the defining target.
    pub(crate) fn references(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
        include_declaration: bool,
        use_site_targets: &[TargetRef],
    ) -> anyhow::Result<Vec<ReferenceLocation>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };
        let subjects = self.subjects_for_symbol(symbol.clone())?;
        if subjects.is_empty() {
            return Ok(Vec::new());
        }

        let mut locations = Vec::new();
        if include_declaration {
            self.push_selected_declarations(symbol, &mut locations)?;
        }

        for candidate in self.reference_candidates(use_site_targets)? {
            if candidate.is_declaration && !include_declaration {
                continue;
            }

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
        for target in SymbolResolver::new(self.0).resolve_symbol(symbol)? {
            let Some(span) = target.span else {
                continue;
            };
            locations.push(ReferenceLocation {
                target: target.target,
                file_id: target.file_id,
                span,
            });
        }
        Ok(())
    }

    fn subjects_for_symbol(&self, symbol: SymbolAt) -> anyhow::Result<Vec<ReferenceSubject>> {
        let entities = EntityResolver::new(self.0).entities_for_symbol(symbol)?;
        let mut subjects = Vec::new();
        for entity in entities {
            let subject = ReferenceSubject::from_entity(entity);
            if !subjects.contains(&subject) {
                subjects.push(subject);
            }
        }
        Ok(subjects)
    }

    fn reference_candidates(
        &self,
        use_site_targets: &[TargetRef],
    ) -> anyhow::Result<Vec<ReferenceCandidate>> {
        let mut candidates = Vec::new();
        let mut visited = Vec::new();
        for target in use_site_targets {
            if visited.contains(target) {
                continue;
            }
            visited.push(*target);

            self.push_def_map_candidates(*target, &mut candidates)?;
            self.push_body_candidates(*target, &mut candidates)?;
            self.push_semantic_candidates(*target, &mut candidates)?;
        }

        Ok(candidates)
    }

    fn push_def_map_candidates(
        &self,
        target: TargetRef,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for (module_ref, module) in self.0.def_map.modules(target)? {
            let Some(source) = shared::module_declaration_source(module) else {
                continue;
            };
            candidates.push(ReferenceCandidate {
                symbol: SymbolAt::Def {
                    def: DefId::Module(module_ref),
                    span: source.selection_span,
                },
                target,
                file_id: source.file_id,
                span: source.selection_span,
                is_declaration: true,
            });
        }

        for (local_def, data) in self.0.def_map.local_defs(target)? {
            let span = data.name_span.unwrap_or(data.span);
            candidates.push(ReferenceCandidate {
                symbol: SymbolAt::Def {
                    def: DefId::Local(local_def),
                    span,
                },
                target,
                file_id: data.file_id,
                span,
                is_declaration: true,
            });
        }

        for (_, import) in self.0.def_map.imports(target)? {
            let module = ModuleRef {
                target,
                module: import.module,
            };
            for (idx, segment) in import.source_path.segments().iter().enumerate() {
                candidates.push(ReferenceCandidate {
                    symbol: SymbolAt::UsePath {
                        module,
                        path: import.source_path.prefix_path(idx),
                        span: segment.span,
                    },
                    target,
                    file_id: import.source.file_id,
                    span: segment.span,
                    is_declaration: false,
                });
            }

            if let Some(alias_span) = import.alias_span {
                candidates.push(ReferenceCandidate {
                    symbol: SymbolAt::UsePath {
                        module,
                        path: Path::from(&import.path),
                        span: alias_span,
                    },
                    target,
                    file_id: import.source.file_id,
                    span: alias_span,
                    is_declaration: false,
                });
            }
        }

        Ok(())
    }

    fn push_semantic_candidates(
        &self,
        target: TargetRef,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for candidate in self.0.semantic_ir.signature_source_candidates(target)? {
            let Some(candidate) = self.semantic_reference_candidate(target, candidate)? else {
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
                let Some(data) = self.0.semantic_ir.field_data(field)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Field { field, span },
                    target,
                    file_id: data.file_id,
                    span,
                    is_declaration: true,
                }
            }
            SemanticCursorCandidate::Function { function, span } => {
                let Some(data) = self.0.semantic_ir.function_data(function)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::Function { function, span },
                    target,
                    file_id: data.source.file_id,
                    span,
                    is_declaration: true,
                }
            }
            SemanticCursorCandidate::EnumVariant { variant, span } => {
                let Some(data) = self.0.semantic_ir.enum_variant_data(variant)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::EnumVariant { variant, span },
                    target,
                    file_id: data.file_id,
                    span,
                    is_declaration: true,
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
                is_declaration: false,
            },
        };

        Ok(Some(candidate))
    }

    fn push_body_candidates(
        &self,
        target: TargetRef,
        candidates: &mut Vec<ReferenceCandidate>,
    ) -> anyhow::Result<()> {
        for candidate in self.0.body_ir.source_candidates(target)? {
            let Some(candidate) = self.body_reference_candidate(target, candidate)? else {
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
                let Some(body_data) = self.0.body_ir.body_data(body)? else {
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
                    is_declaration: true,
                }
            }
            BodyCursorCandidate::Expr { body, expr, .. } => {
                let Some(body_data) = self.0.body_ir.body_data(body)? else {
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
                    is_declaration: false,
                }
            }
            BodyCursorCandidate::LocalItem { item, .. } => {
                let Some(body_data) = self.0.body_ir.body_data(item.body)? else {
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
                    is_declaration: true,
                }
            }
            BodyCursorCandidate::LocalField { field, .. } => {
                let Some(data) = self.0.body_ir.local_field_data(field)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalField { field, span },
                    target,
                    file_id: data.item.source.file_id,
                    span,
                    is_declaration: true,
                }
            }
            BodyCursorCandidate::LocalFunction { function, .. } => {
                let Some(data) = self.0.body_ir.local_function_data(function)? else {
                    return Ok(None);
                };
                ReferenceCandidate {
                    symbol: SymbolAt::LocalFunction { function, span },
                    target,
                    file_id: data.name_source.file_id,
                    span,
                    is_declaration: true,
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
                is_declaration: false,
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
                is_declaration: false,
            },
        };

        Ok(Some(candidate))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferenceCandidate {
    symbol: SymbolAt,
    target: TargetRef,
    file_id: FileId,
    span: Span,
    is_declaration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReferenceSubject {
    Module(ModuleRef),
    TypeDef(TypeDefRef),
    Trait(TraitRef),
    Function(ResolvedFunctionRef),
    Field(ResolvedFieldRef),
    EnumVariant(EnumVariantRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
    Static(StaticRef),
    LocalBinding {
        body: BodyRef,
        binding: rg_body_ir::BindingId,
    },
    LocalItem(BodyItemRef),
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
            ResolvedEntity::LocalDef(local_def) => Self::LocalDef(local_def),
        }
    }
}
