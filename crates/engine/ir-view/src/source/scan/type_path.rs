//! Scope-free traversal and cursor interpretation for written type paths.
//!
//! Body and item-signature scanners attach different semantic scopes to the same item-tree syntax.
//! This module keeps the syntax walk shared so a new type shape or recovered path form cannot be
//! supported by one scanner while silently remaining invisible to the other. It preserves both
//! module-compatible and type-shaped qualifiers, and classifies associated bindings before either
//! caller attaches a scope.
//!
//! ```text
//! fn load(value: outer::Inner<Vec<Item>>) {}
//!                ^^^^^^^^^^^ ^^^ ^^^^ the walk reports these three paths
//!
//! fn load(value: outer::Inn$0) {}
//!                ^^^^^^^ qualifier `outer`, replacement span `Inn`
//!
//! fn read(value: Iterator<Ite$0 = u8>) {}
//!                         ^^^ binding span; resolve the surrounding `Iterator`
//! ```

use rg_ir_model::Path;
use rg_item_tree::{GenericArg, TypePath, TypePathAnchor, TypeRef};
use rg_parse::{Span, TextSpan};

use super::{AssociatedPathQualifier, TypeNamePosition};

/// Cursor-shaped interpretation of one type path before a body or signature scope is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TypePathCompletionSite {
    /// A segment whose candidates come from a resolved qualifier, such as `model::Us$0`.
    Qualified {
        module_qualifier: Option<Path>,
        associated_qualifier: AssociatedPathQualifier,
        member_prefix_span: Span,
    },
    /// A first segment whose candidates come from lexical and module scopes, such as `Us$0`.
    Unqualified {
        member_prefix_span: Span,
        member_prefix: String,
        position: TypeNamePosition,
    },
}

/// Scope-free syntax interpretation of an associated type binding within a trait path.
///
/// `Iterator<It$0 = u8>` is not a normal type path segment: the replacement span belongs to the
/// binding name, while candidates come only from the resolved `Iterator` trait and its
/// supertraits.
///
/// There are deliberately two ways to build this value:
///
/// ```text
/// Iterator<It$0 = u8> // explicit: `=` proves that `It` is a binding
/// Iterator<It$0>      // implicit: `It` may still be an ordinary type argument
/// ```
///
/// The explicit form owns the completion site. The implicit form is only an extra interpretation
/// layered over normal type completion. A body or signature scanner attaches the semantic scope
/// after this syntax-only step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssociatedTypeBindingSyntax {
    pub(super) trait_ref: TypeRef,
    pub(super) member_prefix_span: Span,
    pub(super) existing_bindings: Vec<String>,
}

impl AssociatedTypeBindingSyntax {
    /// Interpret the associated binding that already has an `=` in source.
    ///
    /// All bindings are removed from the returned trait path. They constrain the trait use, but
    /// they do not participate in resolving which trait declaration owns the candidate names.
    pub(super) fn explicit_at(path: &TypePath, offset: u32) -> Option<Self> {
        for (segment_idx, segment) in path.segments.iter().enumerate() {
            let Some(touched_arg_idx) = segment.args.iter().position(|arg| {
                matches!(
                    arg,
                    GenericArg::AssocType { name_span, .. } if name_span.touches(offset)
                )
            }) else {
                continue;
            };
            let GenericArg::AssocType {
                name_span: member_prefix_span,
                ..
            } = &segment.args[touched_arg_idx]
            else {
                unreachable!("the selected generic argument is an associated type binding")
            };

            let existing_bindings = segment
                .args
                .iter()
                .enumerate()
                .filter_map(|(arg_idx, arg)| match arg {
                    GenericArg::AssocType { name, .. } if arg_idx != touched_arg_idx => {
                        Some(name.to_string())
                    }
                    GenericArg::Type(_)
                    | GenericArg::Lifetime(_)
                    | GenericArg::Const(_)
                    | GenericArg::FnTraitArgs { .. }
                    | GenericArg::AssocType { .. }
                    | GenericArg::Unsupported(_) => None,
                })
                .collect();

            // Associated bindings constrain the selected trait but are not part of resolving its
            // identity. Removing all of them also keeps an incomplete binding from feeding an
            // unknown right-hand side back into trait lowering.
            let mut segments = path.segments[..=segment_idx].to_vec();
            segments[segment_idx]
                .args
                .retain(|arg| !matches!(arg, GenericArg::AssocType { .. }));
            return Some(Self {
                trait_ref: TypeRef::Path(TypePath {
                    source_span: path.source_span,
                    absolute: path.absolute,
                    anchor: path.anchor.clone(),
                    segments,
                }),
                member_prefix_span: *member_prefix_span,
                existing_bindings,
            });
        }
        None
    }

    /// Interpret a simple type argument as a binding name before its `=` has been written.
    ///
    /// `Iterator<It$0>` is syntactically indistinguishable from an ordinary type argument. The
    /// candidate argument is removed from the trait path before resolution, and the caller uses
    /// this site only as an overlay while keeping normal type completions.
    pub(super) fn implicit_at(path: &TypePath, offset: u32) -> Option<Self> {
        for (segment_idx, segment) in path.segments.iter().enumerate() {
            for (arg_idx, arg) in segment.args.iter().enumerate() {
                let GenericArg::Type(TypeRef::Path(binding_path)) = arg else {
                    continue;
                };
                if binding_path.absolute
                    || binding_path.anchor.is_some()
                    || binding_path.segments.len() != 1
                {
                    continue;
                }
                let binding_segment = &binding_path.segments[0];
                if !binding_segment.args.is_empty() || !binding_segment.span.touches(offset) {
                    continue;
                }

                let existing_bindings = segment
                    .args
                    .iter()
                    .filter_map(|arg| match arg {
                        GenericArg::AssocType { name, .. } => Some(name.to_string()),
                        GenericArg::Type(_)
                        | GenericArg::Lifetime(_)
                        | GenericArg::Const(_)
                        | GenericArg::FnTraitArgs { .. }
                        | GenericArg::Unsupported(_) => None,
                    })
                    .collect();

                // Resolve the surrounding trait without treating the unfinished binding name as
                // one of its positional generic arguments.
                let mut segments = path.segments[..=segment_idx].to_vec();
                segments[segment_idx].args = segments[segment_idx]
                    .args
                    .iter()
                    .enumerate()
                    .filter(|(candidate_idx, candidate)| {
                        *candidate_idx != arg_idx
                            && !matches!(candidate, GenericArg::AssocType { .. })
                    })
                    .map(|(_, candidate)| candidate.clone())
                    .collect();
                return Some(Self {
                    trait_ref: TypeRef::Path(TypePath {
                        source_span: path.source_span,
                        absolute: path.absolute,
                        anchor: path.anchor.clone(),
                        segments,
                    }),
                    member_prefix_span: binding_segment.span,
                    existing_bindings,
                });
            }
        }
        None
    }
}

impl TypePathCompletionSite {
    /// Interprets the path segment touched by `offset`.
    ///
    /// In `outer::Inn$0`, `outer` is the qualifier and `Inn` is the replacement span. Recovered
    /// syntax such as `outer::$0` has no final segment, so it receives an empty replacement span
    /// after the separator instead.
    pub(super) fn at(path: &TypePath, offset: u32, position: TypeNamePosition) -> Option<Self> {
        for (idx, segment) in path.segments.iter().enumerate() {
            if !segment.span.touches(offset) {
                continue;
            }

            if idx == 0 {
                if path.anchor.is_some() {
                    return Some(Self::Qualified {
                        module_qualifier: None,
                        associated_qualifier: Self::associated_qualifier(path, 0)?,
                        member_prefix_span: segment.span,
                    });
                }
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
                module_qualifier: path.as_def_map_path_prefix(idx - 1),
                associated_qualifier: Self::associated_qualifier(path, idx)?,
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
            module_qualifier: path.as_def_map_path(),
            associated_qualifier: Self::associated_qualifier(path, path.segments.len())?,
            member_prefix_span: Span {
                text: TextSpan {
                    start: offset,
                    end: offset,
                },
            },
        })
    }

    /// Builds the type-shaped prefix while preserving anchors and generic arguments.
    fn associated_qualifier(
        path: &TypePath,
        prefix_segment_count: usize,
    ) -> Option<AssociatedPathQualifier> {
        if prefix_segment_count == 0 {
            return match path.anchor.as_ref()? {
                TypePathAnchor::Type(ty) => {
                    Some(AssociatedPathQualifier::Type(ty.as_ref().clone()))
                }
                TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                    Some(AssociatedPathQualifier::QualifiedTrait {
                        self_ty: self_ty.as_ref().clone(),
                        trait_ref: trait_ty.as_ref().clone(),
                    })
                }
            };
        }
        if prefix_segment_count > path.segments.len() {
            return None;
        }

        Some(AssociatedPathQualifier::Type(TypeRef::Path(TypePath {
            source_span: path.source_span,
            absolute: path.absolute,
            anchor: path.anchor.clone(),
            segments: path.segments[..prefix_segment_count].to_vec(),
        })))
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
