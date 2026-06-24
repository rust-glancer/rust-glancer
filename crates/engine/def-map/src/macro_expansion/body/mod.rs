//! Body-facing declarative macro expansion.
//!
//! Body lowering needs expansion as an input to syntax lowering, but it should not know about the
//! token-tree and macro-engine crates directly. This facade keeps body-specific frozen def-map
//! visibility and token conversion next to the def-map query it relies on.

use anyhow::Context as _;

use rg_ir_model::{DefMapRef, ModuleId, ModuleRef, TargetRef, items::BuiltinMacroKind};
use rg_ir_storage::{DefMapQuery, ImportPath, MacroDefinitionView, PathResolver};
use rg_macro_runtime::{CfgSelect, ExpansionParseKind, ExpansionSyntax, MacroExpansionRuntime};
use rg_std::ExpectedUnique;
use rg_syntax::{Parse, SyntaxNode, ast};
use rg_text::Name;

use crate::DefMapReadTxn;

use super::builtin;

mod call;
mod expanded;

pub use self::call::{BodyMacroCallOrigin, BodyMacroCallSite};
use self::call::{BodyMacroCallee, BodyMacroInvocation, ResolvedBodyMacroCall};
pub use self::expanded::{BodyMacroExprExpansion, ExpandedBodyMacro};

/// Expands body macro calls using frozen def-map visibility.
///
/// Declarative macros return generated syntax. Compiler builtins are selected through ordinary
/// macro lookup when possible, with a path-shaped fallback for calls that do not resolve to sysroot
/// definitions.
pub struct BodyMacroExpander<'db, 'txn> {
    def_maps: &'txn DefMapReadTxn<'db>,
    runtime: MacroExpansionRuntime,
}

impl<'db, 'txn> BodyMacroExpander<'db, 'txn> {
    pub fn new(def_maps: &'txn DefMapReadTxn<'db>) -> Self {
        Self {
            def_maps,
            runtime: MacroExpansionRuntime::default(),
        }
    }

    /// Resolves one expression-position macro call to generated syntax or a builtin marker.
    ///
    /// e.g.:
    /// ```rust,ignore
    /// let value = make_expr!(input);
    /// let args = format_args!("value = {}", value);
    /// ```
    pub fn expand_expr_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<BodyMacroExprExpansion>> {
        let query = DefMapQuery::new(self.def_maps);
        let Some(call) = Self::resolve_body_macro_call(&query, site, call)? else {
            return Ok(None);
        };

        let expansion = match call.callee {
            BodyMacroCallee::Declarative(resolved) => self
                .expand_declarative_call(&call.invocation, resolved, ExpansionParseKind::Expr)
                .and_then(|expanded| expanded.cast_root_or_child::<ast::Expr>())
                .map(BodyMacroExprExpansion::Expanded),
            BodyMacroCallee::Builtin(BuiltinMacroKind::Expr(kind)) => {
                Some(BodyMacroExprExpansion::Builtin(kind))
            }
            BodyMacroCallee::Builtin(BuiltinMacroKind::CfgSelect) => {
                Self::expand_cfg_select(&call.invocation, site, ExpansionParseKind::Expr)
                    .and_then(|expanded| expanded.cast_root_or_child::<ast::Expr>())
                    .map(BodyMacroExprExpansion::Expanded)
            }
            BodyMacroCallee::Builtin(
                BuiltinMacroKind::Include
                | BuiltinMacroKind::IgnoredByDefMap
                | BuiltinMacroKind::Unsupported,
            ) => None,
        };

        Ok(expansion)
    }

    /// Expands one statement-position macro call to generated statement-list syntax.
    ///
    /// e.g.:
    /// ```rust,ignore
    /// fn update(input: Input) {
    ///     make_statements!(input);
    /// }
    /// ```
    pub fn expand_stmt_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::MacroStmts>>> {
        let Some(expanded) = self.expand_call_syntax(site, call, ExpansionParseKind::Statements)?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::MacroStmts>())
    }

    /// Expands one pattern-position macro call to generated pattern syntax.
    ///
    /// e.g.:
    /// ```rust,ignore
    /// let make_pattern!(left, right) = pair;
    /// ```
    pub fn expand_pat_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Pat>>> {
        let Some(expanded) = self.expand_call_syntax(site, call, ExpansionParseKind::Pattern)?
        else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::Pat>())
    }

    /// Expands one type-position macro call to generated type syntax.
    ///
    /// e.g.:
    /// ```rust,ignore
    /// let value: make_type!() = input;
    /// ```
    pub fn expand_type_call(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<ast::Type>>> {
        let Some(expanded) = self.expand_call_syntax(site, call, ExpansionParseKind::Type)? else {
            return Ok(None);
        };

        Ok(expanded.cast_root_or_child::<ast::Type>())
    }

    /// Unlike `expand_expr_call`, this does not classify expression builtins. It only expands
    /// declarative macros plus `cfg_select!` when a caller provides cfg context.
    fn expand_call_syntax(
        &mut self,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
        parse_kind: ExpansionParseKind,
    ) -> anyhow::Result<Option<ExpandedBodyMacro<Parse<SyntaxNode>>>> {
        let query = DefMapQuery::new(self.def_maps);
        let Some(call) = Self::resolve_body_macro_call(&query, site, call)? else {
            return Ok(None);
        };

        let expanded = match call.callee {
            BodyMacroCallee::Declarative(resolved) => {
                self.expand_declarative_call(&call.invocation, resolved, parse_kind)
            }
            BodyMacroCallee::Builtin(BuiltinMacroKind::CfgSelect) => {
                Self::expand_cfg_select(&call.invocation, site, parse_kind)
            }
            BodyMacroCallee::Builtin(
                BuiltinMacroKind::Expr(_)
                | BuiltinMacroKind::Include
                | BuiltinMacroKind::IgnoredByDefMap
                | BuiltinMacroKind::Unsupported,
            ) => None,
        };

        Ok(expanded)
    }

    fn resolve_body_macro_call<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        site: BodyMacroCallSite<'_>,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ResolvedBodyMacroCall<'a>>> {
        let invocation = match site.invocation(call) {
            Some(invocation) => invocation,
            None => return Ok(None),
        };
        // `$crate` in a generated macro call belongs to the macro definition that produced this
        // syntax. Source calls do not have such a definition context, so `$crate` remains invalid
        // for them here.
        // TODO: soft hack, we are not inside of macro resolution context here, so we use this
        // for the lack of better method; probably we should get rid of `ImportPath` whatsoever
        // (it exists for historical reasons mostly, and it's equivalent to `Path`) and introduce
        // appropriate constructors.
        let Some(path) = ImportPath::from_macro_path_text(
            invocation.path_text(),
            site.dollar_crate_target_for_path(),
        ) else {
            return Ok(None);
        };
        let resolved = Self::resolve_macro_definition(query, site.target(), site.module(), &path)
            .context("while attempting to resolve body macro call")?;

        let callee = match resolved {
            ExpectedUnique::One(resolved) => match resolved.data.builtin {
                Some(kind) => BodyMacroCallee::Builtin(kind),
                None => BodyMacroCallee::Declarative(resolved),
            },
            ExpectedUnique::Ambiguous => return Ok(None),
            ExpectedUnique::Empty => match builtin::kind_from_path(&path) {
                Some(kind) => BodyMacroCallee::Builtin(kind),
                None => return Ok(None),
            },
        };

        Ok(Some(ResolvedBodyMacroCall::new(invocation, callee)))
    }

    fn expand_cfg_select(
        invocation: &BodyMacroInvocation,
        site: BodyMacroCallSite<'_>,
        parse_kind: ExpansionParseKind,
    ) -> Option<ExpandedBodyMacro<Parse<SyntaxNode>>> {
        let cfg = site.cfg()?;
        let cfg_select = CfgSelect::parse(invocation.args())?;
        let arm = cfg_select
            .arms()
            .iter()
            .find(|arm| cfg.is_predicate_enabled(&arm.predicate))?;
        let ExpansionSyntax { parse, span_map } =
            ExpansionSyntax::from_token_tree(arm.payload.clone(), parse_kind);

        Some(ExpandedBodyMacro::new(
            parse,
            span_map,
            site.dollar_crate_target_for_expansion(),
        ))
    }

    fn expand_declarative_call(
        &mut self,
        invocation: &BodyMacroInvocation,
        resolved: MacroDefinitionView<'_>,
        parse_kind: ExpansionParseKind,
    ) -> Option<ExpandedBodyMacro<Parse<SyntaxNode>>> {
        let request = invocation.expansion_request(resolved.def_ref, resolved.data, parse_kind);
        let Some(ExpansionSyntax { parse, span_map }) = self.runtime.expand_now(request) else {
            return None;
        };

        Some(ExpandedBodyMacro::new(
            parse,
            span_map,
            resolved.data.dollar_crate_target,
        ))
    }

    fn resolve_macro_definition<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleRef,
        path: &ImportPath,
    ) -> anyhow::Result<ExpectedUnique<MacroDefinitionView<'a>>> {
        // Body expansion is target-local. Synthetic body modules resolve through their semantic
        // fallback before reaching this facade.
        let Some(module_target) = module.origin.as_target_ref() else {
            return Ok(ExpectedUnique::Empty);
        };
        if module_target != target {
            return Ok(ExpectedUnique::Empty);
        }

        if let Some(name) = path.relative_single_name() {
            return Self::resolve_single_name_macro(query, target, module.module, name);
        }

        let bindings = PathResolver::new(query)
            .macro_bindings(target, module.module, path)
            .context("while attempting to resolve qualified body macro path")?;
        let mut macros = ExpectedUnique::new();
        for binding in bindings {
            // Path resolution may return duplicate bindings to the same macro definition; collapse
            // those while still treating different definitions as ambiguous.
            if let Some(macro_) = query
                .macro_definition_view(binding.def)
                .context("while attempting to fetch body macro definition")?
            {
                macros.push(macro_);
            }
        }

        Ok(macros)
    }

    fn resolve_single_name_macro<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleId,
        name: &Name,
    ) -> anyhow::Result<ExpectedUnique<MacroDefinitionView<'a>>> {
        let importing_module = ModuleRef {
            origin: DefMapRef::Target(target),
            module,
        };
        let mut module_scope_modules = Vec::new();
        let mut current = Some(module);

        // Note: Body macro expansion intentionally uses the frozen module graph as an approximation
        // of item-position macro visibility. We do not model body-specific textual ordering here:
        // doing so would require cross-body statement order and nested body macro scope tracking,
        // while real projects overwhelmingly use macros that are already module-visible.
        while let Some(module) = current {
            let module_ref = ModuleRef {
                origin: DefMapRef::Target(target),
                module,
            };
            module_scope_modules.push(module_ref);

            current = query
                .module_data(module_ref)
                .context("while attempting to fetch parent module for body macro lookup")?
                .and_then(|module| module.parent);
        }

        let bindings = PathResolver::new(query)
            .visible_unqualified_macro_bindings(importing_module, module_scope_modules, name)
            .context("while attempting to resolve unqualified body macro")?;

        let module_scope = Self::resolve_macro_bindings(query, bindings.module_scope)?;
        if !module_scope.is_empty() {
            return Ok(module_scope);
        }

        Self::resolve_macro_bindings(query, bindings.standard_prelude)
    }

    fn resolve_macro_bindings<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        bindings: Vec<rg_ir_storage::ScopeBinding>,
    ) -> anyhow::Result<ExpectedUnique<MacroDefinitionView<'a>>> {
        let mut resolved = ExpectedUnique::new();

        for binding in bindings {
            if let Some(macro_) = query
                .macro_definition_view(binding.def)
                .context("while attempting to fetch body macro definition")?
            {
                resolved.push(macro_);
            }
        }

        Ok(resolved)
    }
}
