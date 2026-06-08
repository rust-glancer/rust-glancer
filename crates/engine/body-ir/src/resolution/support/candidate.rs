use rg_ty::Ty;

pub(crate) fn unique_ty_or_unknown(tys: impl Into<Vec<Ty>>) -> Ty {
    let mut tys = tys.into();
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        Ty::Unknown
    }
}
