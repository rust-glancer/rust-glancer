//! Body-local macro expansion state used while lowering syntax into Body IR.
//!
//! Body lowering is otherwise a mechanical syntax-to-IR pass. This module is the narrow adapter
//! that lets it ask def-map for macro expansion while keeping the expansion cache, parse package,
//! and recursion guard outside the lowering context itself.

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{
    BodyMacroCallOrigin, BodyMacroCallSite, BodyMacroExpander as DefMapBodyMacroExpander,
    BodyMacroExpansionScope, BodyMacroExprExpansionOutcome, BodyMacroPatExpansionOutcome,
    BodyMacroStmtExpansionOutcome, BodyMacroTypeExpansionOutcome, DefMapReadTxn,
};
use rg_ir_model::ModuleRef;
use rg_std::CancellationToken;
use rg_syntax::ast;

use crate::body::BodySource;

pub(crate) trait BodyMacroExpansionContext {
    /// Enter one nested expansion step, returning `None` when the recursion cap is reached.
    ///
    /// Example: lowering `recurse!()` enters a scope before expanding the call. If the expansion
    /// produces another `recurse!()`, nested lowering asks for another scope until the cap is hit
    /// and the caller keeps the original macro as an unknown expression or statement.
    fn expansion_scope(&self) -> Option<BodyMacroExpansionScope>;

    /// Expand an expression macro call or classify a known compiler builtin.
    ///
    /// Example: `let value = make_expr!(input);` asks for generated `ast::Expr` syntax, while
    /// `format_args!("hi")` can lower to a builtin expression when macro lookup identifies a
    /// compiler builtin. If neither path succeeds, expression lowering falls back to the original
    /// macro expression.
    fn expand_expr_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroExprExpansionOutcome>>;

    /// Expand a macro call as statement-list syntax, leaving lowering to splice the result.
    ///
    /// Example: `make_stmts!(input);` asks for `ast::MacroStmts`. Block lowering then splices the
    /// generated statements, and an empty expansion contributes no placeholder statement.
    fn expand_stmt_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroStmtExpansionOutcome>>;

    /// Expand a macro call as pattern syntax, leaving lowering to preserve binding semantics.
    ///
    /// Example: `let bind_pair!(left, right) = value;` asks for an `ast::Pat`. Pattern lowering
    /// then applies the same binding rules that a handwritten tuple or identifier pattern would.
    fn expand_pat_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroPatExpansionOutcome>>;

    /// Expand a macro call as type syntax, leaving lowering to preserve the fallback type.
    ///
    /// Example: `let value: make_ty!() = input;` asks for an `ast::Type`. Type lowering then
    /// lowers the generated type under the original macro call source span.
    fn expand_type_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroTypeExpansionOutcome>>;
}

/// Keeps macro expansion cache and recursion policy out of the mechanical body lowering context.
pub(crate) struct BodyMacroExpansion<'ctx, 'db> {
    parse_package: &'ctx rg_parse::Package,
    expander: DefMapBodyMacroExpander<'db, 'ctx>,
    cfg: CfgEvaluator<'ctx>,
}

impl<'ctx, 'db> BodyMacroExpansion<'ctx, 'db> {
    pub(crate) fn new(
        parse_package: &'ctx rg_parse::Package,
        def_maps: &'ctx DefMapReadTxn<'db>,
        cfg: CfgEvaluator<'ctx>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            parse_package,
            expander: DefMapBodyMacroExpander::new(def_maps, cancellation),
            cfg,
        }
    }

    fn call_site(
        &self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
    ) -> BodyMacroCallSite<'ctx> {
        BodyMacroCallSite::new(module, source, origin, self.parse_package.edition())
    }
}

impl BodyMacroExpansionContext for BodyMacroExpansion<'_, '_> {
    fn expansion_scope(&self) -> Option<BodyMacroExpansionScope> {
        self.expander.expansion_scope()
    }

    fn expand_expr_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroExprExpansionOutcome>> {
        let site = self.call_site(module, source, origin).with_cfg(self.cfg);
        self.expander.expand_expr_call(site, call)
    }

    fn expand_stmt_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroStmtExpansionOutcome>> {
        let site = self.call_site(module, source, origin).with_cfg(self.cfg);
        self.expander.expand_stmt_call(site, call)
    }

    fn expand_pat_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroPatExpansionOutcome>> {
        let site = self.call_site(module, source, origin);
        self.expander.expand_pat_call(site, call)
    }

    fn expand_type_call(
        &mut self,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroTypeExpansionOutcome>> {
        let site = self.call_site(module, source, origin);
        self.expander.expand_type_call(site, call)
    }
}
