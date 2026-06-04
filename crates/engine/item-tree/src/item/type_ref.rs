use rg_ir_model::items::{GenericArg, Mutability, TypeBound, TypePath, TypePathSegment, TypeRef};
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
                len: ty.const_arg().map(|arg| normalized_syntax(&arg)),
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
                .map(|path| TypePath::from_ast(&path, (line_index, &mut *interner)))
                .map(Self::Path)
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
                lifetime: ty.lifetime().map(|lifetime| normalized_syntax(&lifetime)),
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
        let absolute = path
            .first_segment()
            .is_some_and(|segment| segment.coloncolon_token().is_some());
        let mut segments = Vec::new();
        collect_segments(&path, line_index, interner, &mut segments);

        Self {
            source_span,
            absolute,
            segments,
        }
    }
}

fn collect_segments(
    path: &ast::Path,
    line_index: &LineIndex,
    interner: &mut NameInterner,
    segments: &mut Vec<TypePathSegment>,
) {
    if let Some(qualifier) = path.qualifier() {
        collect_segments(&qualifier, line_index, interner, segments);
    }

    if let Some(segment) = path.segment() {
        segments.push(type_path_segment_from_ast(&segment, line_index, interner));
    }
}

fn type_path_segment_from_ast(
    segment: &ast::PathSegment,
    line_index: &LineIndex,
    interner: &mut NameInterner,
) -> TypePathSegment {
    let name = segment
        .name_ref()
        .map(|name| interner.intern(name.syntax().text().to_string().trim()))
        .unwrap_or_else(|| interner.intern(normalized_syntax(segment)));
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
        args.push(GenericArg::Unsupported(normalized_syntax(
            &parenthesized_args,
        )));
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
            ast::GenericArg::AssocTypeArg(arg) => Self::AssocType {
                name: arg
                    .name_ref()
                    .map(|name| interner.intern(name.syntax().text().to_string()))
                    .unwrap_or_else(|| interner.intern("<missing>")),
                ty: arg
                    .ty()
                    .map(|ty| TypeRef::from_ast(&ty, (line_index, &mut *interner))),
            },
            ast::GenericArg::ConstArg(arg) => Self::Const(normalized_syntax(&arg)),
            ast::GenericArg::LifetimeArg(arg) => arg
                .lifetime()
                .map(|lifetime| Self::Lifetime(normalized_syntax(&lifetime)))
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
        return TypeBound::Lifetime(normalized_syntax(&lifetime));
    }

    if let Some(ty) = bound.ty() {
        return TypeBound::Trait(TypeRef::from_ast(&ty, (line_index, interner)));
    }

    TypeBound::Unsupported(normalized_syntax(&bound))
}
