//! Extraction of compiler identities from item syntax.
//!
//! Item-tree lowering reads `#[lang = "..."]` and `#[rustc_diagnostic_item = "..."]` before
//! semantic item ids exist. It records the supported identity on `ItemNode`; semantic lowering
//! can then attach it to the typed item id without rediscovering the item from its name or path.

use rg_syntax::{AstNode as _, ast};

use super::MaybeFromAst;
use crate::item::LangItem;

impl MaybeFromAst for LangItem {
    type AstNode = dyn ast::HasAttrs;
    type Context<'a> = ();

    fn maybe_from_ast(item: &Self::AstNode, (): Self::Context<'_>) -> Option<Self> {
        item.attrs().find_map(|attr| {
            if !attr.kind().is_outer() {
                return None;
            }
            let ast::Meta::KeyValueMeta(meta) = attr.meta()? else {
                // `cfg_attr(..., lang = "...")` needs target-aware attribute expansion, which this
                // syntax extractor does not own. Keep nested attributes unresolved instead of
                // pretending every conditional attribute is active.
                return None;
            };
            let path = meta.path()?;
            let attribute = path.syntax().text();
            if attribute != "lang" && attribute != "rustc_diagnostic_item" {
                return None;
            }
            let ast::Expr::Literal(literal) = meta.expr()? else {
                return None;
            };
            let ast::LiteralKind::String(value) = literal.kind() else {
                return None;
            };
            LangItem::from_attr_value(&attribute.to_string(), &value.value().ok()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use rg_syntax::{Edition, SourceFile, ast, ast::HasModuleItem as _};

    use super::MaybeFromAst;
    use crate::item::LangItem;

    #[test]
    fn extracts_supported_language_items_from_their_own_declarations() {
        let parse = SourceFile::parse(
            r#"
            #[lang = "deref"]
            trait Deref {
                #[lang = "deref_target"]
                type Target;
            }
            "#,
            Edition::CURRENT,
        );
        let ast::Item::Trait(deref) = parse
            .tree()
            .items()
            .next()
            .expect("fixture should have item")
        else {
            panic!("fixture item should be a trait");
        };
        assert_eq!(LangItem::maybe_from_ast(&deref, ()), Some(LangItem::Deref));

        let ast::AssocItem::TypeAlias(target) = deref
            .assoc_item_list()
            .expect("fixture trait should have associated items")
            .assoc_items()
            .next()
            .expect("fixture trait should have Target")
        else {
            panic!("fixture associated item should be a type alias");
        };
        assert_eq!(
            LangItem::maybe_from_ast(&target, ()),
            Some(LangItem::DerefTarget)
        );
    }
}
