use rg_syntax::{AstNode as _, ast};
use rg_text::{Name, NameInterner};

use super::normalized_syntax;

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroDefinitionItem {
    pub syntax: MacroDefinitionSyntax,
    pub args: Option<String>,
    pub body: Option<String>,
}

impl MacroDefinitionItem {
    pub fn from_macro_rules(item: &ast::MacroRules) -> Self {
        Self {
            syntax: MacroDefinitionSyntax::MacroRules,
            args: None,
            body: item
                .token_tree()
                .map(|token_tree| token_tree.syntax().text().to_string()),
        }
    }

    pub fn from_macro_def(item: &ast::MacroDef) -> Self {
        Self {
            syntax: MacroDefinitionSyntax::MacroDef,
            args: item
                .args()
                .map(|token_tree| token_tree.syntax().text().to_string()),
            body: item
                .body()
                .map(|token_tree| token_tree.syntax().text().to_string()),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(args) = &mut self.args {
            args.shrink_to_fit();
        }
        if let Some(body) = &mut self.body {
            body.shrink_to_fit();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub enum MacroDefinitionSyntax {
    MacroRules,
    MacroDef,
}

#[derive(Debug, Clone, PartialEq, Eq, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct MacroCallItem {
    pub path: Option<String>,
    pub callee: Option<Name>,
    pub args: Option<String>,
}

impl MacroCallItem {
    pub fn from_ast(item: &ast::MacroCall, interner: &mut NameInterner) -> Self {
        Self {
            path: item.path().map(|path| normalized_syntax(&path)),
            callee: item
                .path()
                .and_then(|path| path.segment())
                .and_then(|segment| segment.name_ref())
                .map(|name_ref| interner.intern(name_ref.text())),
            args: item
                .token_tree()
                .map(|token_tree| token_tree.syntax().text().to_string()),
        }
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        if let Some(path) = &mut self.path {
            path.shrink_to_fit();
        }
        if let Some(callee) = &mut self.callee {
            callee.shrink_to_fit();
        }
        if let Some(args) = &mut self.args {
            args.shrink_to_fit();
        }
    }
}
