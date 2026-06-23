//! Body-facing declarative macro expansion.
//!
//! Body lowering needs expansion as an input to syntax lowering, but it should not know about the
//! token-tree and macro-engine crates directly. This facade keeps resolution and token conversion
//! next to the def-map-owned macro expansion cache.

use anyhow::Context as _;

use rg_ir_model::{DefMapRef, ModuleId, ModuleRef, TargetRef};
use rg_ir_storage::{
    DefMapQuery, ImportPath, MacroDefinitionView, PathResolver, ScopeResolutionEnv,
};
use rg_macro_expand::{ExpansionParseKind, ExpansionSyntax};
use rg_parse::{FileId, Span};
use rg_std::ExpectedUnique;
use rg_syntax::{AstNode as _, ast, utils::normalized_syntax_text};
use rg_text::Name;
use rg_tt::syntax_bridge::{SpanFactory, syntax_node_to_token_tree_with_span};

use crate::DefMapReadTxn;

use super::{MacroExpansionCache, PreparedMacroExpansion, macro_edition, tt_span_for_parse_span};

/// Expands declarative macros for Body IR lowering using frozen def-map visibility.
pub struct BodyMacroExpander<'db, 'txn> {
    def_maps: &'txn DefMapReadTxn<'db>,
    cache: MacroExpansionCache,
}

impl<'db, 'txn> BodyMacroExpander<'db, 'txn> {
    pub fn new(def_maps: &'txn DefMapReadTxn<'db>) -> Self {
        Self {
            def_maps,
            cache: MacroExpansionCache::default(),
        }
    }

    /// Expands one expression-position macro call to generated expression syntax.
    pub fn expand_expr_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ast::Expr>> {
        let Some(syntax) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Expr,
        )?
        else {
            return Ok(None);
        };

        let root = syntax.parse.syntax_node();
        Ok(ast::Expr::cast(root.clone()).or_else(|| root.children().find_map(ast::Expr::cast)))
    }

    /// Expands one statement-position macro call to generated statement-list syntax.
    pub fn expand_stmt_call(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
    ) -> anyhow::Result<Option<ast::MacroStmts>> {
        let Some(syntax) = self.expand_call_syntax(
            target,
            module,
            file_id,
            span,
            parse_package,
            call,
            ExpansionParseKind::Statements,
        )?
        else {
            return Ok(None);
        };

        let root = syntax.parse.syntax_node();
        Ok(ast::MacroStmts::cast(root.clone())
            .or_else(|| root.children().find_map(ast::MacroStmts::cast)))
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_call_syntax(
        &mut self,
        target: TargetRef,
        module: ModuleRef,
        file_id: FileId,
        span: Span,
        parse_package: &rg_parse::Package,
        call: &ast::MacroCall,
        parse_kind: ExpansionParseKind,
    ) -> anyhow::Result<Option<ExpansionSyntax>> {
        let Some(path_text) = call.path().map(|path| normalized_syntax_text(&path)) else {
            return Ok(None);
        };
        let Some(args) = call.token_tree() else {
            return Ok(None);
        };
        // Note: generated body syntax can contain `$crate` paths, but parsing them requires the
        // macro definition target. Body expressions are plain AST nodes at this boundary, so reject
        // those paths until body expansions carry the same origin tracking as generated items.
        let Some(path) = ImportPath::from_macro_path_text(&path_text, None) else {
            return Ok(None);
        };

        let query = DefMapQuery::new(self.def_maps);
        let Some(resolved) = Self::resolve_macro_definition(&query, target, module, &path)
            .context("while attempting to resolve body macro call")?
        else {
            return Ok(None);
        };

        let definition_edition = resolved.data.edition;
        let compile_result = self.cache.compile(
            resolved.def_ref,
            resolved.data,
            macro_edition(definition_edition),
        );
        let Some(macro_) = compile_result.macro_ else {
            return Ok(None);
        };

        let edition = macro_edition(parse_package.edition());
        let span_factory = SpanFactory::new(
            u32::try_from(file_id.0).expect("file id should fit macro span storage"),
            edition,
        );
        let call_site = tt_span_for_parse_span(file_id, span, edition);
        let args =
            syntax_node_to_token_tree_with_span(&args, &mut |range| span_factory.span_for(range));
        let prepared_expansion = self.cache.prepare_expansion(
            resolved.def_ref,
            macro_,
            &path_text,
            &args,
            call_site,
            parse_kind,
        );

        let syntax = match prepared_expansion.expansion {
            PreparedMacroExpansion::Syntax(syntax) => Some(syntax),
            PreparedMacroExpansion::Failed => None,
            PreparedMacroExpansion::Work(work) => {
                let result = work.expand_syntax();
                let syntax = result.generated_syntax;
                self.cache.insert_expansion(result.key, syntax.clone());
                syntax
            }
        };

        Ok(syntax)
    }

    fn resolve_macro_definition<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleRef,
        path: &ImportPath,
    ) -> anyhow::Result<Option<MacroDefinitionView<'a>>> {
        // Body expansion is target-local. Synthetic body modules resolve through their semantic
        // fallback before reaching this facade.
        let Some(module_target) = module.origin.as_target_ref() else {
            return Ok(None);
        };
        if module_target != target {
            return Ok(None);
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

        Ok(macros.into_option())
    }

    fn resolve_single_name_macro<'a>(
        query: &'a DefMapQuery<&DefMapReadTxn<'_>>,
        target: TargetRef,
        module: ModuleId,
        name: &Name,
    ) -> anyhow::Result<Option<MacroDefinitionView<'a>>> {
        let mut resolved = ExpectedUnique::new();
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
            if let Some(entry) = query
                .module_scope_entry(module_ref, name.as_str())
                .context("while attempting to inspect body macro scope entry")?
            {
                for binding in entry.macros() {
                    if let Some(macro_) = query
                        .macro_definition_view(binding.def)
                        .context("while attempting to fetch body macro definition")?
                    {
                        resolved.push(macro_);
                    }
                }
            }

            current = query
                .module_data(module_ref)
                .context("while attempting to fetch parent module for body macro lookup")?
                .and_then(|module| module.parent);
        }

        Ok(resolved.into_option())
    }
}
