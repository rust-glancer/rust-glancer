//! Current declaration context copied into one request-local body item store.

use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, HasVisibility as _},
};

use rg_ir_model::ScopeId;
use rg_item_tree::{
    Documentation, FromAst as _, ImplItem, ImplItemContext, ItemKind, ItemTreeId, MaybeFromAst,
    OuterDocs, TraitItem, TraitItemContext, VisibilityLevel,
};

use super::{body::BodyLowering, syntax::associated_item_owner};

impl BodyLowering<'_> {
    /// Keep a new or changed function's signature in the same temporary store as its body.
    ///
    /// A free function is a direct declaration in the function's outer scope. An associated
    /// function is wrapped in a request-local copy of its enclosing impl or trait, with only this
    /// function attached. This gives body analysis its parameters, inherited generics, and `Self`
    /// type without publishing the declaration to crate-wide lookup.
    pub(super) fn lower_request_root_function(&mut self, function: &ast::Fn, scope: ScopeId) {
        if self.lower_request_root_associated_item(ast::AssocItem::Fn(function.clone()), scope) {
            return;
        }

        self.lower_request_root_module_item(ast::Item::Fn(function.clone()), scope);
    }

    /// Keep a new or changed const's signature and associated owner beside its initializer.
    pub(super) fn lower_request_root_const(&mut self, konst: &ast::Const, scope: ScopeId) {
        if self.lower_request_root_associated_item(ast::AssocItem::Const(konst.clone()), scope) {
            return;
        }

        self.lower_request_root_module_item(ast::Item::Const(konst.clone()), scope);
    }

    /// Keep a new or changed module static beside its initializer.
    pub(super) fn lower_request_root_static(&mut self, static_: &ast::Static, scope: ScopeId) {
        self.lower_request_root_module_item(ast::Item::Static(static_.clone()), scope);
    }

    /// Lower one request-root item directly into the root body's outer scope.
    fn lower_request_root_module_item(&mut self, item: ast::Item, scope: ScopeId) {
        let source = self.source(item.syntax());
        let node = self
            .lower_source_item(&item)
            .expect("a request-root declaration should lower to a source item");
        self.builder.alloc_scope_source_item(scope, node, source);
    }

    /// Add the current impl header and its sibling signatures to the selected body's local store.
    ///
    /// An unchanged selected method or const keeps its saved identity, so `include_selected` omits
    /// that one declaration from the temporary impl. A new or changed root must be included so the
    /// impl can supply its final request-local owner. In both cases the remaining function, const,
    /// and type-alias signatures become visible only from this body's lookup context.
    pub(super) fn lower_current_enclosing_impl(
        &mut self,
        selected: ast::AssocItem,
        scope: ScopeId,
        include_selected: bool,
    ) {
        let impl_item = associated_item_owner(selected.syntax())
            .and_then(ast::Impl::cast)
            .expect("an enclosing-impl root should belong to an impl");
        let selected_range = selected.syntax().text_range();
        let mut selected_was_seen = false;
        let mut members = Vec::new();

        if let Some(item_list) = impl_item.assoc_item_list() {
            for item in item_list.assoc_items() {
                let is_selected = item.syntax().text_range() == selected_range;
                selected_was_seen |= is_selected;
                if is_selected && !include_selected {
                    continue;
                }

                // The selected root was already chosen for this request. Other siblings still
                // obey cfg filtering before their signatures enter the temporary impl.
                let item = if is_selected {
                    item
                } else {
                    let Some(item) = self.cfg.enabled_syntax(item) else {
                        continue;
                    };
                    item
                };
                let source = self.source(item.syntax());
                if let Some(node) = self.lower_source_assoc_item(item) {
                    members.push(self.builder.alloc_scopeless_source_item(node, source));
                }
            }
        }
        debug_assert!(selected_was_seen);

        // TODO: Relate this temporary impl to the saved impl it shadows. Without that identity,
        // members removed or renamed in the editor can still arrive from saved lookup; a current
        // same-name member does take precedence through ordinary body-local lookup.
        self.lower_current_impl_wrapper(&impl_item, members, scope);
    }

    /// Wrap an associated request root in the impl or trait that supplies its inherited context.
    ///
    /// Only the selected declaration is copied into the wrapper. Sibling declarations remain
    /// saved-project facts, so choosing one body does not turn the surrounding edited item list
    /// into a new discoverable impl or trait.
    ///
    /// For `impl<T> Service<T> for Worker { fn edited(&self, _: T) {} fn sibling() {} }`, selecting
    /// `edited` builds a temporary impl containing that method only. The wrapper still supplies
    /// `T` and `Self`; `sibling` is not copied into the temporary item store.
    fn lower_request_root_associated_item(&mut self, item: ast::AssocItem, scope: ScopeId) -> bool {
        let Some(owner) = associated_item_owner(item.syntax()) else {
            return false;
        };
        let impl_item = ast::Impl::cast(owner.clone());
        let trait_item = ast::Trait::cast(owner);
        if impl_item.is_none() && trait_item.is_none() {
            return false;
        }
        let source = self.source(item.syntax());
        let Some(member) = self.lower_source_assoc_item(item) else {
            return false;
        };
        let member = self.builder.alloc_scopeless_source_item(member, source);

        if let Some(impl_item) = impl_item {
            self.lower_current_impl_wrapper(&impl_item, vec![member], scope);
            return true;
        }

        let (kind, name, visibility, docs, syntax) = if let Some(trait_item) = trait_item {
            (
                ItemKind::Trait(TraitItem::from_ast(
                    &trait_item,
                    TraitItemContext {
                        items: vec![member],
                        line_index: self.line_index,
                        interner: &mut *self.interner,
                    },
                )),
                trait_item.name(),
                VisibilityLevel::from_ast(&trait_item.visibility(), ()),
                <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(&trait_item, OuterDocs),
                trait_item.syntax().clone(),
            )
        } else {
            unreachable!("associated request root should have an impl or trait owner")
        };

        let node = self.named_source_item_node(kind, name, visibility, docs, &syntax);
        let owner_source = self.source(&syntax);
        self.builder
            .alloc_scope_source_item(scope, node, owner_source);
        true
    }

    /// Lower one request-local impl wrapper after its associated items have been selected.
    fn lower_current_impl_wrapper(
        &mut self,
        impl_item: &ast::Impl,
        members: Vec<ItemTreeId>,
        scope: ScopeId,
    ) {
        let kind = ItemKind::Impl(ImplItem::from_ast(
            impl_item,
            ImplItemContext {
                items: members,
                line_index: self.line_index,
                interner: &mut *self.interner,
            },
        ));
        let node = self.named_source_item_node(
            kind,
            None,
            VisibilityLevel::from_ast(&impl_item.visibility(), ()),
            <Documentation as MaybeFromAst<OuterDocs>>::maybe_from_ast(impl_item, OuterDocs),
            impl_item.syntax(),
        );
        let source = self.source(impl_item.syntax());
        self.builder.alloc_scope_source_item(scope, node, source);
    }
}
