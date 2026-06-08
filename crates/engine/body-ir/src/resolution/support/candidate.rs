use rg_ty::Ty;

pub(crate) fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    // Resolution often merges candidates from local, inherent, and trait sources. Keeping order
    // while deduplicating makes snapshots stable without pretending this is a ranking policy.
    if !items.contains(&item) {
        items.push(item);
    }
}

pub(crate) fn unique_ty_or_unknown(mut tys: Vec<Ty>) -> Ty {
    if tys.len() == 1 {
        tys.pop().expect("one type should exist")
    } else {
        Ty::Unknown
    }
}
