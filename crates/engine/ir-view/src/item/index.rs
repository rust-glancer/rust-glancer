//! Generic declaration enumeration over indexed items.
//!
//! This view answers "what declarations exist in this target or body" without committing callers
//! to DefMap, Semantic IR, or Body IR storage shapes.

use rg_body_ir::BodyImplData;
use rg_ir_model::{
    AssocItemId, BodyFunctionRef, BodyImplRef, BodyItemRef, BodyValueItemRef, ConstRef, DefMapRef,
    EnumVariantRef as SemanticEnumVariantRef, FunctionRef as SemanticFunctionRef, ModuleId,
    ModuleRef, SemanticItemKind, TargetRef, TypeAliasRef, TypeDefId, TypeDefRef,
    identity::{DeclarationRef, DeclarationRefRepr, ImplRefRepr, ItemRefRepr},
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::SemanticItemView;

use crate::{IndexedViewDb, ty::locals::BodyView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSyntaxChild {
    name: String,
    file_id: FileId,
    span: Span,
}

impl IndexedSyntaxChild {
    pub fn field(file_id: FileId, name: String, span: Span) -> Self {
        Self {
            name,
            file_id,
            span,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedItemChild {
    Declaration(IndexedItem),
    Syntax(IndexedSyntaxChild),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedItem {
    declaration: DeclarationRef,
    children: Vec<IndexedItemChild>,
}

impl IndexedItem {
    pub fn declaration(&self) -> DeclarationRef {
        self.declaration
    }

    pub fn children(&self) -> &[IndexedItemChild] {
        &self.children
    }

    fn leaf(declaration: DeclarationRef) -> Self {
        Self {
            declaration,
            children: Vec::new(),
        }
    }

    fn with_children(declaration: DeclarationRef, children: Vec<IndexedItemChild>) -> Self {
        Self {
            declaration,
            children,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedBodyLocalGroup {
    owner: DeclarationRef,
    children: Vec<IndexedItem>,
}

impl IndexedBodyLocalGroup {
    pub fn owner(&self) -> DeclarationRef {
        self.owner
    }

    pub fn children(&self) -> &[IndexedItem] {
        &self.children
    }
}

pub struct ItemIndexView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> ItemIndexView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn included_targets(&self) -> anyhow::Result<Vec<TargetRef>> {
        Ok(self
            .db
            .semantic_ir
            .included_stores()?
            .into_iter()
            .map(|store| store.target_ref()) // TODO: smell -- can't we use item stores down the line?
            .collect())
    }

    pub fn module_declarations(&self, target: TargetRef) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(def_map) = self.db.def_map.def_map(target)? else {
            return Ok(Vec::new());
        };

        Ok(def_map
            .module_refs()
            .map(|module_ref| DeclarationRef::module(module_ref))
            .collect())
    }

    pub fn module_container_name(&self, module_ref: ModuleRef) -> anyhow::Result<Option<String>> {
        let Some(target) = module_ref.origin.as_target_ref() else {
            return Ok(None);
        };
        let Some(def_map) = self.db.def_map.def_map(target)? else {
            return Ok(None);
        };
        let Some(module) = def_map.module(module_ref.module) else {
            return Ok(None);
        };
        let Some(parent) = module.parent else {
            return Ok(None);
        };
        // Workspace-symbol containers are local module paths, not canonical package paths. A
        // direct child of the root module therefore has no visible container.
        let path = self.module_path(module_ref.origin, parent)?;

        Ok((!path.is_empty()).then_some(path))
    }

    pub fn module_owned_items(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> anyhow::Result<Vec<IndexedItem>> {
        let Some(store) = self.db.semantic_ir.items(target)? else {
            return Ok(Vec::new());
        };

        let mut items = Vec::new();
        for item in store.semantic_items() {
            if item.module_owner().is_none() {
                continue;
            }
            if file_id.is_some_and(|file_id| item.source().file_id != file_id) {
                continue;
            }
            if let Some(item) = self.semantic_item(item)? {
                items.push(item);
            }
        }
        Ok(items)
    }

    pub fn body_local_groups(
        &self,
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<IndexedBodyLocalGroup>> {
        let body_view = BodyView::new(self.db);
        let mut groups = Vec::new();

        for group in body_view.local_groups(target, file_id)? {
            let mut children = Vec::new();
            for declaration in body_view.local_scope_declarations(group.body(), file_id)? {
                if let Some(item) = self.item_for_declaration(declaration)? {
                    children.push(item);
                }
            }
            if children.is_empty() {
                continue;
            }
            groups.push(IndexedBodyLocalGroup {
                owner: group.owner(),
                children,
            });
        }

        Ok(groups)
    }

    fn semantic_item(&self, item: SemanticItemView<'_>) -> anyhow::Result<Option<IndexedItem>> {
        let declaration = DeclarationRef::semantic(item.item().into());
        match item.kind() {
            SemanticItemKind::Struct | SemanticItemKind::Union => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.type_def_item(declaration, ty)
            }
            SemanticItemKind::Enum => {
                let Some(ty) = item.type_def() else {
                    return Ok(None);
                };
                self.enum_item(declaration, ty)
            }
            SemanticItemKind::Trait | SemanticItemKind::Impl => {
                let children = item
                    .assoc_items()
                    .map(|items| self.assoc_item_children(item.item().origin(), items))
                    .transpose()?
                    .unwrap_or_default();
                Ok(Some(IndexedItem::with_children(declaration, children)))
            }
            SemanticItemKind::Function
            | SemanticItemKind::TypeAlias
            | SemanticItemKind::Const
            | SemanticItemKind::Static => Ok(Some(IndexedItem::leaf(declaration))),
        }
    }

    fn item_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<IndexedItem>> {
        match declaration.repr() {
            DeclarationRefRepr::Item(item) => match item.repr() {
                ItemRefRepr::BodyLocal(item) => self.body_item(item),
                ItemRefRepr::BodyLocalValue(_) | ItemRefRepr::Semantic(_) => {
                    Ok(Some(IndexedItem::leaf(declaration)))
                }
            },
            DeclarationRefRepr::Impl(impl_ref) => match impl_ref.repr() {
                ImplRefRepr::BodyLocal(impl_ref) => self.body_impl(impl_ref),
                ImplRefRepr::Semantic(_) => Ok(Some(IndexedItem::leaf(declaration))),
            },
            DeclarationRefRepr::Function(_)
            | DeclarationRefRepr::Module(_)
            | DeclarationRefRepr::NameDef(_)
            | DeclarationRefRepr::Field(_)
            | DeclarationRefRepr::EnumVariant(_)
            | DeclarationRefRepr::Binding(_) => Ok(Some(IndexedItem::leaf(declaration))),
        }
    }

    fn type_def_item(
        &self,
        declaration: DeclarationRef,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<IndexedItem>> {
        let mut children = Vec::new();
        for field in self.db.semantic_ir.fields_for_type(ty)? {
            children.push(IndexedItemChild::Declaration(IndexedItem::leaf(
                DeclarationRef::semantic(field.into()),
            )));
        }
        Ok(Some(IndexedItem::with_children(declaration, children)))
    }

    fn enum_item(
        &self,
        declaration: DeclarationRef,
        ty: TypeDefRef,
    ) -> anyhow::Result<Option<IndexedItem>> {
        let mut children = Vec::new();
        for variant_ref in self.enum_variant_refs(ty)? {
            let Some(variant) = self.db.semantic_ir.enum_variant_data(variant_ref)? else {
                continue;
            };
            let fields = variant
                .variant
                .fields
                .fields()
                .iter()
                .map(|field| {
                    IndexedItemChild::Syntax(IndexedSyntaxChild::field(
                        variant.file_id,
                        Self::field_label(field.key_declaration_label()),
                        field.span,
                    ))
                })
                .collect();
            children.push(IndexedItemChild::Declaration(IndexedItem::with_children(
                DeclarationRef::semantic(variant_ref.into()),
                fields,
            )));
        }
        Ok(Some(IndexedItem::with_children(declaration, children)))
    }

    fn body_item(&self, item_ref: BodyItemRef) -> anyhow::Result<Option<IndexedItem>> {
        let declaration = DeclarationRef::body_item(item_ref);
        let mut children = Vec::new();
        for field_ref in BodyView::new(self.db).fields_for_local_type(item_ref)? {
            children.push(IndexedItemChild::Declaration(IndexedItem::leaf(
                DeclarationRef::body_field(field_ref),
            )));
        }
        Ok(Some(IndexedItem::with_children(declaration, children)))
    }

    fn body_impl(&self, impl_ref: BodyImplRef) -> anyhow::Result<Option<IndexedItem>> {
        let Some(body) = self.db.body_ir.body_data(impl_ref.body)? else {
            return Ok(None);
        };
        let Some(impl_data) = body.local_impl(impl_ref.impl_id) else {
            return Ok(None);
        };
        Ok(Some(IndexedItem::with_children(
            DeclarationRef::body(impl_ref.into()),
            self.body_impl_children(impl_ref, impl_data)?,
        )))
    }

    fn body_impl_children(
        &self,
        impl_ref: BodyImplRef,
        impl_data: &BodyImplData,
    ) -> anyhow::Result<Vec<IndexedItemChild>> {
        let mut children = Vec::new();

        for item in &impl_data.types {
            let item_ref = BodyItemRef {
                body: impl_ref.body,
                item: *item,
            };
            if let Some(item) = self.body_item(item_ref)? {
                children.push(IndexedItemChild::Declaration(item));
            }
        }

        for item in &impl_data.consts {
            children.push(IndexedItemChild::Declaration(IndexedItem::leaf(
                DeclarationRef::body_value_item(BodyValueItemRef {
                    body: impl_ref.body,
                    item: *item,
                }),
            )));
        }

        for function in &impl_data.functions {
            children.push(IndexedItemChild::Declaration(IndexedItem::leaf(
                DeclarationRef::body_function(BodyFunctionRef {
                    body: impl_ref.body,
                    function: *function,
                }),
            )));
        }

        Ok(children)
    }

    fn assoc_item_children(
        &self,
        origin: DefMapRef,
        items: &[AssocItemId],
    ) -> anyhow::Result<Vec<IndexedItemChild>> {
        Ok(items
            .iter()
            .map(|item| {
                IndexedItemChild::Declaration(IndexedItem::leaf(Self::assoc_item(origin, item)))
            })
            .collect())
    }

    fn assoc_item(origin: DefMapRef, item: &AssocItemId) -> DeclarationRef {
        match item {
            AssocItemId::Function(id) => {
                DeclarationRef::semantic(SemanticFunctionRef { origin, id: *id }.into())
            }
            AssocItemId::TypeAlias(id) => {
                DeclarationRef::semantic(TypeAliasRef { origin, id: *id }.into())
            }
            AssocItemId::Const(id) => DeclarationRef::semantic(ConstRef { origin, id: *id }.into()),
        }
    }

    fn enum_variant_refs(&self, ty: TypeDefRef) -> anyhow::Result<Vec<SemanticEnumVariantRef>> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = self.db.semantic_ir.enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };

        Ok((0..data.variants.len())
            .map(|index| SemanticEnumVariantRef {
                origin: ty.origin,
                enum_id,
                index,
            })
            .collect())
    }

    fn field_label(name: Option<String>) -> String {
        name.unwrap_or_else(|| "<unsupported>".to_string())
    }

    fn module_path(&self, origin: DefMapRef, module: ModuleId) -> anyhow::Result<String> {
        let Some(target) = origin.as_target_ref() else {
            return Ok(String::new());
        };
        let Some(def_map) = self.db.def_map.def_map(target)? else {
            return Ok(String::new());
        };
        let Some(data) = def_map.module(module) else {
            return Ok(String::new());
        };
        let Some(name) = &data.name else {
            return Ok(String::new());
        };
        let Some(parent) = data.parent else {
            return Ok(name.to_string());
        };

        let parent_path = self.module_path(origin, parent)?;
        if parent_path.is_empty() {
            Ok(name.to_string())
        } else {
            Ok(format!("{parent_path}::{name}"))
        }
    }
}
