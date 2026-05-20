use rg_parse::FileId;
use rg_syntax::{
    TextRange,
    ast::{self, HasAttrs as _},
};
use rg_text::{Name, NameInterner};
use rg_tt::{
    Edition, Span, TopSubtree,
    syntax_bridge::{SpanFactory, syntax_node_to_token_tree_with_span},
};
use rg_workspace::RustEdition;

use super::normalized_syntax;

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

    pub(crate) fn shrink_to_fit(&mut self) {}
}

/// Macro-specific attributes that affect def-map visibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroDefinitionAttrs {
    pub macro_export: bool,
}

impl MacroDefinitionAttrs {
    fn from_macro_rules(item: &ast::MacroRules) -> Self {
        Self {
            macro_export: item
                .attrs()
                .filter(|attr| attr.kind().is_outer())
                .any(|attr| attr.simple_name().as_deref() == Some("macro_export")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroCallItem {
    pub path: Option<String>,
    pub callee: Option<Name>,
    pub args: Option<TopSubtree>,
}

impl MacroCallItem {
    pub fn from_ast(
        item: &ast::MacroCall,
        file_id: FileId,
        edition: RustEdition,
        interner: &mut NameInterner,
    ) -> Self {
        let span_factory = SpanFactory::new(file_id_u32(file_id), macro_edition(edition));
        let mut span_for_range = |range| span_factory.span_for(range);
        Self::from_ast_with_span(item, interner, &mut span_for_range)
    }

    pub fn from_ast_with_span(
        item: &ast::MacroCall,
        interner: &mut NameInterner,
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
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(path) = &mut self.path {
            path.shrink_to_fit();
        }
        if let Some(callee) = &mut self.callee {
            callee.shrink_to_fit();
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
