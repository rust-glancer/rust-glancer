//! Resolves impl headers after semantic item identities are available.

use rg_def_map::{DefMapDb, DefMapReadTxn, PackageSlot};
use rg_ir_model::Path;
use rg_ir_model::{ImplRef, ModuleRef, TargetRef, TraitRef, TypeDefRef};
use rg_ir_storage::ItemStoreQuery;
use rg_item_tree::TypeRef;
use rg_package_store::PackageStoreError;
use rg_parse::TargetId;
use rg_std::ExpectedUnique;
use rg_ty::ItemPathQuery;

use crate::{SemanticIrReadTxn, store::SemanticIrDbMutator};

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
    resolved_self_ty: ExpectedUnique<TypeDefRef>,
    resolved_trait_ref: ExpectedUnique<TraitRef>,
}

pub(super) fn impl_header_resolutions_for_packages(
    semantic_ir: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    packages: &[PackageSlot],
) -> Result<Vec<ImplHeaderResolution>, PackageStoreError> {
    let mut resolutions = Vec::new();
    let item_query = ItemStoreQuery::new(semantic_ir);

    for package in packages {
        let package_ir = semantic_ir.package(*package)?;

        for (target_idx, _) in package_ir.targets().iter().enumerate() {
            let target = TargetRef {
                package: *package,
                target: TargetId(target_idx),
            };
            for (impl_ref, _) in semantic_ir
                .items(target)?
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

                resolutions.push(ImplHeaderResolution {
                    impl_ref,
                    resolved_self_ty,
                    resolved_trait_ref,
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
        let _ = db.set_impl_header_facts(
            resolution.impl_ref,
            resolution.resolved_self_ty,
            resolution.resolved_trait_ref,
        );
    }
}

fn resolve_type_defs_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TypeDefRef>, PackageStoreError> {
    let Some(path) = Path::from_type_ref(ty) else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for type_def in ItemPathQuery::new(def_map, db).type_defs_for_path(owner, &path)? {
        result.push(type_def);
    }
    Ok(result)
}

fn resolve_traits_from_ref(
    db: &SemanticIrReadTxn<'_>,
    def_map: &DefMapReadTxn<'_>,
    owner: ModuleRef,
    ty: &TypeRef,
) -> Result<ExpectedUnique<TraitRef>, PackageStoreError> {
    let Some(path) = Path::from_type_ref(ty) else {
        return Ok(ExpectedUnique::new());
    };

    let mut result = ExpectedUnique::new();
    for trait_ref in ItemPathQuery::new(def_map, db).traits_for_path(owner, &path)? {
        result.push(trait_ref);
    }
    Ok(result)
}
