//! Body-local macro expansion state used while lowering syntax into Body IR.
//!
//! Body lowering is otherwise a mechanical syntax-to-IR pass. This module is the narrow adapter
//! that lets it ask def-map for macro expansion while keeping the expansion cache, parse package,
//! and recursion guard outside the lowering context itself.

use std::{cell::Cell, rc::Rc};

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{
    BodyMacroCallOrigin, BodyMacroExpander as DefMapBodyMacroExpander, BodyMacroExprExpansion,
    DefMapReadTxn, ExpandedBodyMacro,
};
use rg_ir_model::{ModuleRef, TargetRef};
use rg_syntax::ast;

use crate::ir::BodySource;

const BODY_MACRO_EXPANSION_DEPTH_LIMIT: usize = 64;

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
    /// `format_args!("hi")` can lower to a builtin expression after normal macro lookup fails. If
    /// neither path succeeds, expression lowering falls back to the original macro expression.
    fn expand_expr_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroExprExpansion>>;

    /// Expand a macro call as statement-list syntax, leaving lowering to splice the result.
    ///
    /// Example: `make_stmts!(input);` asks for `ast::MacroStmts`. Block lowering then splices the
    /// generated statements, and an empty expansion contributes no placeholder statement.
    fn expand_stmt_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::MacroStmts>>>;

    /// Expand a macro call as pattern syntax, leaving lowering to preserve binding semantics.
    ///
    /// Example: `let bind_pair!(left, right) = value;` asks for an `ast::Pat`. Pattern lowering
    /// then applies the same binding rules that a handwritten tuple or identifier pattern would.
    fn expand_pat_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Pat>>>;

    /// Expand a macro call as type syntax, leaving lowering to preserve the fallback type.
    ///
    /// Example: `let value: make_ty!() = input;` asks for an `ast::Type`. Type lowering then
    /// lowers the generated type under the original macro call source span.
    fn expand_type_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Type>>>;
}

/// RAII guard that keeps recursive macro expansion depth balanced across early returns.
pub(crate) struct BodyMacroExpansionScope {
    depth: Rc<Cell<usize>>,
}

impl Drop for BodyMacroExpansionScope {
    fn drop(&mut self) {
        let depth = self.depth.get();
        self.depth.set(
            depth
                .checked_sub(1)
                .expect("body macro expansion depth should be balanced"),
        );
    }
}

/// Keeps macro expansion cache and recursion policy out of the mechanical body lowering context.
pub(crate) struct BodyMacroExpansion<'ctx, 'db> {
    parse_package: &'ctx rg_parse::Package,
    expander: DefMapBodyMacroExpander<'db, 'ctx>,
    cfg: CfgEvaluator<'ctx>,
    depth: Rc<Cell<usize>>,
}

impl<'ctx, 'db> BodyMacroExpansion<'ctx, 'db> {
    pub(crate) fn new(
        parse_package: &'ctx rg_parse::Package,
        def_maps: &'ctx DefMapReadTxn<'db>,
        cfg: CfgEvaluator<'ctx>,
    ) -> Self {
        Self {
            parse_package,
            expander: DefMapBodyMacroExpander::new(def_maps),
            cfg,
            depth: Rc::new(Cell::new(0)),
        }
    }
}

impl BodyMacroExpansionContext for BodyMacroExpansion<'_, '_> {
    fn expansion_scope(&self) -> Option<BodyMacroExpansionScope> {
        let depth = self.depth.get();
        if depth >= BODY_MACRO_EXPANSION_DEPTH_LIMIT {
            return None;
        }
        self.depth.set(depth + 1);
        Some(BodyMacroExpansionScope {
            depth: Rc::clone(&self.depth),
        })
    }

    fn expand_expr_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroExprExpansion>> {
        self.expander.expand_expr_call(
            target,
            module,
            source,
            origin,
            self.parse_package,
            self.cfg,
            call,
        )
    }

    fn expand_stmt_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::MacroStmts>>> {
        self.expander.expand_stmt_call(
            target,
            module,
            source,
            origin,
            self.parse_package,
            self.cfg,
            call,
        )
    }

    fn expand_pat_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Pat>>> {
        self.expander
            .expand_pat_call(target, module, source, origin, self.parse_package, call)
    }

    fn expand_type_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        source: BodySource,
        origin: BodyMacroCallOrigin,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Type>>> {
        self.expander
            .expand_type_call(target, module, source, origin, self.parse_package, call)
    }
}
