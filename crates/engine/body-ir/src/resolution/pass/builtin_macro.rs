//! Type facts for compiler-provided expression macros.
//!
//! Def-map marks resolved compiler builtin macro definitions before Body IR lowers them. Body
//! resolution only needs the conservative type fact for that lowered builtin expression, so this
//! module keeps the synthetic type construction out of the general expression walker.

use rg_ir_model::{
    BuiltinMacroExprKind, ExprId, Span, TextSpan,
    items::{GenericArg as ItemGenericArg, Mutability, TypePath, TypePathSegment, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_text::Name;
use rg_ty::{PrimitiveTy, Ty, UnsignedIntTy};

use crate::resolution::{BodyResolutionContext, TypeRefUseSite};

/// Maps a recognized builtin expression macro to the type Body IR should expose for it.
pub(super) struct BuiltinMacroExprTypeMapper<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BuiltinMacroExprTypeMapper<'query, D, I> {
    pub(super) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }
}

impl<'query, D, I> BuiltinMacroExprTypeMapper<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(super) fn ty_for(
        &self,
        expr: ExprId,
        kind: BuiltinMacroExprKind,
    ) -> Result<Ty, PackageStoreError> {
        match kind {
            BuiltinMacroExprKind::Cfg => Ok(Ty::Primitive(PrimitiveTy::Bool)),
            BuiltinMacroExprKind::Column | BuiltinMacroExprKind::Line => {
                Ok(Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U32)))
            }
            BuiltinMacroExprKind::Concat
            | BuiltinMacroExprKind::Env
            | BuiltinMacroExprKind::File
            | BuiltinMacroExprKind::IncludeStr
            | BuiltinMacroExprKind::ModulePath
            | BuiltinMacroExprKind::Stringify => Ok(Self::static_str_ty()),
            BuiltinMacroExprKind::IncludeBytes => Ok(Ty::reference(
                Mutability::Shared,
                Ty::slice(Ty::Primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))),
            )),
            BuiltinMacroExprKind::FormatArgs | BuiltinMacroExprKind::FormatArgsNl => {
                self.fmt_arguments_ty(expr)
            }
            BuiltinMacroExprKind::OptionEnv => self.option_env_ty(expr),
        }
    }

    fn fmt_arguments_ty(&self, expr: ExprId) -> Result<Ty, PackageStoreError> {
        self.resolve_synthetic_type_ref(
            expr,
            self.synthetic_type_path(expr, &["core", "fmt", "Arguments"], Vec::new()),
        )
    }

    fn option_env_ty(&self, expr: ExprId) -> Result<Ty, PackageStoreError> {
        let synthetic_span = self.synthetic_span_for_expr(expr);
        let str_ref = TypeRef::Reference {
            lifetime: None,
            mutability: Mutability::Shared,
            inner: Box::new(TypeRef::Path(TypePath {
                source_span: synthetic_span,
                absolute: false,
                segments: vec![TypePathSegment {
                    name: Name::new("str"),
                    args: Vec::new(),
                    span: synthetic_span,
                }],
            })),
        };

        self.resolve_synthetic_type_ref(
            expr,
            self.synthetic_type_path(
                expr,
                &["core", "option", "Option"],
                vec![ItemGenericArg::Type(str_ref)],
            ),
        )
    }

    fn resolve_synthetic_type_ref(
        &self,
        expr: ExprId,
        ty: TypeRef,
    ) -> Result<Ty, PackageStoreError> {
        let expr_data = self.context.body().expr_unchecked(expr);

        // Builtins produce compiler-known types, but some fixtures and partial workspaces cannot
        // resolve the corresponding `core` paths. Keep those cases unknown instead of surfacing
        // synthetic syntax as if the user had written it.
        let ty = self
            .context
            .type_refs(TypeRefUseSite::Scope(expr_data.scope))
            .resolve(&ty)?;
        Ok(if matches!(ty, Ty::Syntax(_)) {
            Ty::Unknown
        } else {
            ty
        })
    }

    fn synthetic_type_path(
        &self,
        expr: ExprId,
        segments: &[&str],
        final_args: Vec<ItemGenericArg>,
    ) -> TypeRef {
        let synthetic_span = self.synthetic_span_for_expr(expr);
        let final_idx = segments.len().saturating_sub(1);
        TypeRef::Path(TypePath {
            source_span: synthetic_span,
            absolute: false,
            segments: segments
                .iter()
                .enumerate()
                .map(|(idx, name)| TypePathSegment {
                    name: Name::new(name),
                    args: if idx == final_idx {
                        final_args.clone()
                    } else {
                        Default::default()
                    },
                    span: synthetic_span,
                })
                .collect(),
        })
    }

    fn synthetic_span_for_expr(&self, expr: ExprId) -> Span {
        let expr_data = self.context.body().expr_unchecked(expr);
        let source_span = expr_data.source.span;
        Span {
            text: TextSpan {
                start: source_span.text.start,
                end: source_span.text.start,
            },
        }
    }

    fn static_str_ty() -> Ty {
        Ty::reference(Mutability::Shared, Ty::Primitive(PrimitiveTy::Str))
    }
}
