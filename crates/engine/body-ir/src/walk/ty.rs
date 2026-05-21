use rg_item_tree::{GenericArg, TypeBound, TypePath, TypeRef};

/// Walks every path node nested inside a type reference.
///
/// The outer path is visited before paths inside its generic arguments, matching the order a reader
/// sees in syntax such as `Outer<Inner>`.
pub(crate) fn walk_type_ref_paths<'ty>(ty: &'ty TypeRef, visit: &mut impl FnMut(&'ty TypePath)) {
    match ty {
        TypeRef::Path(path) => {
            visit(path);

            for segment in &path.segments {
                for arg in &segment.args {
                    match arg {
                        GenericArg::Type(ty) => walk_type_ref_paths(ty, visit),
                        GenericArg::AssocType { ty: Some(ty), .. } => {
                            walk_type_ref_paths(ty, visit);
                        }
                        GenericArg::Lifetime(_)
                        | GenericArg::Const(_)
                        | GenericArg::AssocType { ty: None, .. }
                        | GenericArg::Unsupported(_) => {}
                    }
                }
            }
        }
        TypeRef::Tuple(types) => {
            for ty in types {
                walk_type_ref_paths(ty, visit);
            }
        }
        TypeRef::Reference { inner, .. }
        | TypeRef::RawPointer { inner, .. }
        | TypeRef::Slice(inner) => walk_type_ref_paths(inner, visit),
        TypeRef::Array { inner, .. } => walk_type_ref_paths(inner, visit),
        TypeRef::FnPointer { params, ret } => {
            for param in params {
                walk_type_ref_paths(param, visit);
            }
            walk_type_ref_paths(ret, visit);
        }
        TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
            for bound in bounds {
                if let TypeBound::Trait(ty) = bound {
                    walk_type_ref_paths(ty, visit);
                }
            }
        }
        TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
    }
}
