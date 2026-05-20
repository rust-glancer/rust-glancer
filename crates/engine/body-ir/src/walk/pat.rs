use crate::{BodyData, PatData, PatId, PatKind, ScopeId};

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
pub(crate) fn walk_pat<'body>(
    body: &'body BodyData,
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
