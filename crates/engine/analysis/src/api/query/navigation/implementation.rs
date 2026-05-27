//! Goto-implementation query flow.

use rg_ir_model::{TargetRef, identity::DeclarationRef};
use rg_ir_view::implementation::ImplementationView;
use rg_parse::FileId;

use super::target::NavigationTargetProjection;
use crate::{
    api::{Analysis, source_symbol::SourceSymbolResolver},
    model::{NavigationTarget, SymbolAt},
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
            && let Some(declarations) =
                implementations.implementations_for_method_call_expr(*expr)?
        {
            return NavigationTargetProjection::new(self.0.view_db())
                .targets_for_declarations(declarations);
        }

        let mut declarations = Vec::new();
        let source_symbols = SourceSymbolResolver::new(self.0.view_db());
        for declaration in source_symbols.declarations_for_symbol(symbol.clone())? {
            Self::extend_unique_declarations(
                &mut declarations,
                implementations.implementations_for_declaration(declaration)?,
            );
        }

        if declarations.is_empty()
            && let Some(ty) = source_symbols.ty_for_symbol(symbol)?
        {
            Self::extend_unique_declarations(
                &mut declarations,
                implementations.implementations_for_ty(&ty)?,
            );
        }

        NavigationTargetProjection::new(self.0.view_db()).targets_for_declarations(declarations)
    }

    fn extend_unique_declarations(
        declarations: &mut Vec<DeclarationRef>,
        new_declarations: Vec<DeclarationRef>,
    ) {
        for declaration in new_declarations {
            if !declarations.contains(&declaration) {
                declarations.push(declaration);
            }
        }
    }
}
