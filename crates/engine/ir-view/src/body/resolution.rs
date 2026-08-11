//! Body-owned semantic resolution behind the view facade.
//!
//! A body id and lexical scope are needed to resolve type paths, value paths, enum variants,
//! methods, and rich associated qualifiers such as `Widget::<u8>` or `<T as Factory>`. The actual
//! algorithms still live in Body IR; this adapter constructs their shared context in one place and
//! preserves body-local modules and impl overlays for `ir-view` callers.

use anyhow::Context as _;
use rg_body_ir::{BodyAssociatedPathPrefix, BodyResolutionContext, BodyView};
use rg_ir_model::{BodyRef, EnumVariantRef, Path, ScopeId, identity::DeclarationRef};
use rg_item_tree::TypeRef;
use rg_semantic_ir::{ItemLookupIndex, TypePathResolution};
use rg_ty::{AssociatedItemCandidateRef, MemberMethodCandidateRef, Ty};

use crate::IndexedViewDb;

/// Runs body-aware resolution queries for view projections.
pub(crate) struct BodyResolutionView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> BodyResolutionView<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    fn body_with_index(
        &self,
        body_ref: BodyRef,
    ) -> anyhow::Result<Option<(BodyView<'_>, &ItemLookupIndex)>> {
        let Some(body) = self
            .db
            .body_ir
            .body(body_ref)
            .context("load body for resolution")?
        else {
            return Ok(None);
        };
        let Some(item_lookup_index) = self
            .db
            .body_ir
            .item_lookup_index(body_ref.crate_ref)
            .context("load item lookup index for body resolution")?
        else {
            return Ok(None);
        };

        Ok(Some((body, item_lookup_index)))
    }

    /// Resolve a type path in a body scope.
    pub(crate) fn type_path_resolution(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<TypePathResolution>> {
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body type path context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                item_lookup_index,
                trait_selection,
            )
            .type_path_query()
            .resolve_in_scope(scope, path)
            .context("resolve body type path")?,
        ))
    }

    /// Resolve an enum variant selected through a body-local type path.
    pub(crate) fn type_path_enum_variant(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Option<EnumVariantRef>> {
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body enum variant context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            item_lookup_index,
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
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body value path context")?
        else {
            return Ok(Vec::new());
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            item_lookup_index,
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
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body value type context")?
        else {
            return Ok(Ty::Unknown);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        BodyResolutionContext::new(
            self.db,
            self.db,
            body_ref,
            body,
            item_lookup_index,
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
        ty: &Ty,
    ) -> anyhow::Result<Option<Vec<MemberMethodCandidateRef>>> {
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body method context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                item_lookup_index,
                trait_selection,
            )
            .methods()
            .method_candidates_for_ty(ty)
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
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body associated item context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                item_lookup_index,
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
        let Some((body, item_lookup_index)) = self
            .body_with_index(body_ref)
            .context("load body trait item context")?
        else {
            return Ok(None);
        };
        let trait_selection = self.db.trait_selection(body_ref.crate_ref);

        Ok(Some(
            BodyResolutionContext::new(
                self.db,
                self.db,
                body_ref,
                body,
                item_lookup_index,
                trait_selection,
            )
            .trait_associated_item_candidates(scope, trait_ref)
            .context("resolve body trait item candidates")?,
        ))
    }
}
