//! Pattern binding candidate materialization.
//!
//! Lowering has to build scopes before value-name resolution is available, so ambiguous identifier
//! patterns temporarily occupy binding slots. This query decides which slots are real bindings.
//! The caller compacts the body to its final binding arena once the query has released the
//! immutable body view.

use rg_def_map::{DefMapSource, NamespaceSet};
use rg_ir_model::{
    BindingId, DefId, DefMapRef, ModuleId, ModuleRef, Path, ScopeId, SemanticItemRef,
    identity::DeclarationRef,
};
use rg_semantic_ir::{ItemLookupQuery, SemanticItemSource};
use rg_source::ReadError;

use super::lower::{LoweredBodyData, PendingBindingResolution};
use crate::{body::facts::BodyResolution, resolution::BodyResolutionContext};

/// Decide which lowered pattern identifiers are bindings. The body remains immutable until
/// all decisions are made: lexical lookup still needs the original pending binding identities.
pub(crate) struct PatternBindingQuery<'query, D, I> {
    context: BodyResolutionContext<'query, &'query D, &'query I>,
    body: &'query LoweredBodyData,
}

impl<'query, D, I> PatternBindingQuery<'query, D, I>
where
    for<'source> &'source D: DefMapSource,
    for<'source> &'source I: SemanticItemSource<'source>,
{
    pub(crate) fn new(
        def_maps: &'query D,
        item_stores: &'query I,
        item_lookup_query: &ItemLookupQuery<'query>,
        body_ref: rg_ir_model::BodyRef,
        body: &'query LoweredBodyData,
        cancellation: &rg_std::CancellationToken,
    ) -> Self {
        Self {
            context: BodyResolutionContext::new(
                def_maps,
                item_stores,
                body_ref,
                body.body(),
                item_lookup_query,
                cancellation.clone(),
            ),
            body,
        }
    }

    /// Compute the final binding membership, then release the borrowed view before compaction.
    pub(crate) fn active_bindings(self) -> Result<Vec<bool>, ReadError> {
        // `active` is indexed by the original pending binding ids. We keep this temporary view
        // while lookup still needs source-order visibility against the lowered scope lists.
        let pending_count = self.body.body().bindings().len();
        let mut active = Vec::with_capacity(pending_count);
        for binding_idx in 0..pending_count {
            let binding = BindingId(binding_idx);
            let is_active = match self.body.pending_binding_resolution(binding) {
                PendingBindingResolution::AlwaysBinding => true,
                PendingBindingResolution::AmbiguousPattern => {
                    !self.ambiguous_pattern_binding_resolves_as_value(binding, &active)?
                }
            };
            active.push(is_active);
        }

        Ok(active)
    }

    /// Returns true when an ambiguous identifier pattern should stay a value path.
    fn ambiguous_pattern_binding_resolves_as_value(
        &self,
        binding: BindingId,
        active: &[bool],
    ) -> Result<bool, ReadError> {
        let Some(binding_data) = self.body.body().binding(binding) else {
            return Ok(false);
        };
        let Some(name) = binding_data.name.as_deref() else {
            return Ok(false);
        };

        // Local bindings shadow same-named constants and variants in pattern position. Because we
        // are still in pending-id space, only earlier active bindings can participate.
        if self.previous_active_binding_visible(binding_data.scope, binding, name, active) {
            return Ok(false);
        }

        let path = Path::unqualified_name(name);
        if self.pattern_path_resolves_to_const_like(binding_data.scope, &path)? {
            return Ok(true);
        }

        let (resolution, _) = self
            .context
            .value_paths()
            .resolve_nonlocal_path_expr(binding_data.scope, &path)?;

        Ok(matches!(
            resolution,
            BodyResolution::Declarations(declarations)
                if declarations.iter().any(Self::declaration_is_const_like_pattern)
        ))
    }

    fn pattern_path_resolves_to_const_like(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<bool, ReadError> {
        // Check the body-local lexical module first, then the owner/fallback modules used by
        // ordinary body lookup. This keeps body-local consts visible without losing crate items.
        let from = ModuleRef {
            origin: DefMapRef::Body(self.context.body_ref()),
            module: ModuleId(scope.0),
        };
        let def_maps = self.context.def_map_query();
        let body_defs = def_maps
            .name_resolver()
            .resolve_lexical_path(from, path, NamespaceSet::VALUES)?
            .resolved;
        if self.semantic_items_include_const_like(body_defs)? {
            return Ok(true);
        }

        let owner_module = self.body.body().owner_module();
        let owner_defs = def_maps
            .name_resolver()
            .resolve_path(owner_module, path, NamespaceSet::VALUES)?
            .resolved;
        if self.semantic_items_include_const_like(owner_defs)? {
            return Ok(true);
        }

        let fallback_module = self.body.body().fallback_module();
        if fallback_module == owner_module {
            return Ok(false);
        }

        let fallback_defs = def_maps
            .name_resolver()
            .resolve_path(fallback_module, path, NamespaceSet::VALUES)?
            .resolved;
        self.semantic_items_include_const_like(fallback_defs)
    }

    fn semantic_items_include_const_like(&self, defs: Vec<DefId>) -> Result<bool, ReadError> {
        let item_query = self.context.item_query();
        for def in defs {
            let DefId::Local(local_def) = def else {
                continue;
            };
            let Some(item) = item_query.semantic_item_for_local_def(local_def)? else {
                continue;
            };
            if Self::semantic_item_is_const_like_pattern(item) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn semantic_item_is_const_like_pattern(item: SemanticItemRef) -> bool {
        matches!(item, SemanticItemRef::Const(_) | SemanticItemRef::Static(_))
    }

    fn previous_active_binding_visible(
        &self,
        start_scope: ScopeId,
        binding: BindingId,
        name: &str,
        active: &[bool],
    ) -> bool {
        let mut scope = Some(start_scope);
        while let Some(scope_id) = scope {
            let Some(scope_data) = self.body.body().scope(scope_id) else {
                return false;
            };

            for candidate in scope_data.bindings.iter().rev() {
                if candidate.0 >= binding.0 {
                    continue;
                }
                if !active.get(candidate.0).copied().unwrap_or(false) {
                    continue;
                }

                let Some(candidate_data) = self.body.body().binding(*candidate) else {
                    continue;
                };
                if candidate_data.name.as_deref() == Some(name) {
                    return true;
                }
            }

            scope = scope_data.parent;
        }

        false
    }

    fn declaration_is_const_like_pattern(declaration: &DeclarationRef) -> bool {
        matches!(
            declaration,
            DeclarationRef::Item(SemanticItemRef::Const(_) | SemanticItemRef::Static(_))
                | DeclarationRef::EnumVariant(_)
        )
    }
}
