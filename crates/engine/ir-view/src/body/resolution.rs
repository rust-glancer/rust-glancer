//! Body-owned semantic resolution behind the view facade.
//!
//! A body id and lexical scope are needed to resolve type paths, value paths, enum variants,
//! methods, and rich associated qualifiers such as `Widget::<u8>` or `<T as Factory>`. The actual
//! algorithms still live in Body IR; this adapter constructs their shared context in one place and
//! preserves body-local modules and impl overlays for `ir-view` callers.

use anyhow::Context as _;
use rg_body_ir::{BodyAssociatedPathPrefix, BodyResolutionContext, BodyView};
use rg_ir_model::{
    BodyRef, DefMapRef, EnumVariantRef, ModuleId, ModuleRef, Path, ScopeId,
    identity::DeclarationRef,
};
use rg_item_tree::TypeRef;
use rg_semantic_ir::{ItemLookupQuery, TypePathResolution};
use rg_ty::{AssociatedItemCandidateRef, ItemPathQuery, MemberMethodCandidateRef, Ty};

use crate::IndexedViewDb;

/// Runs body-aware resolution queries for view projections.
pub(crate) struct BodyResolutionView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyResolutionView<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    fn body_with_lookup(
        &self,
        body_ref: BodyRef,
    ) -> anyhow::Result<Option<(BodyView<'_>, ItemLookupQuery<'_>)>> {
        let Some(body) = self
            .db
            .body_ir
            .body(body_ref)
            .context("load body for resolution")?
        else {
            return Ok(None);
        };
        let item_lookup_query = self
            .db
            .item_lookup_query(body_ref.crate_ref)
            .context("assemble item lookup query for body resolution")?;

        Ok(Some((body, item_lookup_query)))
    }

    /// Resolves a type path while keeping the common lexical lookup on the exact-read path.
    ///
    /// Ordinary names return before visibility-wide item lookup is assembled. `Self`, associated
    /// aliases, and unresolved lexical paths continue through the complete body resolution context.
    pub(crate) fn type_path_resolution(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<TypePathResolution>> {
        let Some(body) = self
            .db
            .body_ir
            .body(body_ref)
            .context("load body type path")?
        else {
            return Ok(None);
        };

        // Paths such as `App` or `bevy::prelude::App` already have an identity in DefMap. Resolve
        // them from the lexical body scope first so the query reads only the DefMap and item stores
        // on that path. Associated aliases and `Self` need the richer context built below.
        let from = ModuleRef {
            origin: DefMapRef::Body(body_ref),
            module: ModuleId(scope.0),
        };
        let item_paths = ItemPathQuery::new(self.db, self.db);
        let direct_resolution = item_paths
            .resolve_lexical_type_path(from, path)
            .context("resolve lexical body type path")?;
        if !matches!(direct_resolution, TypePathResolution::Unknown) {
            return Ok(Some(direct_resolution));
        }

        let item_lookup_query = self
            .db
            .item_lookup_query(body_ref.crate_ref)
            .context("assemble item lookup query for body type path")?;
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                &item_lookup_query,
                trait_selection,
            )
            .type_path_query()
            .resolve_in_scope(scope, path)
            .context("resolve body type path")?,
        ))
    }

    /// Lower a complete source type without dropping its written generic arguments.
    pub(crate) fn type_ref_ty(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        type_ref: &TypeRef,
    ) -> anyhow::Result<Option<Ty>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body type reference context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                &item_lookup_query,
                trait_selection,
            )
            .resolve_type_ref(scope, type_ref)
            .context("lower body type reference")?,
        ))
    }

    /// Resolve an enum variant selected through a body-local type path.
    pub(crate) fn type_path_enum_variant(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<EnumVariantRef>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body enum variant context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            &item_lookup_query,
            trait_selection,
        )
        .type_path_query()
        .resolve_enum_variant_in_scope(scope, path)
        .map_err(Into::into)
    }

    /// Find declarations for a body value path without local binding ordering.
    pub(crate) fn nonlocal_value_path_declarations(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body value path context")?
        else {
            return Ok(Vec::new());
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            &item_lookup_query,
            trait_selection,
        )
        .value_paths()
        .resolve_nonlocal_path_declarations(scope, path)
        .context("resolve nonlocal body value declarations")
    }

    /// Resolve the type of a body value path without local binding ordering.
    pub(crate) fn nonlocal_value_path_ty(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Ty> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body value type context")?
        else {
            return Ok(Ty::Unknown);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            &item_lookup_query,
            trait_selection,
        )
        .value_paths()
        .resolve_nonlocal_path_ty(scope, path)
        .context("resolve nonlocal body value type")
    }

    /// Return body-aware method refs for a receiver type.
    pub(crate) fn method_candidate_refs_for_ty(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        ty: &Ty,
    ) -> anyhow::Result<Option<Vec<MemberMethodCandidateRef>>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body method context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                &item_lookup_query,
                trait_selection,
            )
            .methods()
            .method_candidates_for_ty(scope, ty)
            .context("resolve body method candidates")?,
        ))
    }

    /// Resolve all associated declarations that may follow a body-owned type-shaped qualifier.
    ///
    /// The prefix preserves forms a DefMap path cannot carry, including `Widget::<u8>` and
    /// `<T as Factory>`. Lookup combines nominal items, visible trait bounds, and body-local impl
    /// overlays; the view layer later collapses duplicate declaration identities.
    pub(crate) fn associated_item_candidate_refs(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        prefix: &BodyAssociatedPathPrefix,
    ) -> anyhow::Result<Option<Vec<AssociatedItemCandidateRef>>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body associated item context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                &item_lookup_query,
                trait_selection,
            )
            .associated_item_candidates(scope, prefix)
            .context("resolve body associated item candidates")?,
        ))
    }

    /// Resolve declarations from the trait used by an associated type binding.
    ///
    /// In `Iterator<It$0 = u8>`, the qualifier is a trait constraint, not a receiver for general
    /// `Iterator::` lookup. This narrower query therefore walks only the resolved trait and its
    /// supertraits.
    pub(crate) fn trait_associated_item_candidate_refs(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        trait_ref: &TypeRef,
    ) -> anyhow::Result<Option<Vec<AssociatedItemCandidateRef>>> {
        let Some((body, item_lookup_query)) = self
            .body_with_lookup(body_ref)
            .context("load body trait item context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection_for_body(body_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                &item_lookup_query,
                trait_selection,
            )
            .trait_associated_item_candidates(scope, trait_ref)
            .context("resolve body trait item candidates")?,
        ))
    }
}
