//! Goto-implementation query flow.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId, identity::DeclarationRef};
use rg_ir_view::implementation::ImplementationView;
use rg_std::UniqueVec;

use super::target::NavigationTargetProjection;
use crate::{Analysis, model::NavigationTarget, source_symbol::SourceSymbolResolver};

/// Implements goto-implementation with the facts rust-glancer already collects.
///
/// The query deliberately returns concrete source declarations only: impl blocks for types/traits
/// and concrete methods for trait-method declarations or calls. It avoids inventing targets for
/// default trait items because those are declarations, not user-written implementations.
pub(crate) struct ImplementationResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> ImplementationResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn goto_implementation(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(crate_ref, file_id, offset)? else {
            return Ok(Vec::new());
        };

        let implementations = ImplementationView::new(self.0.view_db());
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        // Keep the use site's body even when the selected trait lives at module level.
        // Its local impls also matter for a trait path in an impl header or a standalone method
        // path, so obtaining this context must not depend on finding an enclosing call.
        let body_ref = symbol.body_ref();

        // Final call facts identify the trait chosen by inference. Use that identity, but not
        // its Self or generic arguments: all impls of this trait can implement the method.
        // Declarations and standalone method paths join the same expansion below.
        let call = source_symbols
            .call_for_symbol(&symbol)
            .context("resolve implementation call context")?;
        let selected = match call.as_ref().and_then(|call| call.facts()) {
            Some(facts) => vec![DeclarationRef::from(facts.function())],
            None => source_symbols.declarations_for_symbol(symbol.clone())?,
        };
        let mut declarations = UniqueVec::new();
        let mut handled = false;
        for declaration in selected {
            if let Some(targets) =
                implementations.implementations_for_declaration(crate_ref, body_ref, declaration)?
            {
                handled = true;
                declarations.extend(targets);
            }
        }

        // An empty method answer is complete. Only symbols without declaration-based
        // implementation navigation may fall back to impls of their inferred type.
        if !handled && let Some(ty) = source_symbols.ty_for_symbol(symbol)? {
            declarations.extend(implementations.implementations_for_ty(crate_ref, &ty)?);
        }

        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }
}
