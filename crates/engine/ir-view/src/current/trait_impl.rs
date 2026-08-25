//! Request-local semantic storage for one trait impl from the editor buffer.
//!
//! A newly typed impl has no identity in saved Semantic IR. This module lowers just that impl and
//! its associated declarations into the same DefMap/item-store shape used by body-local items.
//! Header paths still resolve from the saved containing module, so the temporary store supplies
//! only facts that genuinely come from current syntax: the impl header, its generics, and members.

use anyhow::Context as _;
use rg_body_ir::BodyLocalItems;
use rg_def_map::{
    BodyItemSourceRef, DefMapBuilder, ItemSource, ItemSourceKind, LocalImplData, ModuleData,
    ModuleOrigin, ModuleScope, Visibility,
};
use rg_ir_model::{BodyRef, CrateRef, DefMapRef, ImplRef, ModuleRef, Path, TraitDefRef};
use rg_item_tree::{
    ConstItem, FromAst as _, FunctionItem, ImplItem, ImplItemContext, ItemKind, ItemNode,
    ItemTreeId, TypeAliasItem, VisibilityLevel,
};
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, LineIndex, Span};
use rg_semantic_ir::{
    ItemStoreLowerer, ItemStoreSourceReader, TypePathContext, TypePathResolution,
};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasName as _, HasVisibility as _},
};
use rg_text::NameInterner;
use rg_ty::{
    ItemPathQuery, SemanticSignatureQuery, TraitApplication, TypeLoweringAnchor, TypePathResolver,
};

use crate::{
    IndexedViewDb,
    trait_impl::{MissingTraitMember, TraitImplView},
};

/// Semantic interpretation of one trait impl that exists only in the request source.
///
/// The current impl is lowered into a one-impl DefMap and item store. That gives its generic
/// parameters normal semantic identities, so `impl<T> Service<T> for Worker<T>` can use the same
/// substitution and signature rendering as a saved impl. The small store is layered over a clone
/// of the request database and is never added to crate-wide discovery.
pub struct CurrentTraitImplView<'db> {
    db: IndexedViewDb<'db>,
    impl_ref: ImplRef,
    trait_ref: TraitDefRef,
    application: TraitApplication,
}

impl<'db> CurrentTraitImplView<'db> {
    /// Lower and resolve one current impl header in its saved containing module.
    ///
    /// Module declarations remain indexed, while the selected impl and its associated items come
    /// from the editor text. This deliberately does not publish current `use` declarations; a
    /// header name must still resolve from the saved module, its prelude, or a qualified path.
    pub fn new(
        db: &IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        fallback_module: ModuleRef,
        line_index: &LineIndex,
        impl_: &ast::Impl,
    ) -> anyhow::Result<Option<Self>> {
        if impl_.trait_().is_none() || fallback_module.origin.origin_crate() != crate_ref {
            return Ok(None);
        }

        let body_ref = db
            .next_synthetic_body_ref(crate_ref)
            .context("allocate current trait impl identity")?;
        let (items, impl_ref) = CurrentImplItems::lower(body_ref, file_id, line_index, impl_)
            .context("lower current trait impl items")?;
        let db = db.clone().with_request_local_items(items);
        let resolver = CurrentImplPathResolver {
            db: &db,
            current_origin: DefMapRef::Body(body_ref),
            fallback_module,
        };
        let signatures = SemanticSignatureQuery::with_resolver(&db, &db, resolver);
        let Some(header) = signatures
            .impl_header(impl_ref)
            .context("resolve current trait impl header")?
        else {
            return Ok(None);
        };
        let Some(trait_lowering) = header.trait_ref else {
            return Ok(None);
        };
        let application = trait_lowering.application;
        let trait_ref = application.def;

        Ok(Some(Self {
            db,
            impl_ref,
            trait_ref,
            application,
        }))
    }

    /// Return trait declarations absent from the impl currently shown in the editor.
    pub fn missing_members(&self) -> anyhow::Result<Vec<MissingTraitMember>> {
        TraitImplView::new(&self.db).missing_members_for_application(
            self.impl_ref,
            self.trait_ref,
            &self.application,
        )
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

    /// Lower only associated declarations that participate in missing-member comparison.
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

/// Resolves scratch impl paths as if the impl were still declared in its saved module.
struct CurrentImplPathResolver<'view, 'db> {
    db: &'view IndexedViewDb<'db>,
    current_origin: DefMapRef,
    fallback_module: ModuleRef,
}

impl TypePathResolver for CurrentImplPathResolver<'_, '_> {
    type Error = PackageStoreError;

    fn resolve_type_path(
        &self,
        anchor: TypeLoweringAnchor,
        path: &Path,
    ) -> Result<TypePathResolution, Self::Error> {
        let TypeLoweringAnchor::Context(mut context) = anchor else {
            return Ok(TypePathResolution::Unknown);
        };
        if context.module.origin == self.current_origin {
            context = TypePathContext {
                module: self.fallback_module,
                impl_ref: context.impl_ref,
            };
        }
        ItemPathQuery::new(self.db, self.db).resolve_type_path(context, path)
    }
}
