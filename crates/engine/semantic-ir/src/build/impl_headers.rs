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

use std::collections::HashMap;

use crate::ItemResolutionQuery;
use crate::ItemStoreQuery;
use rg_def_map::{DefMapReadTxn, PackageSlot};
use rg_ir_model::{CrateId, CrateRef, ImplRef, ModuleRef, PrimitiveTy, TraitDefRef, TypeDefRef};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_std::ExpectedUnique;

use crate::{SemanticIrReadTxn, TraitImplSelfHead, TypePathContext, store::SemanticIrDbMutator};

/// Named identities and a conservative lookup key collected for one impl declaration.
///
/// Keeping these facts separate from the mutable database lets the pass finish all reads before
/// it rebuilds indexes. For an inherent `impl Widget`, `resolved_trait_ref` and `self_head` are
/// empty. For `impl<T> Paint for [T]`, the named trait resolves while `resolved_self_ty` remains
/// empty and `self_head` records `Slice`.
pub(super) struct ImplHeaderResolution {
    impl_ref: ImplRef,
    resolved_self_ty: ExpectedUnique<TypeDefRef>,
    resolved_trait_ref: ExpectedUnique<TraitDefRef>,
    self_head: Option<TraitImplSelfHead>,
}

/// Collect every impl's resolvable header facts without mutating the database being read.
///
/// Name resolution needs the complete semantic item graph, so this phase runs after initial item
/// lowering. The returned batch is applied only after the read transaction has ended.
pub(super) fn impl_header_resolutions_for_packages(
    semantic_ir: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    packages: &[PackageSlot],
) -> Result<Vec<ImplHeaderResolution>, PackageStoreError> {
    let mut resolutions = Vec::new();
    let item_query = ItemStoreQuery::new(semantic_ir);

    for package in packages {
        let package_ir = semantic_ir.package(*package)?;

        for (crate_idx, _) in package_ir.crates().iter().enumerate() {
            let crate_ref = CrateRef {
                package: *package,
                crate_id: CrateId(crate_idx),
            };
            for (impl_ref, _) in semantic_ir
                .items(crate_ref)?
                .into_iter()
                .flat_map(|i| i.impls_with_refs())
            {
                let Some(data) = item_query.impl_data(impl_ref)? else {
                    continue;
                };

                let resolved_self_ty =
                    resolve_type_defs_from_ref(semantic_ir, def_map, data.owner, &data.self_ty)?;
                let resolved_trait_ref = data
                    .trait_ref
                    .as_ref()
                    .map(|ty| resolve_traits_from_ref(semantic_ir, def_map, data.owner, ty))
                    .transpose()?
                    .unwrap_or_default();
                let self_head = data
                    .trait_ref
                    .as_ref()
                    .map(|_| {
                        trait_impl_self_head(
                            semantic_ir,
                            def_map,
                            impl_ref,
                            data,
                            &resolved_self_ty,
                        )
                    })
                    .transpose()?
                    .flatten();

                resolutions.push(ImplHeaderResolution {
                    impl_ref,
                    resolved_self_ty,
                    resolved_trait_ref,
                    self_head,
                });
            }
        }
    }

    Ok(resolutions)
}

/// Store resolved header identities and return the self-head keys needed for index rebuilding.
///
/// The database owns the full resolved `Self` and trait facts. The returned map is intentionally
/// temporary: it carries the smaller trait-impl routing key only across the boundary where each
/// crate-local [`crate::ItemLookupIndex`] is rebuilt.
pub(super) fn apply_impl_header_resolutions(
    db: &mut SemanticIrDbMutator<'_>,
    resolutions: Vec<ImplHeaderResolution>,
) -> HashMap<ImplRef, TraitImplSelfHead> {
    let mut self_heads = HashMap::with_capacity(resolutions.len());
    for resolution in resolutions {
        if let Some(self_head) = resolution.self_head {
            self_heads.insert(resolution.impl_ref, self_head);
        }
        let _ = db.set_impl_header_facts(
            resolution.impl_ref,
            resolution.resolved_self_ty,
            resolution.resolved_trait_ref,
        );
    }
    self_heads
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
/// its primitive head would require alias normalization. Native proof can still normalize and
/// accept it later; the declaration index merely declines to put it in the direct `u32` lane.
fn trait_impl_self_head(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    impl_ref: ImplRef,
    data: &crate::ImplData,
    resolved_self_ty: &ExpectedUnique<TypeDefRef>,
) -> Result<Option<TraitImplSelfHead>, PackageStoreError> {
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
                    impl_ref: Some(impl_ref),
                };
                let declarations = ItemResolutionQuery::new(def_map, db)
                    .semantic_items_for_type_path(context, &path)?;
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

fn resolve_type_defs_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TypeDefRef>, PackageStoreError> {
    let Some(path) = ty.as_def_map_path() else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for type_def in ItemResolutionQuery::new(def_map, db).type_defs_for_path(owner, &path)? {
        result.push(type_def);
    }
    Ok(result)
}

fn resolve_traits_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TraitDefRef>, PackageStoreError> {
    let Some(path) = ty.as_def_map_path() else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for trait_ref in ItemResolutionQuery::new(def_map, db).traits_for_path(owner, &path)? {
        result.push(trait_ref);
    }
    Ok(result)
}
