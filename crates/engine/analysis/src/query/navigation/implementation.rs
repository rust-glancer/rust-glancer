//! Goto-implementation query flow.

use rg_ir_model::TargetRef;
use rg_ir_view::implementation::ImplementationView;
use rg_parse::FileId;
use rg_std::UniqueVec;

use super::target::NavigationTargetProjection;
use crate::{
    Analysis,
    model::{NavigationTarget, SymbolAt},
    source_symbol::SourceSymbolResolver,
};

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
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let Some(symbol) = self.0.symbol_at_for_query(target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        let implementations = ImplementationView::new(self.0.view_db());
        if let SymbolAt::Expr { expr } = &symbol
            && let Some(declarations) = implementations.method_call_implementations(*expr)?
        {
            return NavigationTargetProjection::new(self.0.view_db())
                .targets_for_declarations(declarations);
        }

        let mut declarations = UniqueVec::new();
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        for declaration in source_symbols.declarations_for_symbol(symbol.clone())? {
            declarations
                .extend(implementations.implementations_for_declaration(target, declaration)?);
        }

        if declarations.is_empty()
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
        {
            declarations.extend(implementations.implementations_for_ty(target, &ty)?);
        }

        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }
}
