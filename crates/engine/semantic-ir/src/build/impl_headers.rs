//! Resolves impl headers after semantic item identities are available.

use rg_def_map::{DefMapDb, DefMapReadTxn, ModuleRef, PackageSlot, Path, TargetRef};
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_parse::TargetId;

use crate::{
    SemanticIrReadTxn,
    db::SemanticIrDbMutator,
    ids::{ImplRef, TraitRef, TypeDefRef},
};

pub(super) fn resolve_impl_headers(
    db: &mut SemanticIrDbMutator<'_>,
    def_map: &DefMapDb,
) -> Result<(), PackageStoreError> {
    let packages = (0..db.package_count()).map(PackageSlot).collect::<Vec<_>>();
    let def_map = def_map.read_txn(super::unexpected_package_loader());
    let semantic_ir = db.read_txn(super::unexpected_package_loader());
    let resolutions = impl_header_resolutions_for_packages(&semantic_ir, &def_map, &packages)?;

    drop(semantic_ir);
    apply_impl_header_resolutions(db, resolutions);
    Ok(())
}

pub(super) struct ImplHeaderResolution {
    impl_ref: ImplRef,
    resolved_self_tys: Vec<TypeDefRef>,
    resolved_trait_refs: Vec<TraitRef>,
}

pub(super) fn impl_header_resolutions_for_packages(
    semantic_ir: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    packages: &[PackageSlot],
) -> Result<Vec<ImplHeaderResolution>, PackageStoreError> {
    let mut resolutions = Vec::new();

    for package in packages {
        let package_ir = semantic_ir.package(*package)?;

        for (target_idx, _) in package_ir.into_ref().targets().iter().enumerate() {
            let target = TargetRef {
                package: *package,
                target: TargetId(target_idx),
            };
            for (impl_ref, _) in semantic_ir.impls(target)? {
                let Some(data) = semantic_ir.impl_data(impl_ref)? else {
                    continue;
                };

                let resolved_self_tys =
                    resolve_type_defs_from_ref(semantic_ir, def_map, data.owner, &data.self_ty)?;
                let resolved_trait_refs = data
                    .trait_ref
                    .as_ref()
                    .map(|ty| resolve_traits_from_ref(semantic_ir, def_map, data.owner, ty))
                    .transpose()?
                    .unwrap_or_default();

                resolutions.push(ImplHeaderResolution {
                    impl_ref,
                    resolved_self_tys,
                    resolved_trait_refs,
                });
            }
        }
    }

    Ok(resolutions)
}

pub(super) fn apply_impl_header_resolutions(
    db: &mut SemanticIrDbMutator<'_>,
    resolutions: Vec<ImplHeaderResolution>,
) {
    for resolution in resolutions {
        let Some(data) = db.impl_data_mut(resolution.impl_ref) else {
            continue;
        };
        data.resolved_self_tys = resolution.resolved_self_tys;
        data.resolved_trait_refs = resolution.resolved_trait_refs;
    }
}

fn resolve_type_defs_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<Vec<TypeDefRef>, PackageStoreError> {
    let Some(path) = Path::from_type_ref(ty) else {
        return Ok(Vec::new());
    };

    db.type_defs_for_path(def_map, owner, &path)
}

fn resolve_traits_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<Vec<TraitRef>, PackageStoreError> {
    let Some(path) = Path::from_type_ref(ty) else {
        return Ok(Vec::new());
    };

    db.traits_for_path(def_map, owner, &path)
}
