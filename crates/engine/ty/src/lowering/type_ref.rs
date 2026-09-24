//! Interpret source shapes directly in working storage, including inference and opaque identities.

use rg_def_map::DefMapSource;
use rg_ir_model::{GenericParamRef, OpaqueTyId, OpaqueTyRef, TypeParamRef};
use rg_item_tree::{ConstExpr, TypeRef};
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};
use rustc_type_ir as ir;

use super::{ImplTraitMode, MAX_TYPE_LOWERING_DEPTH, TypeLoweringSession, TypePathResolver};
use crate::solver::{InferenceTable, OpaqueTy, Region, Ty};

impl<'s, 'lower, 'query, D, I, R> TypeLoweringSession<'s, 'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Lower a source type, treating `impl Trait` as an opaque type occurrence.
    pub fn lower_type_ref(&mut self, ty: &TypeRef) -> Result<Ty<'s>, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, None)
    }

    /// A written `_` creates a real variable in the caller's table. Missing or unsupported
    /// syntax remains an error type: it must not silently become a fresh inference question.
    pub fn lower_type_ref_with_inference(
        &mut self,
        ty: &TypeRef,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, Some(table))
    }

    /// Lower a function parameter type, where `impl Trait` introduces an anonymous type parameter.
    ///
    /// `fn visit(value: impl Display)` is generic over a hidden function parameter constrained by
    /// `Display`; it is not the opaque return type produced by `fn make() -> impl Display`.
    pub(crate) fn lower_parameter_type(&mut self, ty: &TypeRef) -> Result<Ty<'s>, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Argument, None)
    }

    pub(crate) fn lower_type_ref_with_mode(
        &mut self,
        ty: &TypeRef,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&InferenceTable<'s>>,
    ) -> Result<Ty<'s>, D::Error> {
        if self.type_ref_depth >= MAX_TYPE_LOWERING_DEPTH {
            self.report_limit("type_ref_depth", Some(MAX_TYPE_LOWERING_DEPTH));
            return Ok(self.cx.unknown());
        }

        self.type_ref_depth += 1;
        let result = self.lower_type_ref_with_mode_inner(ty, impl_trait_mode, inference);
        self.type_ref_depth -= 1;
        result
    }

    fn lower_type_ref_with_mode_inner(
        &mut self,
        ty: &TypeRef,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&InferenceTable<'s>>,
    ) -> Result<Ty<'s>, D::Error> {
        match ty {
            TypeRef::Unknown(_) | TypeRef::DynTrait(_) => Ok(self.cx.unknown()),
            TypeRef::Infer => Ok(inference
                .map(|table| table.new_type_var())
                .unwrap_or(self.cx.unknown())),
            TypeRef::Never => Ok(self.cx.never()),
            TypeRef::Unit => Ok(self.cx.unit()),
            TypeRef::Tuple(types) => Ok(self.cx.tuple(
                types
                    .iter()
                    .map(|ty| self.lower_type_ref_with_mode(ty, impl_trait_mode, inference))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            TypeRef::Reference {
                lifetime,
                mutability,
                inner,
            } => {
                let lifetime = lifetime
                    .as_ref()
                    .map(|lifetime| self.lower_lifetime(lifetime))
                    .transpose()?
                    .unwrap_or(Region(ir::ReErased));
                let inner = self.lower_type_ref_with_mode(inner, impl_trait_mode, inference)?;
                // An unresolved source reference supplies no type evidence. A written `&_`
                // is different: its inner type is a real variable and keeps the reference shape.
                Ok(if inner.is_unknown() {
                    inner
                } else {
                    self.cx
                        .reference_with_lifetime(lifetime, *mutability, inner)
                })
            }
            TypeRef::RawPointer { mutability, inner } => Ok(self.cx.raw_pointer(
                *mutability,
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference)?,
            )),
            TypeRef::Slice(inner) => Ok(self.cx.slice(self.lower_type_ref_with_mode(
                inner,
                impl_trait_mode,
                inference,
            )?)),
            TypeRef::Array { inner, len } => Ok(self.cx.array(
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference)?,
                self.lower_const(len.as_ref().map(ConstExpr::as_str))?,
            )),
            TypeRef::FnPointer { params, ret } => Ok(self.cx.fn_pointer(
                &params
                    .iter()
                    .map(|param| self.lower_type_ref_with_mode(param, impl_trait_mode, inference))
                    .collect::<Result<Vec<_>, _>>()?,
                self.lower_type_ref_with_mode(ret, impl_trait_mode, inference)?,
            )),
            TypeRef::ImplTrait(_) if impl_trait_mode == ImplTraitMode::Argument => self
                .next_argument_impl_trait_param()?
                .map(|param| self.param_ty(param))
                .map_or(Ok(self.cx.unknown()), Ok),
            TypeRef::ImplTrait(bounds) => {
                let opaque = OpaqueTyRef {
                    owner: self.owner,
                    id: OpaqueTyId(self.next_opaque_index()),
                };
                let generics = self.query.item_paths.generics().generics(self.owner)?;
                let opaque = OpaqueTy {
                    opaque,
                    args: self
                        .subst
                        .args_for(self.cx, generics.iter().map(|p| p.param())),
                };
                let self_ty = self.cx.opaque(opaque);
                let mut lowered_bounds = Vec::new();
                for bound in bounds {
                    let Some(trait_ty) = bound.required_trait_ty() else {
                        continue;
                    };
                    if let Some(bound) = self.lower_trait_ref(trait_ty, self_ty)? {
                        lowered_bounds.push(bound);
                    }
                }
                self.opaque_bounds.push((opaque, lowered_bounds));
                Ok(self_ty)
            }
            TypeRef::Path(path) => self.lower_type_path(path, impl_trait_mode, inference),
        }
    }

    /// Allocate the next opaque occurrence ordinal for the active owner.
    ///
    /// Alias bodies can temporarily change `self.owner` during the same session, so each owner
    /// keeps its own counter. Replaying a complete signature walk then produces the same refs.
    fn next_opaque_index(&mut self) -> usize {
        if let Some((_, index)) = self
            .opaque_indices
            .iter_mut()
            .find(|(owner, _)| *owner == self.owner)
        {
            let current = *index;
            *index += 1;
            current
        } else {
            self.opaque_indices.push((self.owner, 1));
            0
        }
    }

    /// Match the next parameter-position `impl Trait` syntax node to its precomputed parameter ref.
    fn next_argument_impl_trait_param(&mut self) -> Result<Option<TypeParamRef>, D::Error> {
        let index = if let Some((_, index)) = self
            .argument_impl_trait_indices
            .iter_mut()
            .find(|(owner, _)| *owner == self.owner)
        {
            let current = *index;
            *index += 1;
            current
        } else {
            self.argument_impl_trait_indices.push((self.owner, 1));
            0
        };

        Ok(self
            .query
            .item_paths
            .generics()
            .generics(self.owner)?
            .iter_self()
            .filter_map(|param| {
                matches!(param.source(), GenericParamSource::ArgumentImplTrait(_))
                    .then_some(param.param())
            })
            .nth(index)
            .and_then(|param| match param {
                GenericParamRef::Type(param) => Some(param),
                GenericParamRef::Lifetime(_) | GenericParamRef::Const(_) => None,
            }))
    }
}
