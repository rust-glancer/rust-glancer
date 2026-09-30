//! Goto-implementation query flow.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, FileId};
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
        // A dot call supplies a receiver that can narrow the possible implementations. Keep
        // all matching methods here; unlike definition navigation, this query need not prove
        // one complete trait application. Associated calls continue through their declarations.
        if let Some(call) = source_symbols
            .call_for_symbol(&symbol)
            .context("resolve implementation call context")?
            && let Some(declarations) = implementations
                .method_call_implementations(&call)
                .context("find method call implementations")?
        {
            return NavigationTargetProjection::new(self.0.view_db())
                .targets_for_declarations(declarations);
        }

        let mut declarations = UniqueVec::new();
        for declaration in source_symbols.declarations_for_symbol(symbol.clone())? {
            declarations
                .extend(implementations.implementations_for_declaration(crate_ref, declaration)?);
        }

        if declarations.is_empty()
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
        {
            declarations.extend(implementations.implementations_for_ty(crate_ref, &ty)?);
        }

        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }
}
