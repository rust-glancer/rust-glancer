//! Semantic dependency discovery for a goal-directed Chalk program.
//!
//! A goal only names its outer traits and types, but Chalk can follow their predicates into more
//! traits, associated values, and opaque bounds. This module walks that graph before lowering any
//! datums. The result is a closed semantic scope: when the build phase starts, every associated
//! type ID it encounters already has a declaration scheduled for the same program extension.

use rg_def_map::DefMapSource;
use rg_ir_model::{AssocItemId, ItemOwner, TraitDefRef, TypeAliasRef};
use rg_semantic_ir::{CrateItemQuery, ItemStoreSource};

use super::{ChalkProgram, ChalkProgramRoots, ChalkProgramScope};
use crate::inference::InferenceTable;
use crate::trait_selection::TraitSelectionSession;
use crate::{Clause, ItemPathQuery, SemanticSignatureQuery, TraitRefLowering};

impl ChalkProgramRoots {
    pub(super) fn is_empty(&self) -> bool {
        self.traits.is_empty() && self.opaque_tys.is_empty()
    }

    pub(super) fn merge(&mut self, other: &Self) {
        self.traits.extend(other.traits.iter().copied());
        self.opaque_tys.extend(other.opaque_tys.iter().copied());
    }

    pub(super) fn new_since(&self, previous: &Self) -> Self {
        Self {
            traits: self
                .traits
                .iter()
                .filter(|trait_ref| !previous.traits.contains(trait_ref))
                .copied()
                .collect(),
            opaque_tys: self
                .opaque_tys
                .iter()
                .filter(|opaque| !previous.opaque_tys.contains(opaque))
                .copied()
                .collect(),
        }
    }

    pub(super) fn collect_goal<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        goal: &crate::TraitGoal,
        table: &InferenceTable,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.traits.push(goal.trait_ref());
        self.collect_args(item_paths, &goal.application.args, Some(table))?;
        for binding in &goal.associated_types {
            self.collect_associated_ty(item_paths, binding.associated_ty)?;
            self.collect_ty(item_paths, &binding.ty, Some(table))?;
        }
        Ok(())
    }

    pub(super) fn collect_clauses<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        clauses: &[Clause],
        table: Option<&InferenceTable>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for clause in clauses {
            match clause {
                Clause::Implemented(application) => {
                    self.traits.push(application.def);
                    self.collect_args(item_paths, &application.args, table)?;
                }
                Clause::AliasEq { alias, ty } => {
                    self.collect_associated_ty(item_paths, alias.associated_ty)?;
                    self.collect_args(item_paths, &alias.args, table)?;
                    self.collect_ty(item_paths, ty, table)?;
                }
            }
        }
        Ok(())
    }

    fn collect_trait_ref<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        trait_ref: &TraitRefLowering,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        self.traits.push(trait_ref.application.def);
        self.collect_args(item_paths, &trait_ref.application.args, None)?;
        for binding in &trait_ref.associated_types {
            self.collect_associated_ty(item_paths, binding.associated_ty)?;
            self.collect_ty(item_paths, &binding.ty, None)?;
        }
        Ok(())
    }

    fn collect_associated_ty<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        associated_ty: TypeAliasRef,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let Some(data) = item_paths.items().type_alias_data(associated_ty)? else {
            return Ok(());
        };
        if let ItemOwner::Trait(id) = data.owner {
            self.traits.push(TraitDefRef {
                origin: associated_ty.origin,
                id,
            });
        }
        Ok(())
    }

    fn collect_args<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        args: &crate::GenericArgs,
        table: Option<&InferenceTable>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        for arg in args {
            if let crate::GenericArg::Type(ty) = arg {
                self.collect_ty(item_paths, ty, table)?;
            }
        }
        Ok(())
    }

    fn collect_ty<'query, D, I>(
        &mut self,
        item_paths: &ItemPathQuery<'query, D, I>,
        ty: &crate::Ty,
        table: Option<&InferenceTable>,
    ) -> Result<(), I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        // Canonicalizing the complete tree once exposes any goal variables that already resolve to
        // an opaque or projection. Recursive calls then walk that stable shape without cloning at
        // every child.
        if let Some(table) = table {
            return self.collect_ty(item_paths, &table.canonicalize(ty), None);
        }

        match ty {
            crate::Ty::Tuple(fields) => {
                for field in fields {
                    self.collect_ty(item_paths, field, None)?;
                }
            }
            crate::Ty::Array { inner, .. }
            | crate::Ty::Slice(inner)
            | crate::Ty::Reference { inner, .. }
            | crate::Ty::RawPointer { inner, .. } => {
                self.collect_ty(item_paths, inner, None)?;
            }
            crate::Ty::FnPointer { params, ret } => {
                for param in params {
                    self.collect_ty(item_paths, param, None)?;
                }
                self.collect_ty(item_paths, ret, None)?;
            }
            crate::Ty::Adt(ty) => self.collect_args(item_paths, &ty.args, None)?,
            crate::Ty::Alias(crate::AliasTy::Projection(alias)) => {
                self.collect_associated_ty(item_paths, alias.associated_ty)?;
                self.collect_args(item_paths, &alias.args, None)?;
            }
            crate::Ty::Alias(crate::AliasTy::Opaque(alias)) => {
                self.opaque_tys.push(alias.opaque);
                self.collect_args(item_paths, &alias.args, None)?;
            }
            crate::Ty::FnDef(function) => {
                self.collect_args(item_paths, &function.args, None)?;
            }
            crate::Ty::Unit
            | crate::Ty::Never
            | crate::Ty::Primitive(_)
            | crate::Ty::Param(_)
            | crate::Ty::Closure(_)
            | crate::Ty::Unknown
            | crate::Ty::InferVar { .. } => {}
        }
        Ok(())
    }
}

impl ChalkProgramScope {
    /// Expand new roots until trait predicates and opaque bounds introduce no more definitions.
    pub(super) fn discover<'query, D, I>(
        item_paths: &ItemPathQuery<'query, D, I>,
        crate_items: &CrateItemQuery<'query, D, I>,
        session: &TraitSelectionSession,
        roots: &ChalkProgramRoots,
        program: &ChalkProgram,
    ) -> Result<Self, I::Error>
    where
        D: DefMapSource<Error = I::Error>,
        I: ItemStoreSource<'query>,
    {
        let mut scope = Self {
            definitions: roots.clone(),
            ..Self::default()
        };
        let mut trait_cursor = 0;
        let mut opaque_cursor = 0;

        // Each discovered trait contributes its own declaration predicates and all visible impl
        // predicates. Opaque bounds can introduce more traits, so alternate both queues until the
        // semantic dependency closure stops growing.
        while trait_cursor < scope.definitions.traits.len()
            || opaque_cursor < scope.definitions.opaque_tys.len()
        {
            while trait_cursor < scope.definitions.traits.len() {
                let trait_ref = scope.definitions.traits.as_slice()[trait_cursor];
                trait_cursor += 1;
                if program.materialized_traits.contains(&trait_ref) {
                    continue;
                }

                if let Some(header) =
                    SemanticSignatureQuery::trait_header_from(item_paths, trait_ref)?
                {
                    scope
                        .definitions
                        .collect_ty(item_paths, &header.self_ty, None)?;
                    scope
                        .definitions
                        .collect_clauses(item_paths, &header.clauses, None)?;
                    scope.trait_headers.insert(trait_ref, header);
                }

                for impl_ref in crate_items.impls_for_trait(trait_ref)? {
                    if !scope.impls.push(impl_ref) {
                        continue;
                    }
                    let Some(header) =
                        session.impl_header_with(item_paths, item_paths, impl_ref)?
                    else {
                        continue;
                    };
                    scope
                        .definitions
                        .collect_ty(item_paths, &header.self_ty, None)?;
                    if let Some(trait_ref) = &header.trait_ref {
                        scope.definitions.collect_trait_ref(item_paths, trait_ref)?;
                    }
                    scope
                        .definitions
                        .collect_clauses(item_paths, &header.clauses, None)?;

                    // Associated values may themselves project through another trait. Discover
                    // that trait before lowering so every referenced associated-type datum exists.
                    if let Some(impl_data) = crate_items.items().impl_data(impl_ref)? {
                        for item in &impl_data.items {
                            let AssocItemId::TypeAlias(id) = item else {
                                continue;
                            };
                            let alias = TypeAliasRef {
                                origin: impl_ref.origin,
                                id: *id,
                            };
                            if let Some(ty) =
                                SemanticSignatureQuery::type_alias_ty_from(item_paths, alias)?
                            {
                                scope.definitions.collect_ty(item_paths, &ty, None)?;
                            }
                        }
                    }
                    scope.impl_headers.insert(impl_ref, header);
                }
            }

            while opaque_cursor < scope.definitions.opaque_tys.len() {
                let opaque = scope.definitions.opaque_tys.as_slice()[opaque_cursor];
                opaque_cursor += 1;
                if program.materialized_opaque_owners.contains(&opaque.owner) {
                    continue;
                }
                if !scope.loaded_opaque_owners.push(opaque.owner) {
                    continue;
                }

                for (opaque, bounds) in
                    SemanticSignatureQuery::opaque_bounds_for_owner_from(item_paths, opaque.owner)?
                {
                    scope.definitions.opaque_tys.push(opaque.opaque);
                    for bound in &bounds {
                        scope.definitions.collect_trait_ref(item_paths, bound)?;
                    }
                    scope.opaque_bounds.insert(opaque.opaque, (opaque, bounds));
                }
            }
        }

        Ok(scope)
    }
}
