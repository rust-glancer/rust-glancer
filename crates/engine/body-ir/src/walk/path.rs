use rg_ir_model::{
    BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind,
    items::{GenericArg, TypeRef},
};

/// Walks type references embedded in rich body-path syntax.
///
/// Body paths are value/type paths with body-only details such as turbofish arguments and
/// qualified type anchors. This walker yields the written type references inside those details;
/// callers can then decide how deeply to inspect each type reference.
pub(crate) fn walk_body_path_type_refs<'path>(
    path: &'path BodyPath,
    visit: &mut impl FnMut(&'path TypeRef),
) {
    for segment in path.segments() {
        walk_segment_type_refs(segment, visit);
    }
}

fn walk_segment_type_refs<'path>(
    segment: &'path BodyPathSegment,
    visit: &mut impl FnMut(&'path TypeRef),
) {
    if let BodyPathSegmentKind::TypeAnchor { ty, trait_ref } = segment.kind() {
        if let Some(ty) = ty {
            visit(ty);
        }
        if let Some(trait_ref) = trait_ref {
            visit(trait_ref);
        }
    }

    if let Some(args) = segment.args() {
        walk_segment_args_type_refs(args, visit);
    }
}

fn walk_segment_args_type_refs<'path>(
    args: &'path BodyPathSegmentArgs,
    visit: &mut impl FnMut(&'path TypeRef),
) {
    let BodyPathSegmentArgs::Angle { args, .. } = args else {
        return;
    };

    walk_generic_args_type_refs(args, visit);
}

pub(crate) fn walk_generic_args_type_refs<'path>(
    args: &'path [GenericArg],
    visit: &mut impl FnMut(&'path TypeRef),
) {
    for arg in args {
        walk_generic_arg_type_refs(arg, visit);
    }
}

fn walk_generic_arg_type_refs<'path>(
    arg: &'path GenericArg,
    visit: &mut impl FnMut(&'path TypeRef),
) {
    match arg {
        GenericArg::Type(ty) => visit(ty),
        GenericArg::FnTraitArgs { params, ret } => {
            for param in params {
                visit(param);
            }
            visit(ret);
        }
        GenericArg::AssocType { ty: Some(ty), .. } => visit(ty),
        GenericArg::Lifetime(_)
        | GenericArg::Const(_)
        | GenericArg::AssocType { ty: None, .. }
        | GenericArg::Unsupported(_) => {}
    }
}
