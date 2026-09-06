//! Declaration context needed by a selected body or an impl-header query.
//!
//! Body contexts may omit the selected member when it keeps its saved identity. A complete impl
//! includes every supported current member. Both use the same lowering and cfg policy here.

use rg_cfg_eval::CfgEvaluator;
use rg_item_tree::{
    Documentation, FromAst as _, ImplItem, ImplItemContext, ItemKind, ItemNode, ItemTreeId,
    MaybeFromAst, OuterDocs, TraitItem, TraitItemContext, VisibilityLevel,
};
use rg_parse::{FileId, LineIndex, Span};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, HasVisibility as _},
};
use rg_text::NameInterner;

use crate::build::local_items::LocalItemLowering;
use crate::{BodySource, BodySourceItems};

/// Which current declaration supplies a selected body's inherited context.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CurrentRootItems {
    /// The root's signature already lives in saved data or an enclosing body.
    None,
    /// A root without a saved identity needs its own signature and any trait owner header.
    Declaration,
    /// Copy the impl header and sibling signatures. Use `include_selected: false` when the
    /// selected member keeps its saved identity, so we do not create a second copy of it.
    EnclosingImpl { include_selected: bool },
}

/// Build declaration trees for the scope around a selected body or an impl-header query.
///
/// For `impl<T> Wrapper<T> { fn get(&self) -> T { ... } }`, copying just `get` would lose
/// the enclosing `T` and `Self` context. We keep the needed owner headers alongside the
/// member signatures, without copying their expression bodies.
pub(crate) struct CurrentDeclarationBuilder<'a> {
    pub(crate) file: FileId,
    pub(crate) line_index: &'a LineIndex,
    pub(crate) cfg: CfgEvaluator<'a>,
    pub(crate) interner: &'a mut NameInterner,
    pub(crate) items: &'a mut BodySourceItems,
    pub(crate) cancellation: &'a rg_std::CancellationToken,
}

impl CurrentDeclarationBuilder<'_> {
    /// An associated item sits directly inside the impl or trait's member list.
    pub(crate) fn associated_owner(
        syntax: &rg_syntax::SyntaxNode,
    ) -> Option<rg_syntax::SyntaxNode> {
        let list = syntax.parent()?;
        ast::AssocItemList::cast(list.clone())?;
        list.parent()
    }

    /// Lower the declaration tree that the caller attaches to the body's outer scope.
    pub(crate) fn root(
        &mut self,
        item: ast::Item,
        role: CurrentRootItems,
    ) -> anyhow::Result<Option<ItemTreeId>> {
        if matches!(role, CurrentRootItems::None) {
            return Ok(None);
        }
        let owner = Self::associated_owner(item.syntax());
        if let Some(impl_) = owner.clone().and_then(ast::Impl::cast) {
            let selected = Span::from_text_range(item.syntax().text_range());
            let include_selected = match role {
                CurrentRootItems::EnclosingImpl { include_selected } => include_selected,
                CurrentRootItems::Declaration => true,
                CurrentRootItems::None => unreachable!("empty root role returned above"),
            };
            return self
                .impl_(&impl_, Some((selected, include_selected)))
                .map(Some);
        }

        let Some(member) = self.declaration(&item) else {
            return Ok(None);
        };
        if let Some(trait_) = owner.and_then(ast::Trait::cast) {
            let kind = ItemKind::Trait(TraitItem::from_ast(
                &trait_,
                TraitItemContext {
                    items: vec![member],
                    line_index: self.line_index,
                    interner: &mut *self.interner,
                },
            ));
            let name = trait_.name();
            let name_span = name
                .as_ref()
                .map(|name| Span::from_text_range(name.syntax().text_range()));
            let name = name.map(|name| self.interner.intern(name.text()));
            let source = self.source(trait_.syntax());
            let node = ItemNode::source(
                kind,
                name,
                name_span,
                VisibilityLevel::from_ast(&trait_.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&trait_, OuterDocs),
                source.span,
                source.file_id,
            );
            return Ok(Some(self.items.alloc(node, source)));
        }
        Ok(Some(member))
    }

    /// Copy associated signatures without lowering their expression bodies.
    ///
    /// `None` collects a complete impl. `Some((span, include))` identifies the selected member
    /// by its declaration span. Omit it when its saved signature is reused, or include it
    /// without a second cfg check.
    pub(crate) fn impl_(
        &mut self,
        impl_: &ast::Impl,
        selected: Option<(Span, bool)>,
    ) -> anyhow::Result<ItemTreeId> {
        let mut members = Vec::new();
        if let Some(list) = impl_.assoc_item_list() {
            for item in list.assoc_items() {
                rg_std::check_cancel!(self.cancellation, "current associated declaration");
                let is_selected = selected.is_some_and(|(span, _)| {
                    span == Span::from_text_range(item.syntax().text_range())
                });
                if is_selected && selected.is_some_and(|(_, include)| !include) {
                    continue;
                }
                // Root selection already accepted the edited member. Its siblings must still
                // obey this Cargo target's cfg before entering the temporary context.
                let item = if is_selected {
                    item
                } else {
                    let Some(item) = self.cfg.enabled_syntax(item) else {
                        continue;
                    };
                    item
                };
                let Some(item) = LocalItemLowering::associated(item) else {
                    continue;
                };
                if let Some(member) = self.declaration(&item) {
                    members.push(member);
                }
            }
        }
        // TODO: Relate this temporary impl to the saved impl it shadows. Removed or renamed
        // members can still arrive from saved lookup; current same-name members take precedence.
        let kind = ItemKind::Impl(ImplItem::from_ast(
            impl_,
            ImplItemContext {
                items: members,
                line_index: self.line_index,
                interner: &mut *self.interner,
            },
        ));
        let source = self.source(impl_.syntax());
        let node = ItemNode::source(
            kind,
            None,
            None,
            VisibilityLevel::from_ast(&impl_.visibility(), ()),
            <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(impl_, OuterDocs),
            source.span,
            source.file_id,
        );
        Ok(self.items.alloc(node, source))
    }

    fn declaration(&mut self, item: &ast::Item) -> Option<ItemTreeId> {
        let source = self.source(item.syntax());
        let name_span =
            LocalItemLowering::name_syntax(item).map(|name| self.source(name.syntax()).span);
        let node = LocalItemLowering::declaration(
            item,
            self.line_index,
            self.interner,
            source,
            name_span,
        )?;
        Some(self.items.alloc(node, source))
    }

    fn source(&self, syntax: &rg_syntax::SyntaxNode) -> BodySource {
        BodySource::written(self.file, Span::from_text_range(syntax.text_range()))
    }
}
