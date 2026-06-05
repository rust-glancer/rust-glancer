use rg_cfg_eval::CfgPredicate;
use rg_ir_model::items::{
    BuiltinMacroItem, CfgAttrMacroUse, MacroCallItem, MacroDefinitionAttrs, MacroDefinitionItem,
    MacroUseAttr, MacroUseSelector,
};
use rg_syntax::{
    AstNode as _, TextRange,
    ast::{self, HasAttrs as _},
};
use rg_text::{Name, NameInterner};
use rg_tt::{Span, syntax_bridge::syntax_node_to_token_tree_with_span};

use super::{FromAst, MaybeFromAst, normalized_syntax};

pub struct MacroRulesAst;
pub struct MacroDefAst;

pub struct MacroRulesContext<'a> {
    pub span_for_range: &'a mut dyn FnMut(TextRange) -> Span,
}

pub struct MacroDefContext<'a> {
    pub span_for_range: &'a mut dyn FnMut(TextRange) -> Span,
}

pub struct MacroCallContext<'a> {
    pub interner: &'a mut NameInterner,
    pub builtin: Option<BuiltinMacroItem>,
    pub span_for_range: &'a mut dyn FnMut(TextRange) -> Span,
}

impl FromAst<MacroRulesAst> for MacroDefinitionItem {
    type AstNode = ast::MacroRules;
    type Context<'a> = MacroRulesContext<'a>;

    fn from_ast(item: &Self::AstNode, ctx: Self::Context<'_>) -> Self {
        Self::MacroRules {
            attrs: macro_definition_attrs_from_macro_rules(item),
            body: item.token_tree().map(|token_tree| {
                syntax_node_to_token_tree_with_span(&token_tree, ctx.span_for_range)
            }),
        }
    }
}

impl FromAst<MacroDefAst> for MacroDefinitionItem {
    type AstNode = ast::MacroDef;
    type Context<'a> = MacroDefContext<'a>;

    fn from_ast(item: &Self::AstNode, ctx: Self::Context<'_>) -> Self {
        Self::MacroDef {
            args: item.args().map(|token_tree| {
                syntax_node_to_token_tree_with_span(&token_tree, ctx.span_for_range)
            }),
            body: item.body().map(|token_tree| {
                syntax_node_to_token_tree_with_span(&token_tree, ctx.span_for_range)
            }),
        }
    }
}

impl MaybeFromAst for MacroUseAttr {
    type AstNode = dyn ast::HasAttrs;
    type Context<'a> = &'a mut NameInterner;

    fn maybe_from_ast(item: &Self::AstNode, interner: Self::Context<'_>) -> Option<Self> {
        let mut attr = Self::default();

        for source_attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            let Some(meta) = source_attr.meta() else {
                continue;
            };
            collect_macro_use_meta(&mut attr, meta, None, interner);
        }

        (attr.direct.is_some() || !attr.cfg_attr_macro_use.is_empty()).then_some(attr)
    }
}

impl FromAst for MacroCallItem {
    type AstNode = ast::MacroCall;
    type Context<'a> = MacroCallContext<'a>;

    fn from_ast(item: &Self::AstNode, ctx: Self::Context<'_>) -> Self {
        Self {
            path: item.path().map(|path| normalized_syntax(&path)),
            callee: item
                .path()
                .and_then(|path| path.segment())
                .and_then(|segment| segment.name_ref())
                .map(|name_ref| ctx.interner.intern(name_ref.text())),
            args: item.token_tree().map(|token_tree| {
                syntax_node_to_token_tree_with_span(&token_tree, ctx.span_for_range)
            }),
            builtin: ctx.builtin,
        }
    }
}

fn macro_definition_attrs_from_macro_rules(item: &ast::MacroRules) -> MacroDefinitionAttrs {
    let mut attrs = MacroDefinitionAttrs::default();

    for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
        let Some(meta) = attr.meta() else {
            continue;
        };
        collect_macro_export_meta(&mut attrs, meta, None);
    }

    attrs
}

fn collect_macro_export_meta(
    attrs: &mut MacroDefinitionAttrs,
    meta: ast::Meta,
    predicate: Option<CfgPredicate>,
) {
    if meta.simple_name().as_deref() == Some("macro_export") {
        match predicate {
            Some(predicate) => attrs.cfg_attr_macro_export.push(predicate),
            None => attrs.macro_export = true,
        }
        return;
    }

    match meta {
        ast::Meta::CfgAttrMeta(cfg_attr) => {
            let cfg_attr_predicate = cfg_attr
                .cfg_predicate()
                .map(CfgPredicate::from_ast)
                .unwrap_or(CfgPredicate::Invalid);
            let predicate = match predicate {
                Some(predicate) => CfgPredicate::All(vec![predicate, cfg_attr_predicate]),
                None => cfg_attr_predicate,
            };
            for nested in cfg_attr.metas() {
                collect_macro_export_meta(attrs, nested, Some(predicate.clone()));
            }
        }
        ast::Meta::UnsafeMeta(meta) => {
            if let Some(meta) = meta.meta() {
                collect_macro_export_meta(attrs, meta, predicate);
            }
        }
        ast::Meta::CfgMeta(_)
        | ast::Meta::PathMeta(_)
        | ast::Meta::TokenTreeMeta(_)
        | ast::Meta::KeyValueMeta(_) => {}
    }
}

fn collect_macro_use_meta(
    attr: &mut MacroUseAttr,
    meta: ast::Meta,
    predicate: Option<CfgPredicate>,
    interner: &mut NameInterner,
) {
    if let Some(selector) = macro_use_selector_from_meta(&meta, interner) {
        match predicate {
            Some(predicate) => attr.cfg_attr_macro_use.push(CfgAttrMacroUse {
                predicate,
                selector,
            }),
            None => match &mut attr.direct {
                Some(direct) => direct.merge(&selector),
                None => attr.direct = Some(selector),
            },
        }
        return;
    }

    match meta {
        ast::Meta::CfgAttrMeta(cfg_attr) => {
            let cfg_attr_predicate = cfg_attr
                .cfg_predicate()
                .map(CfgPredicate::from_ast)
                .unwrap_or(CfgPredicate::Invalid);
            let predicate = match predicate {
                Some(predicate) => CfgPredicate::All(vec![predicate, cfg_attr_predicate]),
                None => cfg_attr_predicate,
            };
            for nested in cfg_attr.metas() {
                collect_macro_use_meta(attr, nested, Some(predicate.clone()), interner);
            }
        }
        ast::Meta::UnsafeMeta(meta) => {
            if let Some(meta) = meta.meta() {
                collect_macro_use_meta(attr, meta, predicate, interner);
            }
        }
        ast::Meta::CfgMeta(_)
        | ast::Meta::PathMeta(_)
        | ast::Meta::TokenTreeMeta(_)
        | ast::Meta::KeyValueMeta(_) => {}
    }
}

fn macro_use_selector_from_meta(
    meta: &ast::Meta,
    interner: &mut NameInterner,
) -> Option<MacroUseSelector> {
    if meta.as_simple_atom().as_deref() == Some("macro_use") {
        return Some(MacroUseSelector { names: None });
    }

    if let Some((name, token_tree)) = meta.as_simple_call()
        && name.as_str() == "macro_use"
    {
        return Some(MacroUseSelector {
            names: Some(macro_use_names_from_token_tree(&token_tree, interner)),
        });
    }

    None
}

fn macro_use_names_from_token_tree(
    token_tree: &ast::TokenTree,
    interner: &mut NameInterner,
) -> Vec<Name> {
    let text = token_tree.syntax().text().to_string();
    let text = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(&text);

    text.split(',')
        .filter_map(|name| {
            let name = name.trim();
            (!name.is_empty()).then(|| interner.intern(name))
        })
        .collect()
}
