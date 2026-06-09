use rg_std::UniqueVec;
use rg_ty::Ty;

pub(crate) fn unique_ty_or_unknown(tys: UniqueVec<Ty>) -> Ty {
    Ty::one_or_unknown(tys)
}
