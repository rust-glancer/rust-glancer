use rg_cfg_eval::CfgPredicate;
use rg_parse::FileId;
use rg_syntax::{
    AstNode as _, TextRange,
    ast::{self, HasAttrs as _},
};
use rg_text::{Name, NameInterner};
use rg_tt::{
    Edition, Span, TopSubtree,
    syntax_bridge::{SpanFactory, syntax_node_to_token_tree_with_span},
};
use rg_workspace::RustEdition;

use super::normalized_syntax;

mod builtin;

pub use self::builtin::{BuiltinMacroItem, CfgSelectArmItem, CfgSelectArmPayload};

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum MacroDefinitionItem {
    MacroRules {
        attrs: MacroDefinitionAttrs,
        body: Option<TopSubtree>,
    },
    MacroDef {
        args: Option<TopSubtree>,
        body: Option<TopSubtree>,
    },
}

impl MacroDefinitionItem {
    pub fn from_macro_rules(item: &ast::MacroRules, file_id: FileId, edition: RustEdition) -> Self {
        let span_factory = SpanFactory::new(file_id_u32(file_id), macro_edition(edition));
        let mut span_for_range = |range| span_factory.span_for(range);
        Self::from_macro_rules_with_span(item, &mut span_for_range)
    }

    pub fn from_macro_rules_with_span(
        item: &ast::MacroRules,
        span_for_range: &mut dyn FnMut(TextRange) -> Span,
    ) -> Self {
        Self::MacroRules {
            attrs: MacroDefinitionAttrs::from_macro_rules(item),
            body: item
                .token_tree()
                .map(|token_tree| syntax_node_to_token_tree_with_span(&token_tree, span_for_range)),
        }
    }

    pub fn from_macro_def(item: &ast::MacroDef, file_id: FileId, edition: RustEdition) -> Self {
        let span_factory = SpanFactory::new(file_id_u32(file_id), macro_edition(edition));
        let mut span_for_range = |range| span_factory.span_for(range);
        Self::from_macro_def_with_span(item, &mut span_for_range)
    }

    pub fn from_macro_def_with_span(
        item: &ast::MacroDef,
        span_for_range: &mut dyn FnMut(TextRange) -> Span,
    ) -> Self {
        Self::MacroDef {
            args: item
                .args()
                .map(|token_tree| syntax_node_to_token_tree_with_span(&token_tree, span_for_range)),
            body: item
                .body()
                .map(|token_tree| syntax_node_to_token_tree_with_span(&token_tree, span_for_range)),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        match self {
            Self::MacroRules { attrs, .. } => attrs.shrink_to_fit(),
            Self::MacroDef { .. } => {}
        }
    }
}

/// Macro-specific attributes that affect def-map visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroDefinitionAttrs {
    pub macro_export: bool,
    pub cfg_attr_macro_export: Vec<CfgPredicate>,
}

impl MacroDefinitionAttrs {
    fn from_macro_rules(item: &ast::MacroRules) -> Self {
        let mut attrs = Self::default();

        for attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            let Some(meta) = attr.meta() else {
                continue;
            };
            attrs.collect_macro_export_meta(meta, None);
        }

        attrs
    }

    fn collect_macro_export_meta(&mut self, meta: ast::Meta, predicate: Option<CfgPredicate>) {
        if meta.simple_name().as_deref() == Some("macro_export") {
            match predicate {
                Some(predicate) => self.cfg_attr_macro_export.push(predicate),
                None => self.macro_export = true,
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
                    self.collect_macro_export_meta(nested, Some(predicate.clone()));
                }
            }
            ast::Meta::UnsafeMeta(meta) => {
                if let Some(meta) = meta.meta() {
                    self.collect_macro_export_meta(meta, predicate);
                }
            }
            ast::Meta::CfgMeta(_)
            | ast::Meta::PathMeta(_)
            | ast::Meta::TokenTreeMeta(_)
            | ast::Meta::KeyValueMeta(_) => {}
        }
    }

    fn shrink_to_fit(&mut self) {
        self.cfg_attr_macro_export.shrink_to_fit();
        for predicate in &mut self.cfg_attr_macro_export {
            predicate.shrink_to_fit();
        }
    }
}

/// Legacy `#[macro_use]` import selector.
#[derive(Debug, Clone, Default, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroUseAttr {
    pub direct: Option<MacroUseSelector>,
    pub cfg_attr_macro_use: Vec<CfgAttrMacroUse>,
}

impl MacroUseAttr {
    pub fn from_attrs(item: &impl ast::HasAttrs, interner: &mut NameInterner) -> Option<Self> {
        let mut attr = Self::default();

        for source_attr in item.attrs().filter(|attr| attr.kind().is_outer()) {
            let Some(meta) = source_attr.meta() else {
                continue;
            };
            attr.collect_macro_use_meta(meta, None, interner);
        }

        (attr.direct.is_some() || !attr.cfg_attr_macro_use.is_empty()).then_some(attr)
    }

    fn collect_macro_use_meta(
        &mut self,
        meta: ast::Meta,
        predicate: Option<CfgPredicate>,
        interner: &mut NameInterner,
    ) {
        if let Some(selector) = MacroUseSelector::from_meta(&meta, interner) {
            match predicate {
                Some(predicate) => self.cfg_attr_macro_use.push(CfgAttrMacroUse {
                    predicate,
                    selector,
                }),
                None => match &mut self.direct {
                    Some(direct) => direct.merge(&selector),
                    None => self.direct = Some(selector),
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
                    self.collect_macro_use_meta(nested, Some(predicate.clone()), interner);
                }
            }
            ast::Meta::UnsafeMeta(meta) => {
                if let Some(meta) = meta.meta() {
                    self.collect_macro_use_meta(meta, predicate, interner);
                }
            }
            ast::Meta::CfgMeta(_)
            | ast::Meta::PathMeta(_)
            | ast::Meta::TokenTreeMeta(_)
            | ast::Meta::KeyValueMeta(_) => {}
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(direct) = &mut self.direct {
            direct.shrink_to_fit();
        }
        self.cfg_attr_macro_use.shrink_to_fit();
        for cfg_attr in &mut self.cfg_attr_macro_use {
            cfg_attr.shrink_to_fit();
        }
    }
}

/// Macro-use selector once cfg_attr gates have been evaluated for one target.
#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroUseSelector {
    /// `None` means all exported macros; `Some` keeps the explicit `#[macro_use(foo, bar)]` list.
    pub names: Option<Vec<Name>>,
}

impl MacroUseSelector {
    fn from_meta(meta: &ast::Meta, interner: &mut NameInterner) -> Option<Self> {
        if meta.as_simple_atom().as_deref() == Some("macro_use") {
            return Some(Self { names: None });
        }

        if let Some((name, token_tree)) = meta.as_simple_call()
            && name.as_str() == "macro_use"
        {
            return Some(Self {
                names: Some(Self::names_from_token_tree(&token_tree, interner)),
            });
        }

        None
    }

    pub fn allows(&self, name: &Name) -> bool {
        match &self.names {
            Some(names) => names.iter().any(|allowed| allowed == name),
            None => true,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        let (Some(names), Some(other_names)) = (&mut self.names, &other.names) else {
            self.names = None;
            return;
        };

        for name in other_names {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.clone());
            }
        }
    }

    fn shrink_to_fit(&mut self) {
        if let Some(names) = &mut self.names {
            names.shrink_to_fit();
            for name in names {
                name.shrink_to_fit();
            }
        }
    }

    pub(crate) fn names_from_token_tree(
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
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct CfgAttrMacroUse {
    pub predicate: CfgPredicate,
    pub selector: MacroUseSelector,
}

impl CfgAttrMacroUse {
    fn shrink_to_fit(&mut self) {
        self.predicate.shrink_to_fit();
        self.selector.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroCallItem {
    pub path: Option<String>,
    pub callee: Option<Name>,
    pub args: Option<TopSubtree>,
    pub builtin: Option<BuiltinMacroItem>,
}

impl MacroCallItem {
    /// Builds an item-position macro call payload.
    ///
    /// Builtin macro probes happen before this point because they need package/file context.
    /// `builtin` carries the already discovered source-like payload when available.
    pub fn from_ast(
        item: &ast::MacroCall,
        file_id: FileId,
        edition: RustEdition,
        builtin: Option<BuiltinMacroItem>,
        interner: &mut NameInterner,
    ) -> Self {
        let span_factory = SpanFactory::new(file_id_u32(file_id), macro_edition(edition));
        let mut span_for_range = |range| span_factory.span_for(range);
        Self::from_ast_with_span(item, interner, builtin, &mut span_for_range)
    }

    pub fn from_ast_with_span(
        item: &ast::MacroCall,
        interner: &mut NameInterner,
        builtin: Option<BuiltinMacroItem>,
        span_for_range: &mut dyn FnMut(TextRange) -> Span,
    ) -> Self {
        Self {
            path: item.path().map(|path| normalized_syntax(&path)),
            callee: item
                .path()
                .and_then(|path| path.segment())
                .and_then(|segment| segment.name_ref())
                .map(|name_ref| interner.intern(name_ref.text())),
            args: item
                .token_tree()
                .map(|token_tree| syntax_node_to_token_tree_with_span(&token_tree, span_for_range)),
            builtin,
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(path) = &mut self.path {
            path.shrink_to_fit();
        }
        if let Some(callee) = &mut self.callee {
            callee.shrink_to_fit();
        }
        if let Some(builtin) = &mut self.builtin {
            builtin.shrink_to_fit();
        }
    }
}

fn file_id_u32(file_id: FileId) -> u32 {
    u32::try_from(file_id.0).expect("file id should fit macro span storage")
}

fn macro_edition(edition: RustEdition) -> Edition {
    match edition {
        RustEdition::Edition2015 => Edition::Edition2015,
        RustEdition::Edition2018 => Edition::Edition2018,
        RustEdition::Edition2021 => Edition::Edition2021,
        RustEdition::Edition2024 => Edition::Edition2024,
    }
}
