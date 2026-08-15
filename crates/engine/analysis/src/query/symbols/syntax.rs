//! Exact-source document outlines.
//!
//! A document outline describes the text the editor is showing, so it does not need identities
//! from the saved project. This collector walks named Rust syntax and keeps its nesting.
//!
//! It intentionally does not expand macros or add information from semantic analysis. Saving may
//! make those declarations available to workspace-wide features, but the outline of the current
//! document should still match what is on screen before save.

use rg_ir_view::SymbolKind;
use rg_parse::Span;
use rg_syntax::{AstNode as _, SourceFile, SyntaxNode, ast, ast::HasModuleItem as _};

use crate::DocumentSymbol;

pub(super) struct SyntaxDocumentSymbolCollector;

impl SyntaxDocumentSymbolCollector {
    /// Collect the named syntax nodes that can appear in an editor outline.
    pub(super) fn collect(syntax: &SourceFile) -> Vec<DocumentSymbol> {
        let collector = Self;
        syntax
            .items()
            .filter_map(|item| collector.item(item, false))
            .collect()
    }

    fn item(&self, item: ast::Item, associated: bool) -> Option<DocumentSymbol> {
        match item {
            ast::Item::Const(item) => self.named(item, SymbolKind::Const, Vec::new()),
            ast::Item::Enum(item) => {
                let children = item
                    .variant_list()
                    .into_iter()
                    .flat_map(|list| list.variants())
                    .filter_map(|variant| self.variant(variant))
                    .collect();
                self.named(item, SymbolKind::Enum, children)
            }
            ast::Item::Fn(item) => {
                let children = self.body_items(item.syntax());
                let kind = if associated {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                self.named(item, kind, children)
            }
            ast::Item::Impl(item) => self.impl_symbol(item),
            ast::Item::MacroDef(item) => self.named(item, SymbolKind::Macro, Vec::new()),
            ast::Item::MacroRules(item) => self.named(item, SymbolKind::Macro, Vec::new()),
            ast::Item::Module(item) => {
                let children = item
                    .item_list()
                    .into_iter()
                    .flat_map(|list| list.items())
                    .filter_map(|item| self.item(item, false))
                    .collect();
                self.named(item, SymbolKind::Module, children)
            }
            ast::Item::Static(item) => self.named(item, SymbolKind::Static, Vec::new()),
            ast::Item::Struct(item) => {
                let children = item
                    .field_list()
                    .map(|fields| self.fields(fields))
                    .unwrap_or_default();
                self.named(item, SymbolKind::Struct, children)
            }
            ast::Item::Trait(item) => {
                let children = item
                    .assoc_item_list()
                    .into_iter()
                    .flat_map(|list| list.assoc_items())
                    .filter_map(|item| self.associated_item(item))
                    .collect();
                self.named(item, SymbolKind::Trait, children)
            }
            ast::Item::TypeAlias(item) => self.named(item, SymbolKind::TypeAlias, Vec::new()),
            ast::Item::Union(item) => {
                let children = item
                    .record_field_list()
                    .into_iter()
                    .flat_map(|fields| fields.fields())
                    .filter_map(|field| self.named(field, SymbolKind::Field, Vec::new()))
                    .collect();
                self.named(item, SymbolKind::Union, children)
            }
            ast::Item::AsmExpr(_)
            | ast::Item::ExternBlock(_)
            | ast::Item::ExternCrate(_)
            | ast::Item::MacroCall(_)
            | ast::Item::Use(_) => None,
        }
    }

    fn associated_item(&self, item: ast::AssocItem) -> Option<DocumentSymbol> {
        match item {
            ast::AssocItem::Const(item) => self.named(item, SymbolKind::Const, Vec::new()),
            ast::AssocItem::Fn(item) => {
                let children = self.body_items(item.syntax());
                self.named(item, SymbolKind::Method, children)
            }
            ast::AssocItem::TypeAlias(item) => self.named(item, SymbolKind::TypeAlias, Vec::new()),
            ast::AssocItem::MacroCall(_) => None,
        }
    }

    fn impl_symbol(&self, item: ast::Impl) -> Option<DocumentSymbol> {
        let self_ty = item.self_ty()?.syntax().text().to_string();
        let name = if let Some(trait_) = item.trait_() {
            format!("{} for {self_ty}", trait_.syntax().text())
        } else {
            self_ty
        };
        let children = item
            .assoc_item_list()
            .into_iter()
            .flat_map(|list| list.assoc_items())
            .filter_map(|item| self.associated_item(item))
            .collect();
        let span = syntax_span(item.syntax());
        Some(DocumentSymbol {
            name,
            kind: SymbolKind::Impl,
            span,
            selection_span: span,
            children,
        })
    }

    fn variant(&self, variant: ast::Variant) -> Option<DocumentSymbol> {
        let children = variant
            .field_list()
            .map(|fields| self.fields(fields))
            .unwrap_or_default();
        self.named(variant, SymbolKind::EnumVariant, children)
    }

    fn fields(&self, fields: ast::FieldList) -> Vec<DocumentSymbol> {
        match fields {
            ast::FieldList::RecordFieldList(fields) => fields
                .fields()
                .filter_map(|field| self.named(field, SymbolKind::Field, Vec::new()))
                .collect(),
            ast::FieldList::TupleFieldList(fields) => fields
                .fields()
                .enumerate()
                .map(|(index, field)| {
                    let span = syntax_span(field.syntax());
                    DocumentSymbol {
                        name: format!("#{index}"),
                        kind: SymbolKind::Field,
                        span,
                        selection_span: span,
                        children: Vec::new(),
                    }
                })
                .collect(),
        }
    }

    /// Attach every directly contained local item to its function or method.
    ///
    /// Items in nested blocks still belong to the function outline. Items inside a local item are
    /// handled by that item's own recursive call, so they are not duplicated here.
    fn body_items(&self, owner: &SyntaxNode) -> Vec<DocumentSymbol> {
        owner
            .descendants()
            .skip(1)
            .filter_map(ast::Item::cast)
            .filter(|item| {
                item.syntax()
                    .ancestors()
                    .skip(1)
                    .find(|ancestor| ast::Item::can_cast(ancestor.kind()))
                    .is_some_and(|ancestor| ancestor == *owner)
            })
            .filter_map(|item| self.item(item, false))
            .collect()
    }

    fn named<N>(
        &self,
        node: N,
        kind: SymbolKind,
        children: Vec<DocumentSymbol>,
    ) -> Option<DocumentSymbol>
    where
        N: ast::HasName,
    {
        let name = node.name()?;
        Some(DocumentSymbol {
            name: name.text().to_string(),
            kind,
            span: syntax_span(node.syntax()),
            selection_span: syntax_span(name.syntax()),
            children,
        })
    }
}

fn syntax_span(node: &SyntaxNode) -> Span {
    Span::from_text_range(node.text_range())
}

#[cfg(test)]
mod tests {
    use rg_ir_view::SymbolKind;
    use rg_syntax::{Edition, SourceFile};

    use super::SyntaxDocumentSymbolCollector;

    #[test]
    fn outlines_exact_nested_and_body_local_syntax() {
        let source = r#"
struct User { id: u32 }

impl User {
    fn new() {
        struct Local;
    }
}
"#;

        let syntax = SourceFile::parse(source, Edition::Edition2021).tree();
        let symbols = SyntaxDocumentSymbolCollector::collect(&syntax);

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "User");
        assert_eq!(symbols[0].children[0].name, "id");
        assert_eq!(symbols[1].kind, SymbolKind::Impl);
        assert_eq!(symbols[1].name, "User");
        assert_eq!(symbols[1].children[0].name, "new");
        assert_eq!(symbols[1].children[0].children[0].name, "Local");
    }
}
