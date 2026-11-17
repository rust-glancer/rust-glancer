//! Finishes the parts of an impl header that need named semantic items.
//!
//! Initial item lowering keeps source-shaped headers such as `impl<T> Paint for Vec<T>`. Once all
//! declarations have identities, this pass can resolve `Paint` and `Vec`, and can record the outer
//! shape of the trait impl's `Self` type:
//!
//! ```text
//! impl<T> Paint for Vec<T>  -> trait `Paint`, type `Vec`, self head `Adt(Vec)`
//! impl<T> Paint for [T]     -> trait `Paint`, no named type, self head `Slice`
//! impl<T> Paint for T       -> trait `Paint`, no named type, no direct self head
//! ```
//!
//! The last value is only a lookup hint. Type lowering and trait selection still compare the full
//! header later; this pass does not try to prove that an impl applies.

use rg_def_map::DefMapSource;
use rg_ir_model::{ImplRef, ModuleRef, PrimitiveTy, TraitDefRef, TypeDefRef};
use rg_item_tree::TypeRef;
use rg_source::ReadError;
use rg_std::ExpectedUnique;

use crate::{
    ItemResolutionQuery, ItemStore, SelfTypeOwner, SemanticItemSource, TraitImplSelfHead,
    TypePathContext,
};

/// Named identities and a conservative lookup key collected for one impl declaration.
///
/// Keeping these facts separate from the mutable database lets the pass finish all reads before
/// it rebuilds indexes. For an inherent `impl Widget`, `resolved_trait_ref` and `self_head` are
/// empty. For `impl<T> Paint for [T]`, the named trait resolves while `resolved_self_ty` remains
/// empty and `self_head` records `Slice`.
pub(crate) struct ImplHeaderResolution {
    pub(crate) impl_ref: ImplRef,
    pub(crate) resolved_self_ty: ExpectedUnique<TypeDefRef>,
    pub(crate) resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    pub(crate) self_head: Option<TraitImplSelfHead>,
}

/// Resolve one target's impl headers against its local items and acquired dependency declarations.
pub(crate) fn impl_header_resolutions<'a, D, I>(
    items: &ItemStore,
    query: &ItemResolutionQuery<'a, D, I>,
) -> Result<Vec<ImplHeaderResolution>, ReadError>
where
    D: DefMapSource,
    I: SemanticItemSource<'a>,
{
    let mut resolutions = Vec::new();
    for (impl_ref, data) in items.impls_with_refs() {
        let resolved_self_ty = resolve_type_defs_from_ref(query, data.owner, &data.self_ty)?;
        let resolved_trait_ref = data
            .trait_ref
            .as_ref()
            .map(|ty| resolve_traits_from_ref(query, data.owner, ty))
            .transpose()?
            .unwrap_or_default();
        let self_head = data
            .trait_ref
            .as_ref()
            .map(|_| trait_impl_self_head(query, impl_ref, data, &resolved_self_ty))
            .transpose()?
            .flatten();
        resolutions.push(ImplHeaderResolution {
            impl_ref,
            resolved_self_ty,
            resolved_trait_ref,
            self_head,
        });
    }
    Ok(resolutions)
}

/// Classify only the outer `Self` syntax that Semantic IR can establish without type lowering.
///
/// This step answers a deliberately small question: which candidate lane can safely contain this
/// declaration? It preserves only the information needed to reject unrelated outer shapes:
///
/// ```text
/// impl Trait for (u8, bool) -> Tuple(2)
/// impl<T> Trait for [T; 4]  -> Array
/// impl Trait for &mut u8    -> Reference(Mutable)
/// impl<T> Trait for Vec<T>  -> Adt(Vec)
/// ```
///
/// Element types, array lengths, and generic arguments are left for the exact type-layer matcher.
/// `type Alias = u32; impl Trait for Alias` deliberately remains a fallback because discovering
/// its primitive head would require alias normalization. Trait solving can still normalize and
/// accept it later; the declaration index merely declines to put it in the direct `u32` lane.
fn trait_impl_self_head<'a, D, I>(
    query: &ItemResolutionQuery<'a, D, I>,
    impl_ref: ImplRef,
    data: &crate::ImplData,
    resolved_self_ty: &ExpectedUnique<TypeDefRef>,
) -> Result<Option<TraitImplSelfHead>, ReadError>
where
    D: DefMapSource,
    I: SemanticItemSource<'a>,
{
    let head = match &data.self_ty {
        TypeRef::Unit => Some(TraitImplSelfHead::Unit),
        TypeRef::Never => Some(TraitImplSelfHead::Never),
        TypeRef::Tuple(fields) => u32::try_from(fields.len())
            .ok()
            .map(TraitImplSelfHead::Tuple),
        TypeRef::Array { .. } => Some(TraitImplSelfHead::Array),
        TypeRef::Slice(_) => Some(TraitImplSelfHead::Slice),
        TypeRef::Reference { mutability, .. } => Some(TraitImplSelfHead::Reference(*mutability)),
        TypeRef::RawPointer { mutability, .. } => Some(TraitImplSelfHead::RawPointer(*mutability)),
        TypeRef::FnPointer { params, .. } => u32::try_from(params.len())
            .ok()
            .map(TraitImplSelfHead::FnPointer),
        TypeRef::Path(path) => {
            if let Some(type_def) = resolved_self_ty.as_option() {
                Some(TraitImplSelfHead::Adt(*type_def))
            } else if !resolved_self_ty.is_empty() {
                // Several nominal definitions matched the path, so no exact direct lane is safe.
                None
            } else if !path.has_generic_args()
                && let Some(name) = path.single_name()
                && !data.generics.type_param_names().any(|param| param == name)
                && let Some(primitive) = PrimitiveTy::from_name(name.as_str())
                && let Some(path) = data.self_ty.as_def_map_path()
            {
                let context = TypePathContext {
                    module: data.owner,
                    self_owner: Some(SelfTypeOwner::Impl(impl_ref)),
                };
                let declarations = query.semantic_items_for_type_path(context, &path)?;
                declarations
                    .is_empty()
                    .then_some(TraitImplSelfHead::Primitive(primitive))
            } else {
                None
            }
        }
        TypeRef::Unknown(_) | TypeRef::Infer | TypeRef::ImplTrait(_) | TypeRef::DynTrait(_) => None,
    };

    Ok(head)
}

fn resolve_type_defs_from_ref<'a, D, I>(
    query: &ItemResolutionQuery<'a, D, I>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TypeDefRef>, ReadError>
where
    D: DefMapSource,
    I: SemanticItemSource<'a>,
{
    let Some(path) = ty.as_def_map_path() else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for type_def in query.type_defs_for_path(owner, &path)? {
        result.push(type_def);
    }
    Ok(result)
}

fn resolve_traits_from_ref<'a, D, I>(
    query: &ItemResolutionQuery<'a, D, I>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TraitDefRef>, ReadError>
where
    D: DefMapSource,
    I: SemanticItemSource<'a>,
{
    let Some(path) = ty.as_def_map_path() else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for trait_ref in query.traits_for_path(owner, &path)? {
        result.push(trait_ref);
    }
    Ok(result)
}
