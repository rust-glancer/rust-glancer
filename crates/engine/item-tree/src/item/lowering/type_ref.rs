use crate::item::{
    ConstExpr, GenericArg, TraitBoundModifier, TypeBound, TypePath, TypePathAnchor,
    TypePathSegment, TypeRef,
};
use rg_ir_model::Mutability;
use rg_parse::{LineIndex, Span};
use rg_syntax::{
    AstNode as _,
    ast::{self, HasGenericArgs},
};
use rg_text::NameInterner;

use super::{FromAst, normalized_syntax};

impl FromAst for TypeRef {
    type AstNode = ast::Type;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(ty: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        match ty.clone() {
            ast::Type::ArrayType(ty) => Self::Array {
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
                len: ty.const_arg().map(|arg| {
                    ConstExpr::new(
                        normalized_syntax(&arg),
                        Span::from_text_range(arg.syntax().text_range()),
                    )
                }),
            },
            ast::Type::DynTraitType(ty) => Self::DynTrait(type_bound_list_from_ast(
                ty.type_bound_list(),
                line_index,
                interner,
            )),
            ast::Type::FnPtrType(ty) => Self::FnPointer {
                params: ty
                    .param_list()
                    .into_iter()
                    .flat_map(|param_list| param_list.params())
                    .map(|param| {
                        param
                            .ty()
                            .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                            .unwrap_or_else(|| Self::Unknown(String::new()))
                    })
                    .collect(),
                ret: Box::new(
                    ty.ret_type()
                        .and_then(|ret_ty| ret_ty.ty())
                        .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                        .unwrap_or(Self::Unit),
                ),
            },
            ast::Type::ForType(ty) => ty
                .ty()
                .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::ImplTraitType(ty) => Self::ImplTrait(type_bound_list_from_ast(
                ty.type_bound_list(),
                line_index,
                interner,
            )),
            ast::Type::InferType(_) => Self::Infer,
            ast::Type::MacroType(ty) => Self::unknown_from_text(normalized_syntax(&ty)),
            ast::Type::NeverType(_) => Self::Never,
            ast::Type::ParenType(ty) => ty
                .ty()
                .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::PathType(ty) => ty
                .path()
                .map(|path| Self::Path(TypePath::from_ast(&path, (line_index, &mut *interner))))
                .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            ast::Type::PtrType(ty) => Self::RawPointer {
                mutability: Mutability::from_mut_token(ty.mut_token().is_some()),
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
            },
            ast::Type::RefType(ty) => Self::Reference {
                lifetime: ty
                    .lifetime()
                    .map(|lifetime| interner.intern(lifetime.text())),
                mutability: Mutability::from_mut_token(ty.mut_token().is_some()),
                inner: Box::new(
                    ty.ty()
                        .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                        .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
                ),
            },
            ast::Type::SliceType(ty) => Self::Slice(Box::new(
                ty.ty()
                    .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                    .unwrap_or_else(|| Self::unknown_from_text(normalized_syntax(&ty))),
            )),
            ast::Type::TupleType(ty) => {
                let fields = ty
                    .fields()
                    .map(|ty| Self::from_ast(&ty, (line_index, &mut *interner)))
                    .collect::<Vec<_>>();
                if fields.is_empty() {
                    Self::Unit
                } else {
                    Self::Tuple(fields)
                }
            }
        }
    }
}

impl FromAst for TypePath {
    type AstNode = ast::Path;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(path: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        let source_span = Span::from_text_range(path.syntax().text_range());
        let mut ast_segments = Vec::new();
        collect_ast_path_segments(path, &mut ast_segments);
        let mut anchor = None;
        let mut segment_start = 0;
        let mut absolute = ast_segments
            .first()
            .is_some_and(|segment| segment.coloncolon_token().is_some());

        // Rust stores `<T>::Assoc` and `<T as Trait>::Assoc` as a leading type path segment.
        // Keep that as a path anchor and let the rest of the path use ordinary named segments.
        if let Some(first_segment) = ast_segments.first()
            && let Some(anchor_from_segment) =
                type_path_anchor_from_ast(first_segment, line_index, &mut *interner)
        {
            anchor = Some(anchor_from_segment);
            segment_start = 1;
            absolute = false;
        }
        let segments = ast_segments
            .iter()
            .skip(segment_start)
            .map(|segment| type_path_segment_from_ast(segment, line_index, &mut *interner))
            .collect();

        Self {
            source_span,
            absolute,
            anchor,
            segments,
        }
    }
}

fn collect_ast_path_segments(path: &ast::Path, segments: &mut Vec<ast::PathSegment>) {
    if let Some(qualifier) = path.qualifier() {
        collect_ast_path_segments(&qualifier, segments);
    }

    if let Some(segment) = path.segment() {
        segments.push(segment);
    }
}

fn type_path_anchor_from_ast(
    segment: &ast::PathSegment,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> Option<TypePathAnchor> {
    let ast::PathSegmentKind::Type {
        type_ref,
        trait_ref,
    } = segment.kind()?
    else {
        return None;
    };

    let self_ty = type_ref
        .as_ref()
        .map(|ty| TypeRef::from_ast(ty, (line_index, &mut *interner)))?;

    let trait_ty = match trait_ref {
        Some(trait_ref) => Some(
            trait_ref
                .path()
                .map(|path| TypeRef::Path(TypePath::from_ast(&path, (line_index, interner))))?,
        ),
        None => None,
    };

    Some(TypePathAnchor::from_parts(self_ty, trait_ty))
}

fn type_path_segment_from_ast(
    segment: &ast::PathSegment,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> TypePathSegment {
    let name = segment
        .name_ref()
        .map(|name| interner.intern(name.text()))
        .unwrap_or_else(|| interner.intern_missing());
    let span = segment
        .name_ref()
        .map(|name| name.syntax().text_range())
        .unwrap_or_else(|| segment.syntax().text_range());
    let mut args = Vec::new();

    if let Some(arg_list) = segment.generic_arg_list() {
        args.extend(
            arg_list
                .generic_args()
                .map(|arg| GenericArg::from_ast(&arg, (line_index, &mut *interner))),
        );
    }

    if let Some(parenthesized_args) = segment.parenthesized_arg_list() {
        let params = parenthesized_args
            .type_args()
            .map(|arg| {
                arg.ty()
                    .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
                    .unwrap_or_else(|| TypeRef::unknown_from_text(normalized_syntax(&arg)))
            })
            .collect();
        let ret = segment
            .ret_type()
            .and_then(|ret_ty| ret_ty.ty())
            .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
            .unwrap_or(TypeRef::Unit);

        args.push(GenericArg::FnTraitArgs {
            params,
            ret: Box::new(ret),
        });
    }

    TypePathSegment {
        name,
        args,
        span: Span::from_text_range(span),
    }
}

impl FromAst for GenericArg {
    type AstNode = ast::GenericArg;
    type Context<'a> = (&'a LineIndex, &'a mut NameInterner);

    fn from_ast(arg: &Self::AstNode, (line_index, interner): Self::Context<'_>) -> Self {
        match arg.clone() {
            ast::GenericArg::AssocTypeArg(arg) => {
                let name_ref = arg.name_ref();
                Self::AssocType {
                    name: name_ref
                        .as_ref()
                        .map(|name| interner.intern(name.text()))
                        .unwrap_or_else(|| interner.intern_missing()),
                    name_span: name_ref
                        .as_ref()
                        .map(|name| Span::from_text_range(name.syntax().text_range()))
                        .unwrap_or_else(|| Span::from_text_range(arg.syntax().text_range())),
                    ty: arg
                        .ty()
                        .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
                }
            }
            ast::GenericArg::ConstArg(arg) => Self::Const(ConstExpr::new(
                normalized_syntax(&arg),
                Span::from_text_range(arg.syntax().text_range()),
            )),
            ast::GenericArg::LifetimeArg(arg) => arg
                .lifetime()
                .map(|lifetime| Self::Lifetime(interner.intern(lifetime.text())))
                .unwrap_or_else(|| Self::Unsupported(normalized_syntax(&arg))),
            ast::GenericArg::TypeArg(arg) => arg
                .ty()
                .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner)))
                .map(Self::Type)
                .unwrap_or_else(|| Self::Unsupported(normalized_syntax(&arg))),
        }
    }
}

pub(crate) fn type_bound_list_from_ast(
    bound_list: Option<ast::TypeBoundList>,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> Vec<TypeBound> {
    bound_list
        .into_iter()
        .flat_map(|bound_list| bound_list.bounds())
        .map(|bound| type_bound_from_ast(bound, line_index, interner))
        .collect()
}

fn type_bound_from_ast(
    bound: ast::TypeBound,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> TypeBound {
    if let Some(lifetime) = bound.lifetime() {
        return TypeBound::Lifetime(interner.intern(lifetime.text()));
    }

    if let Some(ty) = bound.ty() {
        let modifier = if bound.question_mark_token().is_some() {
            TraitBoundModifier::Maybe
        } else {
            TraitBoundModifier::None
        };
        return TypeBound::Trait {
            ty: TypeRef::from_ast(&ty, (line_index, interner)),
            modifier,
        };
    }

    TypeBound::Unsupported(normalized_syntax(&bound))
}
