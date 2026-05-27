pub type BodyTy = rg_ty::IndexedTy;
pub type BodyGenericArg = rg_ty::IndexedGenericArg;
pub(crate) type BodyTypeSubst = rg_ty::IndexedTypeSubst;
pub type BodyTyRepr = rg_ty::IndexedTyRepr;
pub type BodyLocalNominalTy = rg_ty::IndexedLocalNominalTy;
pub type BodyNominalTy = rg_ty::IndexedNominalTy;
pub use rg_ty::IndexedTyExt as BodyTyExt;
