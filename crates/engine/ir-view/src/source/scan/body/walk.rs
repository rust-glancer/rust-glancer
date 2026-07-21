//! Structural walkers used by body source scans.
//!
//! These helpers know how to move through local IR shapes, but they do not decide what a visited
//! node means for resolution, completion, or navigation. Query code keeps that policy close to
//! the query and uses these walkers only for reusable child traversal.

use rg_body_ir::{
    BodyPath, BodyPathSegment, BodyPathSegmentArgs, BodyPathSegmentKind, BodyView, PatData, PatKind,
};
use rg_ir_model::{PatId, ScopeId};
use rg_item_tree::{GenericArg, TypeRef};

/// One pattern node reached by structural pattern traversal.
///
/// Nested pattern nodes keep the root scope because Rust pattern bindings are introduced together
/// into the scope owned by the full pattern.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PatWalkSite<'body> {
    pub(crate) scope: ScopeId,
    pub(crate) data: &'body PatData,
}

/// Walks a lowered pattern root and all child pattern nodes.
///
/// For `User { name: Some(value), .. }`, this visits the record pattern, the `Some` tuple-struct
/// pattern, the `value` binding, and the rest pattern. Every visit keeps the scope of the complete
/// root pattern because those bindings become visible together.
pub(crate) fn walk_pat<'body>(
    body: BodyView<'body>,
    scope: ScopeId,
    pat: PatId,
    visit: &mut impl FnMut(PatWalkSite<'body>),
) {
    let Some(data) = body.pat(pat) else {
        return;
    };

    visit(PatWalkSite { scope, data });

    match &data.kind {
        PatKind::TupleStruct { fields, .. }
        | PatKind::Tuple { fields }
        | PatKind::Or { pats: fields }
        | PatKind::Slice { fields } => {
            for field in fields {
                walk_pat(body, scope, *field, visit);
            }
        }
        PatKind::Record { fields, rest, .. } => {
            for field in fields {
                walk_pat(body, scope, field.pat, visit);
            }
            if let Some(rest) = rest {
                walk_pat(body, scope, *rest, visit);
            }
        }
        PatKind::Binding {
            subpat: Some(subpat),
            ..
        }
        | PatKind::Ref { pat: subpat, .. }
        | PatKind::Box { pat: subpat } => {
            walk_pat(body, scope, *subpat, visit);
        }
        PatKind::Range { start, end, .. } => {
            if let Some(start) = start {
                walk_pat(body, scope, *start, visit);
            }
            if let Some(end) = end {
                walk_pat(body, scope, *end, visit);
            }
        }
        PatKind::Binding { subpat: None, .. }
        | PatKind::Path { .. }
        | PatKind::Rest
        | PatKind::Literal { .. }
        | PatKind::ConstBlock { .. }
        | PatKind::Wildcard
        | PatKind::Unsupported => {}
    }
}

/// Walks type references embedded in rich body-path syntax.
///
/// Body paths are value/type paths with body-only details such as turbofish arguments and
/// qualified type anchors. This walker yields the written type references inside those details;
/// callers can then decide how deeply to inspect each type reference.
///
/// For `<Vec<Item> as Trait<Arg>>::method::<Output>`, it yields `Vec<Item>`, `Trait<Arg>`, and
/// `Output`. Walking the nested `Item` and `Arg` paths is deliberately left to the shared
/// type-reference walker.
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
