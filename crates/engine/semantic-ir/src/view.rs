//! Borrowed semantic item views shared by read transactions and downstream queries.

use rg_def_map::ItemSource;
use rg_ir_model::{LocalDefRef, LocalImplRef, ModuleRef};
use rg_item_tree::{Documentation, GenericParams, TypeRef, VisibilityLevel};
use rg_parse::Span;
use rg_text::Name;

use crate::{
    AssocItemId, ConstData, EnumData, FunctionData, ImplData, ItemOwner, SemanticItemKind,
    SemanticItemRef, StaticData, StructData, TraitData, TypeAliasData, TypeDefRef, UnionData,
};

/// Borrowed item-shaped facts for one semantic item.
#[derive(Debug, Clone, Copy)]
pub struct SemanticItemView<'a> {
    item: SemanticItemRef,
    data: SemanticItemData<'a>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SemanticItemData<'a> {
    Struct(&'a StructData),
    Union(&'a UnionData),
    Enum(&'a EnumData),
    Trait(&'a TraitData),
    Impl(&'a ImplData),
    Function(&'a FunctionData),
    TypeAlias(&'a TypeAliasData),
    Const(&'a ConstData),
    Static(&'a StaticData),
}

impl<'a> SemanticItemView<'a> {
    pub(crate) fn new(item: SemanticItemRef, data: SemanticItemData<'a>) -> Self {
        Self { item, data }
    }

    pub fn item(self) -> SemanticItemRef {
        self.item
    }

    pub fn kind(self) -> SemanticItemKind {
        match self.data {
            SemanticItemData::Struct(_) => SemanticItemKind::Struct,
            SemanticItemData::Union(_) => SemanticItemKind::Union,
            SemanticItemData::Enum(_) => SemanticItemKind::Enum,
            SemanticItemData::Trait(_) => SemanticItemKind::Trait,
            SemanticItemData::Impl(_) => SemanticItemKind::Impl,
            SemanticItemData::Function(_) => SemanticItemKind::Function,
            SemanticItemData::TypeAlias(_) => SemanticItemKind::TypeAlias,
            SemanticItemData::Const(_) => SemanticItemKind::Const,
            SemanticItemData::Static(_) => SemanticItemKind::Static,
        }
    }

    pub fn local_def(self) -> Option<LocalDefRef> {
        match self.data {
            SemanticItemData::Struct(data) => Some(data.local_def),
            SemanticItemData::Union(data) => Some(data.local_def),
            SemanticItemData::Enum(data) => Some(data.local_def),
            SemanticItemData::Trait(data) => Some(data.local_def),
            SemanticItemData::Impl(_) => None,
            SemanticItemData::Function(data) => data.local_def,
            SemanticItemData::TypeAlias(data) => data.local_def,
            SemanticItemData::Const(data) => data.local_def,
            SemanticItemData::Static(data) => Some(data.local_def),
        }
    }

    pub fn local_impl(self) -> Option<LocalImplRef> {
        match self.data {
            SemanticItemData::Impl(data) => Some(data.local_impl),
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Trait(_)
            | SemanticItemData::Function(_)
            | SemanticItemData::TypeAlias(_)
            | SemanticItemData::Const(_)
            | SemanticItemData::Static(_) => None,
        }
    }

    pub fn module_owner(self) -> Option<ModuleRef> {
        match self.data {
            SemanticItemData::Struct(data) => Some(data.owner),
            SemanticItemData::Union(data) => Some(data.owner),
            SemanticItemData::Enum(data) => Some(data.owner),
            SemanticItemData::Trait(data) => Some(data.owner),
            SemanticItemData::Impl(data) => Some(data.owner),
            SemanticItemData::Function(data) => match data.owner {
                ItemOwner::Module(module_ref) => Some(module_ref),
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => None,
            },
            SemanticItemData::TypeAlias(data) => match data.owner {
                ItemOwner::Module(module_ref) => Some(module_ref),
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => None,
            },
            SemanticItemData::Const(data) => match data.owner {
                ItemOwner::Module(module_ref) => Some(module_ref),
                ItemOwner::Trait(_) | ItemOwner::Impl(_) => None,
            },
            SemanticItemData::Static(data) => Some(data.owner),
        }
    }

    pub fn item_owner(self) -> Option<ItemOwner> {
        match self.data {
            SemanticItemData::Function(data) => Some(data.owner),
            SemanticItemData::TypeAlias(data) => Some(data.owner),
            SemanticItemData::Const(data) => Some(data.owner),
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Trait(_)
            | SemanticItemData::Impl(_)
            | SemanticItemData::Static(_) => None,
        }
    }

    pub fn type_def(self) -> Option<TypeDefRef> {
        match self.item {
            SemanticItemRef::TypeDef(ty) => Some(ty),
            SemanticItemRef::Trait(_)
            | SemanticItemRef::Impl(_)
            | SemanticItemRef::Function(_)
            | SemanticItemRef::TypeAlias(_)
            | SemanticItemRef::Const(_)
            | SemanticItemRef::Static(_) => None,
        }
    }

    pub fn assoc_items(self) -> Option<&'a [AssocItemId]> {
        match self.data {
            SemanticItemData::Trait(data) => Some(&data.items),
            SemanticItemData::Impl(data) => Some(&data.items),
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Function(_)
            | SemanticItemData::TypeAlias(_)
            | SemanticItemData::Const(_)
            | SemanticItemData::Static(_) => None,
        }
    }

    pub fn source(self) -> ItemSource {
        match self.data {
            SemanticItemData::Struct(data) => data.source,
            SemanticItemData::Union(data) => data.source,
            SemanticItemData::Enum(data) => data.source,
            SemanticItemData::Trait(data) => data.source,
            SemanticItemData::Impl(data) => data.source,
            SemanticItemData::Function(data) => data.source,
            SemanticItemData::TypeAlias(data) => data.source,
            SemanticItemData::Const(data) => data.source,
            SemanticItemData::Static(data) => data.source,
        }
    }

    pub fn name(self) -> Option<&'a Name> {
        match self.data {
            SemanticItemData::Struct(data) => Some(&data.name),
            SemanticItemData::Union(data) => Some(&data.name),
            SemanticItemData::Enum(data) => Some(&data.name),
            SemanticItemData::Trait(data) => Some(&data.name),
            SemanticItemData::Impl(_) => None,
            SemanticItemData::Function(data) => Some(&data.name),
            SemanticItemData::TypeAlias(data) => Some(&data.name),
            SemanticItemData::Const(data) => Some(&data.name),
            SemanticItemData::Static(data) => Some(&data.name),
        }
    }

    pub fn docs(self) -> Option<&'a Documentation> {
        match self.data {
            SemanticItemData::Struct(data) => data.docs.as_ref(),
            SemanticItemData::Union(data) => data.docs.as_ref(),
            SemanticItemData::Enum(data) => data.docs.as_ref(),
            SemanticItemData::Trait(data) => data.docs.as_ref(),
            SemanticItemData::Impl(_) => None,
            SemanticItemData::Function(data) => data.docs.as_ref(),
            SemanticItemData::TypeAlias(data) => data.docs.as_ref(),
            SemanticItemData::Const(data) => data.docs.as_ref(),
            SemanticItemData::Static(data) => data.docs.as_ref(),
        }
    }

    pub fn visibility(self) -> Option<&'a VisibilityLevel> {
        match self.data {
            SemanticItemData::Struct(data) => Some(&data.visibility),
            SemanticItemData::Union(data) => Some(&data.visibility),
            SemanticItemData::Enum(data) => Some(&data.visibility),
            SemanticItemData::Trait(data) => Some(&data.visibility),
            SemanticItemData::Impl(_) => None,
            SemanticItemData::Function(data) => Some(&data.visibility),
            SemanticItemData::TypeAlias(data) => Some(&data.visibility),
            SemanticItemData::Const(data) => Some(&data.visibility),
            SemanticItemData::Static(data) => Some(&data.visibility),
        }
    }

    pub fn generic_params(self) -> Option<&'a GenericParams> {
        match self.data {
            SemanticItemData::Struct(data) => Some(&data.generics),
            SemanticItemData::Union(data) => Some(&data.generics),
            SemanticItemData::Enum(data) => Some(&data.generics),
            SemanticItemData::Trait(data) => Some(&data.generics),
            SemanticItemData::Impl(data) => Some(&data.generics),
            SemanticItemData::Function(data) => data.signature.generics(),
            SemanticItemData::TypeAlias(data) => data.signature.generics(),
            SemanticItemData::Const(_) | SemanticItemData::Static(_) => None,
        }
    }

    pub fn span(self) -> Option<Span> {
        match self.data {
            SemanticItemData::Function(data) => Some(data.span),
            SemanticItemData::TypeAlias(data) => Some(data.span),
            SemanticItemData::Const(data) => Some(data.span),
            SemanticItemData::Static(data) => Some(data.span),
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Trait(_)
            | SemanticItemData::Impl(_) => None,
        }
    }

    pub fn name_span(self) -> Option<Span> {
        match self.data {
            SemanticItemData::Function(data) => data.name_span,
            SemanticItemData::TypeAlias(data) => data.name_span,
            SemanticItemData::Const(data) => data.name_span,
            SemanticItemData::Static(data) => data.name_span,
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Trait(_)
            | SemanticItemData::Impl(_) => None,
        }
    }

    pub fn impl_header(self) -> Option<(&'a TypeRef, Option<&'a TypeRef>)> {
        match self.data {
            SemanticItemData::Impl(data) => Some((&data.self_ty, data.trait_ref.as_ref())),
            SemanticItemData::Struct(_)
            | SemanticItemData::Union(_)
            | SemanticItemData::Enum(_)
            | SemanticItemData::Trait(_)
            | SemanticItemData::Function(_)
            | SemanticItemData::TypeAlias(_)
            | SemanticItemData::Const(_)
            | SemanticItemData::Static(_) => None,
        }
    }
}
