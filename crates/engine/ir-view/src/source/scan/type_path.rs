//! Scope-free traversal and cursor interpretation for written type paths.
//!
//! Body and item-signature scanners attach different semantic scopes to the same item-tree syntax.
//! This module keeps the syntax walk shared so a new type shape or recovered path form cannot be
//! supported by one scanner while silently remaining invisible to the other.
//!
//! ```text
//! fn load(value: outer::Inner<Vec<Item>>) {}
//!                ^^^^^^^^^^^ ^^^ ^^^^ the walk reports these three paths
//!
//! fn load(value: outer::Inn$0) {}
//!                ^^^^^^^ qualifier `outer`, replacement span `Inn`
//! ```

use rg_ir_model::Path;
use rg_item_tree::{GenericArg, TypePath, TypePathAnchor, TypeRef};
use rg_parse::{Span, TextSpan};

use super::TypeNamePosition;

/// Cursor-shaped interpretation of one type path before a body or signature scope is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TypePathCompletionSite {
    /// A segment whose candidates come from a resolved qualifier, such as `model::Us$0`.
    Qualified {
        qualifier: Path,
        member_prefix_span: Span,
    },
    /// A first segment whose candidates come from lexical and module scopes, such as `Us$0`.
    Unqualified {
        member_prefix_span: Span,
        member_prefix: String,
        position: TypeNamePosition,
    },
}

impl TypePathCompletionSite {
    /// Interprets the path segment touched by `offset`.
    ///
    /// In `outer::Inn$0`, `outer` is the qualifier and `Inn` is the replacement span. Recovered
    /// syntax such as `outer::$0` has no final segment, so it receives an empty replacement span
    /// after the separator instead.
    pub(super) fn at(path: &TypePath, offset: u32, position: TypeNamePosition) -> Option<Self> {
        // The segments after `<Type as Trait>` are associated-item paths, not ordinary module
        // paths. Their completion needs associated-item lookup and is intentionally left out
        // until that query exists.
        if path.anchor.is_some() {
            return None;
        }

        for (idx, segment) in path.segments.iter().enumerate() {
            if !segment.span.touches(offset) {
                continue;
            }

            if idx == 0 {
                if path.absolute {
                    return None;
                }
                return Some(Self::Unqualified {
                    member_prefix_span: segment.span,
                    member_prefix: identifier_prefix_at(
                        segment.name.as_str(),
                        segment.span,
                        offset,
                    ),
                    position,
                });
            }

            return Some(Self::Qualified {
                qualifier: path.as_def_map_path_prefix(idx - 1)?,
                member_prefix_span: segment.span,
            });
        }

        let last_segment = path.segments.last()?;
        // Generic argument text also extends past the segment name. That suffix must not be
        // mistaken for the synthetic empty segment recovered after a trailing `::`.
        if !last_segment.args.is_empty()
            || path.source_span.text.end != last_segment.span.text.end + 2
            || !(last_segment.span.text.end..=path.source_span.text.end).contains(&offset)
        {
            return None;
        }

        Some(Self::Qualified {
            qualifier: path.as_def_map_path()?,
            member_prefix_span: Span {
                text: TextSpan {
                    start: offset,
                    end: offset,
                },
            },
        })
    }
}

/// Returns the source prefix before the cursor without splitting a UTF-8 character.
///
/// Lowering stores the complete identifier from `Na$0me`; completion needs only `Na`, because that
/// is the text the accepted completion will replace.
pub(super) fn identifier_prefix_at(name: &str, span: Span, offset: u32) -> String {
    // Lowering retains the complete identifier, while completion only replaces what the user has
    // typed before the cursor.
    let end = offset.saturating_sub(span.text.start).min(span.len());
    let mut end = usize::try_from(end).unwrap_or(name.len());
    while !name.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    name.get(..end).unwrap_or(name).to_string()
}

/// Walks every path node nested inside a type reference.
///
/// The outer path is visited before paths inside its generic arguments, matching the order a reader
/// sees in syntax such as `Outer<Inner>`. The visit also says whether an unqualified path occupies
/// a whole generic argument. That distinction matters for `Array<N$0>`: before `Array` resolves,
/// `N` may be either a type parameter or a const parameter. Structured arguments such as `&N` or
/// `Wrapper<N>` put their inner paths back in an ordinary type position.
pub(super) fn walk_type_ref_paths<'ty>(
    ty: &'ty TypeRef,
    position: TypeNamePosition,
    visit: &mut impl FnMut(&'ty TypePath, TypeNamePosition),
) {
    match ty {
        TypeRef::Path(path) => {
            visit(path, position.for_path(path));

            if let Some(anchor) = &path.anchor {
                walk_type_path_anchor(anchor, visit);
            }

            for segment in &path.segments {
                for arg in &segment.args {
                    match arg {
                        GenericArg::Type(ty) => {
                            walk_type_ref_paths(ty, TypeNamePosition::BareGenericArgument, visit)
                        }
                        GenericArg::FnTraitArgs { params, ret } => {
                            for param in params {
                                walk_type_ref_paths(param, TypeNamePosition::Type, visit);
                            }
                            walk_type_ref_paths(ret, TypeNamePosition::Type, visit);
                        }
                        GenericArg::AssocType { ty: Some(ty), .. } => {
                            walk_type_ref_paths(ty, TypeNamePosition::Type, visit);
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
                walk_type_ref_paths(ty, TypeNamePosition::Type, visit);
            }
        }
        TypeRef::Reference { inner, .. }
        | TypeRef::RawPointer { inner, .. }
        | TypeRef::Slice(inner) => walk_type_ref_paths(inner, TypeNamePosition::Type, visit),
        TypeRef::Array { inner, .. } => walk_type_ref_paths(inner, TypeNamePosition::Type, visit),
        TypeRef::FnPointer { params, ret } => {
            for param in params {
                walk_type_ref_paths(param, TypeNamePosition::Type, visit);
            }
            walk_type_ref_paths(ret, TypeNamePosition::Type, visit);
        }
        TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
            for bound in bounds {
                if let Some(ty) = bound.trait_ty() {
                    walk_type_ref_paths(ty, TypeNamePosition::Type, visit);
                }
            }
        }
        TypeRef::Unknown(_) | TypeRef::Never | TypeRef::Unit | TypeRef::Infer => {}
    }
}

fn walk_type_path_anchor<'ty>(
    anchor: &'ty TypePathAnchor,
    visit: &mut impl FnMut(&'ty TypePath, TypeNamePosition),
) {
    match anchor {
        TypePathAnchor::Type(ty) => walk_type_ref_paths(ty, TypeNamePosition::Type, visit),
        TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
            walk_type_ref_paths(self_ty, TypeNamePosition::Type, visit);
            walk_type_ref_paths(trait_ty, TypeNamePosition::Type, visit);
        }
    }
}
