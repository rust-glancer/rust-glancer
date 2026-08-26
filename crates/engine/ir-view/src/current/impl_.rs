//! Request-local semantic storage for one impl from the editor buffer.
//!
//! A new impl has no saved semantic identity, and an edited header cannot safely borrow its old
//! identity. This view lowers only the selected impl header and its associated declarations into
//! ordinary DefMap and item-store shapes. The temporary module owns those current identities,
//! while path lookup starts in the saved module that contains the edited syntax.

use anyhow::Context as _;
use rg_body_ir::BodyLocalItems;
use rg_def_map::{
    BodyItemSourceRef, DefMapBuilder, ItemSource, ItemSourceKind, LocalImplData, ModuleData,
    ModuleOrigin, ModuleScope, Visibility,
};
use rg_ir_model::{BodyRef, CrateRef, ImplRef};
use rg_item_tree::{
    ConstItem, FromAst as _, FunctionItem, ImplItem, ImplItemContext, ItemKind, ItemNode,
    ItemTreeId, TypeAliasItem, VisibilityLevel,
};
use rg_parse::{CurrentSource, FileId, LineIndex, Span, enclosing_inline_module_path};
use rg_semantic_ir::{ItemStoreLowerer, ItemStoreSourceReader};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, HasVisibility as _},
};
use rg_text::NameInterner;

use crate::IndexedViewDb;

/// Semantic interpretation of one impl that exists only in the request source.
///
/// The view is useful both for trait-member comparison and for ordinary editor queries over an
/// edited impl header. It is deliberately absent from crate-wide item discovery: only a caller
/// interpreting the selected current declaration can see this store.
///
/// In `impl<Local> Service for model::Worker<Local>`, the temporary store owns the impl and
/// `Local`. Lookup for `Service` and `model::Worker` still starts in the saved module around the
/// edited syntax.
pub struct CurrentImplView<'db> {
    db: IndexedViewDb<'db>,
    impl_ref: ImplRef,
}

impl<'db> CurrentImplView<'db> {
    /// Lower one current impl in the saved module that surrounds it.
    pub fn new(
        db: &IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        fallback_module: rg_ir_model::ModuleRef,
        line_index: &LineIndex,
        impl_: &ast::Impl,
    ) -> anyhow::Result<Option<Self>> {
        if fallback_module.origin.origin_crate() != crate_ref {
            return Ok(None);
        }

        let body_ref = db
            .next_synthetic_body_ref(crate_ref)
            .context("allocate current impl identity")?;
        let (items, impl_ref) = CurrentImplItems::lower(body_ref, file_id, line_index, impl_)
            .context("lower current impl items")?;
        let db = db.clone().with_request_local_items(items, fallback_module);
        Ok(Some(Self { db, impl_ref }))
    }

    /// Select and lower the narrowest impl declaration containing one current-source offset.
    ///
    /// This path is used when no function, const, or static body owns the cursor—for example on
    /// `Service` or `Worker` in `impl Service for Worker`. The inline module path comes from the
    /// current syntax, but it must still identify an existing saved module before the overlay can
    /// borrow that module's surrounding names.
    pub fn at_offset(
        db: &IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        current_source: &CurrentSource,
        offset: u32,
    ) -> anyhow::Result<Option<Self>> {
        let edition = db
            .crate_edition(crate_ref)
            .context("read current impl crate edition")?;
        let Some(parse) = current_source.parse(edition) else {
            return Ok(None);
        };
        let Some(impl_) = parse
            .tree()
            .syntax()
            .descendants()
            .filter_map(ast::Impl::cast)
            .filter(|impl_| Span::from_text_range(impl_.syntax().text_range()).touches(offset))
            .min_by_key(|impl_| impl_.syntax().text_range().len())
        else {
            return Ok(None);
        };
        let inline_module_path = enclosing_inline_module_path(impl_.syntax());
        let Some(fallback_module) =
            db.def_map
                .module_for_inline_path(crate_ref, file_id, inline_module_path.as_slice())?
        else {
            return Ok(None);
        };

        Self::new(
            db,
            crate_ref,
            file_id,
            fallback_module,
            current_source.line_index(),
            &impl_,
        )
    }

    pub(crate) fn db(&self) -> &IndexedViewDb<'db> {
        &self.db
    }

    pub(crate) fn impl_ref(&self) -> ImplRef {
        self.impl_ref
    }

    pub fn into_db(self) -> IndexedViewDb<'db> {
        self.db
    }
}

/// Item-tree-shaped input and semantic storage for one current impl.
struct CurrentImplItems;

impl CurrentImplItems {
    /// Build the smallest ordinary DefMap/item-store pair that can own an impl header.
    fn lower(
        body_ref: BodyRef,
        file_id: FileId,
        line_index: &LineIndex,
        impl_: &ast::Impl,
    ) -> anyhow::Result<(BodyLocalItems, ImplRef)> {
        let mut interner = NameInterner::new();
        let mut source_items = Vec::new();
        let mut associated_items = Vec::new();
        if let Some(item_list) = impl_.assoc_item_list() {
            for item in item_list.assoc_items() {
                let Some(node) =
                    Self::associated_item_node(item, file_id, line_index, &mut interner)
                else {
                    continue;
                };
                let item_id = ItemTreeId(source_items.len());
                source_items.push(node);
                associated_items.push(item_id);
            }
        }

        let impl_span = Span::from_text_range(impl_.syntax().text_range());
        let impl_item = ImplItem::from_ast(
            impl_,
            ImplItemContext {
                items: associated_items,
                line_index,
                interner: &mut interner,
            },
        );
        let impl_item_id = ItemTreeId(source_items.len());
        source_items.push(ItemNode::source(
            ItemKind::Impl(impl_item),
            None,
            None,
            VisibilityLevel::from_ast(&impl_.visibility(), ()),
            None,
            impl_span,
            file_id,
        ));

        let mut def_map = DefMapBuilder::new_body(body_ref);
        let module = def_map.alloc_module(ModuleData {
            name: None,
            name_span: None,
            docs: None,
            user_facing_attrs: Default::default(),
            visibility: Visibility::Public,
            parent: None,
            children: Vec::new(),
            local_defs: Vec::new(),
            impls: Vec::new(),
            imports: Vec::new(),
            unresolved_imports: Vec::new(),
            scope: ModuleScope::default(),
            origin: ModuleOrigin::Synthetic {
                file_id,
                span: impl_span,
            },
        });
        let source = ItemSource::body(
            file_id,
            BodyItemSourceRef {
                body: body_ref,
                item: impl_item_id,
            },
        );
        let local_impl = def_map.alloc_local_impl(LocalImplData {
            module,
            source,
            file_id,
            span: impl_span,
        });
        def_map
            .module_mut(module)
            .expect("current impl module should exist")
            .impls
            .push(local_impl);
        let def_map = def_map.build();
        let item_store = ItemStoreLowerer::new(
            &def_map,
            CurrentImplItemReader {
                body_ref,
                source_items: &source_items,
            },
        )
        .lower()
        .context("lower current impl semantic store")?;
        let impl_ref = {
            let mut impls = item_store.impls_with_refs();
            let Some((impl_ref, _)) = impls.next() else {
                anyhow::bail!("current impl semantic store contains no impl");
            };
            anyhow::ensure!(
                impls.next().is_none(),
                "current impl semantic store contains more than one impl",
            );
            impl_ref
        };

        Ok((BodyLocalItems::new(def_map, item_store), impl_ref))
    }

    /// Lower only associated declarations whose signatures may be queried from current source.
    fn associated_item_node(
        item: ast::AssocItem,
        file_id: FileId,
        line_index: &LineIndex,
        interner: &mut NameInterner,
    ) -> Option<ItemNode> {
        let (kind, name, visibility, syntax) = match item {
            ast::AssocItem::Fn(item) => (
                ItemKind::Function(FunctionItem::from_ast(&item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                item.syntax().clone(),
            ),
            ast::AssocItem::TypeAlias(item) => (
                ItemKind::TypeAlias(TypeAliasItem::from_ast(&item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                item.syntax().clone(),
            ),
            ast::AssocItem::Const(item) => (
                ItemKind::Const(ConstItem::from_ast(&item, (line_index, &mut *interner))),
                item.name(),
                VisibilityLevel::from_ast(&item.visibility(), ()),
                item.syntax().clone(),
            ),
            ast::AssocItem::MacroCall(_) => return None,
        };
        let name_span = name
            .as_ref()
            .map(|name| Span::from_text_range(name.syntax().text_range()));
        let name = name.map(|name| interner.intern(name.text()));
        Some(ItemNode::source(
            kind,
            name,
            name_span,
            visibility,
            None,
            Span::from_text_range(syntax.text_range()),
            file_id,
        ))
    }
}

/// Reads the temporary source arena through the ordinary semantic item lowerer contract.
struct CurrentImplItemReader<'items> {
    body_ref: BodyRef,
    source_items: &'items [ItemNode],
}

impl<'items> ItemStoreSourceReader<'items> for CurrentImplItemReader<'items> {
    fn item(&self, source: ItemSource) -> anyhow::Result<&'items ItemNode> {
        let ItemSourceKind::Body(source) = source.kind else {
            anyhow::bail!("current impl item source is not request-local");
        };
        anyhow::ensure!(
            source.body == self.body_ref,
            "current impl item source belongs to another request-local origin",
        );
        self.source_items
            .get(source.item.0)
            .context("current impl source item does not exist")
    }
}
