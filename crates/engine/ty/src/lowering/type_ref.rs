//! Walk source type shapes while retaining one session's inference and opaque identities.

use super::{ImplTraitMode, MAX_TYPE_LOWERING_DEPTH, TypeLoweringSession, TypePathResolver};
use crate::inference::InferenceTable;
use crate::{AliasTy, Lifetime, OpaqueTy, Ty};
use rg_def_map::DefMapSource;
use rg_ir_model::{GenericParamRef, OpaqueTyId, OpaqueTyRef, TypeParamRef};
use rg_item_tree::TypeRef;
use rg_semantic_ir::{GenericParamSource, ItemStoreSource};

impl<'lower, 'query, D, I, R> TypeLoweringSession<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    /// Lower a source type, treating `impl Trait` as an opaque type occurrence.
    pub(crate) fn lower_type_ref(&mut self, ty: &TypeRef) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, None)
    }

    /// Lower a body-written type while giving each explicit `_` a live inference identity.
    ///
    /// Path failures remain `Unknown`; only the syntax node dedicated to inference requests a
    /// variable. Keeping this policy inside the authoritative visitor prevents body inference
    /// from walking `TypeRef` a second time to rediscover holes.
    pub fn lower_type_ref_with_inference(
        &mut self,
        ty: &TypeRef,
        table: &mut InferenceTable,
    ) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Opaque, Some(table))
    }

    /// Lower a function parameter type, where `impl Trait` introduces an anonymous type parameter.
    ///
    /// `fn visit(value: impl Display)` is generic over a hidden function parameter constrained by
    /// `Display`; it is not the opaque return type produced by `fn make() -> impl Display`.
    pub(crate) fn lower_parameter_type(&mut self, ty: &TypeRef) -> Result<Ty, D::Error> {
        self.lower_type_ref_with_mode(ty, ImplTraitMode::Argument, None)
    }

    pub(crate) fn lower_type_ref_with_mode(
        &mut self,
        ty: &TypeRef,
        impl_trait_mode: ImplTraitMode,
        inference: Option<&mut InferenceTable>,
    ) -> Result<Ty, D::Error> {
        if self.type_ref_depth >= MAX_TYPE_LOWERING_DEPTH {
            self.report_limit("type_ref_depth", Some(MAX_TYPE_LOWERING_DEPTH));
            return Ok(Ty::Unknown);
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
        mut inference: Option<&mut InferenceTable>,
    ) -> Result<Ty, D::Error> {
        match ty {
            TypeRef::Unknown(_) | TypeRef::DynTrait(_) => Ok(Ty::Unknown),
            TypeRef::Infer => Ok(inference
                .as_deref_mut()
                .map(InferenceTable::new_type_var)
                .unwrap_or(Ty::Unknown)),
            TypeRef::Never => Ok(Ty::Never),
            TypeRef::Unit => Ok(Ty::Unit),
            TypeRef::Tuple(types) => Ok(Ty::tuple(
                types
                    .iter()
                    .map(|ty| {
                        self.lower_type_ref_with_mode(ty, impl_trait_mode, inference.as_deref_mut())
                    })
                    .collect::<Result<_, _>>()?,
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
                    .unwrap_or(Lifetime::Erased);
                Ok(Ty::reference_with_lifetime(
                    lifetime,
                    *mutability,
                    self.lower_type_ref_with_mode(
                        inner,
                        impl_trait_mode,
                        inference.as_deref_mut(),
                    )?,
                ))
            }
            TypeRef::RawPointer { mutability, inner } => Ok(Ty::raw_pointer(
                *mutability,
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference.as_deref_mut())?,
            )),
            TypeRef::Slice(inner) => Ok(Ty::slice(self.lower_type_ref_with_mode(
                inner,
                impl_trait_mode,
                inference.as_deref_mut(),
            )?)),
            TypeRef::Array { inner, len } => Ok(Ty::array(
                self.lower_type_ref_with_mode(inner, impl_trait_mode, inference.as_deref_mut())?,
                self.lower_const(len.as_ref().map(rg_item_tree::ConstExpr::as_str))?,
            )),
            TypeRef::FnPointer { params, ret } => Ok(Ty::fn_pointer(
                params
                    .iter()
                    .map(|param| {
                        self.lower_type_ref_with_mode(
                            param,
                            impl_trait_mode,
                            inference.as_deref_mut(),
                        )
                    })
                    .collect::<Result<_, _>>()?,
                self.lower_type_ref_with_mode(ret, impl_trait_mode, inference.as_deref_mut())?,
            )),
            TypeRef::ImplTrait(_) if impl_trait_mode == ImplTraitMode::Argument => self
                .next_argument_impl_trait_param()?
                .map(Ty::Param)
                .map_or(Ok(Ty::Unknown), Ok),
            TypeRef::ImplTrait(bounds) => {
                let opaque = OpaqueTyRef {
                    owner: self.owner,
                    id: OpaqueTyId(self.next_opaque_index()),
                };
                let generics = self.query.item_paths.generics().generics(self.owner)?;
                let opaque = OpaqueTy {
                    opaque,
                    args: self.subst.args_for(&generics),
                };
                let self_ty = Ty::Alias(AliasTy::Opaque(opaque.clone()));
                let mut lowered_bounds = Vec::new();
                for bound in bounds {
                    let Some(trait_ty) = bound.required_trait_ty() else {
                        continue;
                    };
                    if let Some(bound) = self.lower_trait_ref(trait_ty, self_ty.clone())? {
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
