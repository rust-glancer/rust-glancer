use rg_arena::Arena;
use rg_def_map::{LocalDefRef, LocalImplRef, ModuleRef};
use rg_item_tree::{
    Documentation, EnumVariantItem, FieldItem, FieldList, GenericParams, ItemTreeRef, Mutability,
    ParamKind, TypeBound, TypeRef, VisibilityLevel,
};
use rg_parse::{FileId, Span};
use rg_text::Name;

use crate::{
    ids::{
        AssocItemId, ConstId, EnumId, FunctionId, ImplId, ItemOwner, StaticId, StructId, TraitId,
        TraitRef, TypeAliasId, TypeDefRef, UnionId,
    },
    signature::{ConstSignature, FunctionSignature, TypeAliasSignature},
};

/// Target-local storage for semantic items.
///
/// Semantic ids are dense indexes into these vectors. Keeping all item families in one store lets
/// lowering allocate ids cheaply while the public query surface exposes stable typed references.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ItemStore {
    pub(crate) structs: Arena<StructId, StructData>,
    pub(crate) unions: Arena<UnionId, UnionData>,
    pub(crate) enums: Arena<EnumId, EnumData>,
    pub(crate) traits: Arena<TraitId, TraitData>,
    pub(crate) impls: Arena<ImplId, ImplData>,
    pub(crate) functions: Arena<FunctionId, FunctionData>,
    pub(crate) type_aliases: Arena<TypeAliasId, TypeAliasData>,
    pub(crate) consts: Arena<ConstId, ConstData>,
    pub(crate) statics: Arena<StaticId, StaticData>,
}

impl ItemStore {
    pub fn struct_data(&self, id: StructId) -> Option<&StructData> {
        self.structs.get(id)
    }

    pub fn union_data(&self, id: UnionId) -> Option<&UnionData> {
        self.unions.get(id)
    }

    pub fn enum_data(&self, id: EnumId) -> Option<&EnumData> {
        self.enums.get(id)
    }

    pub fn trait_data(&self, id: TraitId) -> Option<&TraitData> {
        self.traits.get(id)
    }

    pub fn impl_data(&self, id: ImplId) -> Option<&ImplData> {
        self.impls.get(id)
    }

    pub fn function_data(&self, id: FunctionId) -> Option<&FunctionData> {
        self.functions.get(id)
    }

    pub fn type_alias_data(&self, id: TypeAliasId) -> Option<&TypeAliasData> {
        self.type_aliases.get(id)
    }

    pub fn const_data(&self, id: ConstId) -> Option<&ConstData> {
        self.consts.get(id)
    }

    pub fn static_data(&self, id: StaticId) -> Option<&StaticData> {
        self.statics.get(id)
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.structs.shrink_to_fit();
        for data in self.structs.iter_mut() {
            data.shrink_to_fit();
        }
        self.unions.shrink_to_fit();
        for data in self.unions.iter_mut() {
            data.shrink_to_fit();
        }
        self.enums.shrink_to_fit();
        for data in self.enums.iter_mut() {
            data.shrink_to_fit();
        }
        self.traits.shrink_to_fit();
        for data in self.traits.iter_mut() {
            data.shrink_to_fit();
        }
        self.impls.shrink_to_fit();
        for data in self.impls.iter_mut() {
            data.shrink_to_fit();
        }
        self.functions.shrink_to_fit();
        for data in self.functions.iter_mut() {
            data.shrink_to_fit();
        }
        self.type_aliases.shrink_to_fit();
        for data in self.type_aliases.iter_mut() {
            data.shrink_to_fit();
        }
        self.consts.shrink_to_fit();
        for data in self.consts.iter_mut() {
            data.shrink_to_fit();
        }
        self.statics.shrink_to_fit();
        for data in self.statics.iter_mut() {
            data.shrink_to_fit();
        }
    }
}

impl ItemStore {
    pub(crate) fn alloc_struct(&mut self, data: StructData) -> StructId {
        self.structs.alloc(data)
    }

    pub(crate) fn alloc_union(&mut self, data: UnionData) -> UnionId {
        self.unions.alloc(data)
    }

    pub(crate) fn alloc_enum(&mut self, data: EnumData) -> EnumId {
        self.enums.alloc(data)
    }

    pub(crate) fn alloc_trait(&mut self, data: TraitData) -> TraitId {
        self.traits.alloc(data)
    }

    pub(crate) fn alloc_impl(&mut self, data: ImplData) -> ImplId {
        self.impls.alloc(data)
    }

    pub(crate) fn alloc_function(&mut self, data: FunctionData) -> FunctionId {
        self.functions.alloc(data)
    }

    pub(crate) fn alloc_type_alias(&mut self, data: TypeAliasData) -> TypeAliasId {
        self.type_aliases.alloc(data)
    }

    pub(crate) fn alloc_const(&mut self, data: ConstData) -> ConstId {
        self.consts.alloc(data)
    }

    pub(crate) fn alloc_static(&mut self, data: StaticData) -> StaticId {
        self.statics.alloc(data)
    }
}

/// Borrowed view over one field plus the semantic owner facts needed by analysis.
#[derive(Debug, Clone, Copy)]
pub struct FieldData<'a> {
    pub owner_module: ModuleRef,
    pub file_id: FileId,
    pub field: &'a FieldItem,
}

/// Borrowed view over one enum variant plus the owning enum facts needed by analysis.
///
/// The owner data is repeated here so callers do not have to re-open the enum just to answer
/// editor questions such as "what type does this variant construct?".
#[derive(Debug, Clone, Copy)]
pub struct EnumVariantData<'a> {
    pub owner: TypeDefRef,
    pub owner_module: ModuleRef,
    pub file_id: FileId,
    pub variant: &'a EnumVariantItem,
}

/// Nominal struct lowered from a module item.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StructData {
    pub local_def: LocalDefRef,
    pub source: ItemTreeRef,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub fields: FieldList,
}

impl StructData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
    }
}

/// Nominal union lowered from a module item.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct UnionData {
    pub local_def: LocalDefRef,
    pub source: ItemTreeRef,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub fields: Vec<FieldItem>,
}

impl UnionData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.generics.shrink_to_fit();
        self.fields.shrink_to_fit();
        for field in &mut self.fields {
            field.shrink_to_fit();
        }
    }
}

/// Enum definition together with variant payloads.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct EnumData {
    pub local_def: LocalDefRef,
    pub source: ItemTreeRef,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub variants: Vec<EnumVariantItem>,
}

impl EnumData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.generics.shrink_to_fit();
        self.variants.shrink_to_fit();
        for variant in &mut self.variants {
            variant.shrink_to_fit();
        }
    }
}

/// Trait signature and associated items.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TraitData {
    pub local_def: LocalDefRef,
    pub source: ItemTreeRef,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub generics: GenericParams,
    pub super_traits: Vec<TypeBound>,
    pub items: Vec<AssocItemId>,
    pub is_unsafe: bool,
}

impl TraitData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.generics.shrink_to_fit();
        self.super_traits.shrink_to_fit();
        for bound in &mut self.super_traits {
            bound.shrink_to_fit();
        }
        self.items.shrink_to_fit();
    }
}

/// Impl block header and associated items.
///
/// `resolved_*` fields are intentionally lossy and optimistic: they record all type/trait targets
/// that our current path resolver can identify, without attempting a real trait solver.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ImplData {
    pub local_impl: LocalImplRef,
    pub source: ItemTreeRef,
    pub owner: ModuleRef,
    pub generics: GenericParams,
    pub trait_ref: Option<TypeRef>,
    pub self_ty: TypeRef,
    pub resolved_self_tys: Vec<TypeDefRef>,
    pub resolved_trait_refs: Vec<TraitRef>,
    pub items: Vec<AssocItemId>,
    pub is_unsafe: bool,
}

impl ImplData {
    fn shrink_to_fit(&mut self) {
        self.generics.shrink_to_fit();
        if let Some(trait_ref) = &mut self.trait_ref {
            trait_ref.shrink_to_fit();
        }
        self.self_ty.shrink_to_fit();
        self.resolved_self_tys.shrink_to_fit();
        self.resolved_trait_refs.shrink_to_fit();
        self.items.shrink_to_fit();
    }
}

/// Function signature and source identity.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemTreeRef,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: FunctionSignature,
}

impl FunctionData {
    pub fn has_self_receiver(&self) -> bool {
        self.signature
            .params()
            .first()
            .is_some_and(|param| matches!(param.kind, ParamKind::SelfParam))
    }

    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.signature.shrink_to_fit();
    }
}

/// Type alias signature and optional aliased type.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypeAliasData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemTreeRef,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: TypeAliasSignature,
}

impl TypeAliasData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.signature.shrink_to_fit();
    }
}

/// Const signature and optional value body owner.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstData {
    pub local_def: Option<LocalDefRef>,
    pub source: ItemTreeRef,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ItemOwner,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub signature: ConstSignature,
}

impl ConstData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        self.signature.shrink_to_fit();
    }
}

/// Module-level static item.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StaticData {
    pub local_def: LocalDefRef,
    pub source: ItemTreeRef,
    pub span: Span,
    pub name_span: Option<Span>,
    pub owner: ModuleRef,
    pub name: Name,
    pub visibility: VisibilityLevel,
    pub docs: Option<Documentation>,
    pub ty: Option<TypeRef>,
    pub mutability: Mutability,
}

impl StaticData {
    fn shrink_to_fit(&mut self) {
        self.name.shrink_to_fit();
        if let Some(docs) = &mut self.docs {
            docs.shrink_to_fit();
        }
        if let Some(ty) = &mut self.ty {
            ty.shrink_to_fit();
        }
    }
}
