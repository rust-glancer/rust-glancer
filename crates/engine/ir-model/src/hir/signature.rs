//! Compact item signatures retained by semantic IR.
//!
//! Item tree declarations preserve the whole syntax-shaped item header. Semantic IR usually needs
//! only the parts that participate in queries, so these types keep the hot declaration families
//! smaller without making downstream crates know about the storage tradeoff.

use crate::items::{
    ConstItem, FunctionItem, FunctionQualifiers, GenericParams, ParamItem, TypeAliasItem,
    TypeBound, TypeRef,
};
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Generic params in function/type-alias signatures.
///
/// Most Rust functions are not generic. Boxing only the non-empty case keeps `FunctionData`
/// compact while still preserving the exact syntax facts for signatures that need them.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) enum SignatureGenerics {
    #[default]
    Empty,
    Present(Box<GenericParams>),
}

impl SignatureGenerics {
    fn from_params(params: &GenericParams) -> Self {
        if params.lifetimes.is_empty()
            && params.types.is_empty()
            && params.consts.is_empty()
            && params.where_predicates.is_empty()
        {
            Self::Empty
        } else {
            Self::Present(Box::new(params.clone()))
        }
    }

    fn as_ref(&self) -> Option<&GenericParams> {
        match self {
            Self::Empty => None,
            Self::Present(params) => Some(params),
        }
    }
}

/// Function header facts used by semantic queries and LSP display.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct FunctionSignature {
    pub(crate) generics: SignatureGenerics,
    pub(crate) params: Box<[ParamItem]>,
    pub(crate) ret_ty: Option<TypeRef>,
    pub(crate) qualifiers: FunctionQualifiers,
}

impl FunctionSignature {
    pub fn from_item(item: &FunctionItem) -> Self {
        let mut params = item.params.clone();
        Shrink::shrink_to_fit(&mut params);

        Self {
            generics: SignatureGenerics::from_params(&item.generics),
            params: params.into_boxed_slice(),
            ret_ty: item.ret_ty.clone(),
            qualifiers: item.qualifiers,
        }
    }

    pub fn generics(&self) -> Option<&GenericParams> {
        self.generics.as_ref()
    }

    pub fn params(&self) -> &[ParamItem] {
        &self.params
    }

    pub fn ret_ty(&self) -> Option<&TypeRef> {
        self.ret_ty.as_ref()
    }

    pub fn qualifiers(&self) -> FunctionQualifiers {
        self.qualifiers
    }
}

/// Type alias header facts used by signature cursors and hovers.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct TypeAliasSignature {
    pub(crate) generics: SignatureGenerics,
    pub(crate) bounds: Box<[TypeBound]>,
    pub(crate) aliased_ty: Option<TypeRef>,
}

impl TypeAliasSignature {
    pub fn from_item(item: &TypeAliasItem) -> Self {
        let mut bounds = item.bounds.clone();
        Shrink::shrink_to_fit(&mut bounds);

        Self {
            generics: SignatureGenerics::from_params(&item.generics),
            bounds: bounds.into_boxed_slice(),
            aliased_ty: item.aliased_ty.clone(),
        }
    }

    pub fn generics(&self) -> Option<&GenericParams> {
        self.generics.as_ref()
    }

    pub fn bounds(&self) -> &[TypeBound] {
        &self.bounds
    }

    pub fn aliased_ty(&self) -> Option<&TypeRef> {
        self.aliased_ty.as_ref()
    }
}

/// Const signature facts used by type cursors and hovers.
#[derive(Debug, Clone, PartialEq, Eq, Default, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct ConstSignature {
    pub(crate) ty: Option<TypeRef>,
}

impl ConstSignature {
    pub fn from_item(item: &ConstItem) -> Self {
        Self {
            ty: item.ty.clone(),
        }
    }

    pub fn ty(&self) -> Option<&TypeRef> {
        self.ty.as_ref()
    }
}
