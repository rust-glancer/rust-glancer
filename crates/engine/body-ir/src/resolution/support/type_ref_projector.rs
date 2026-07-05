//! Body-local projection from written type syntax into inference types.
//!
//! Plain type-ref resolution gives us stable `Ty` values, but body inference often needs a richer
//! view: `T` should stay as the same `?T` slot that the call result uses, and associated
//! projections such as `Self::Item` or `S::Item` may need body-local solver evidence.

use rg_ir_model::{
    TraitRef,
    items::{GenericArg as ItemGenericArg, TypePathAnchor, TypeRef},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_text::Name;
use rg_ty::{
    GenericArg, Ty,
    inference::{InferenceTypeRefProjector, InferenceTypeSubst},
};

use crate::resolution::query::TypeRefResolutionQuery;

use super::self_associated_type_name;

/// Callback for selected-trait `Self::Assoc`, such as `Iterator::Item` in `Iterator::map`.
type SelfAssociatedTypeProjector<'a> =
    &'a mut dyn FnMut(&str) -> Result<Option<Ty>, PackageStoreError>;
/// Callback for impl-generic `T::Assoc`, such as `I::Item` inside an adapter alias.
///
/// Qualified syntax like `<I as Iterator>::Item` passes the explicit trait goal. Plain `I::Item`
/// passes `None`, because the caller has to choose the supporting predicate itself.
type AssociatedTypeProjector<'a> = &'a mut dyn FnMut(
    &Name,
    Option<(TraitRef, Vec<GenericArg>)>,
    &Name,
) -> Result<Option<Ty>, PackageStoreError>;

/// Projects written `TypeRef`s using body-local inference and associated projection evidence.
///
/// This is the inference-side version of "what type does this written syntax mean here?" For
/// ordinary syntax it delegates to `InferenceTypeRefProjector`, so `Vec<T>` can become `Vec<?T>`.
/// Callers provide body-local associated projection callbacks. That keeps this component focused
/// on walking written type syntax and lets obligation code own the solver evidence.
pub(crate) struct BodyTypeRefProjector<'a, 'query, D, I> {
    subst: &'a InferenceTypeSubst,
    resolver: &'a TypeRefResolutionQuery<'query, D, I>,
    self_associated_ty: Option<SelfAssociatedTypeProjector<'a>>,
    type_param_associated_ty: Option<AssociatedTypeProjector<'a>>,
}

/// Result of trying the body-local projection hooks before ordinary type resolution.
///
/// This needs three states, not two. `Unsupported` means the syntax definitely asked for
/// body-local evidence and we failed to prove it. `NotBodyLocal` means the syntax did not need
/// this layer at all, so ordinary type-ref projection should still run.
enum LocalProjection {
    Projected(Ty),
    /// The written shape asked for a body-local associated projection, but we could not prove it.
    Unsupported,
    /// The written shape has no body-local projection syntax and should use ordinary fallback.
    NotBodyLocal,
}

impl LocalProjection {
    fn from_attempted_projection(ty: Option<Ty>) -> Self {
        match ty {
            Some(ty) => Self::Projected(ty),
            None => Self::Unsupported,
        }
    }

    fn map_projected(self, f: impl FnOnce(Ty) -> Ty) -> Self {
        match self {
            Self::Projected(ty) => Self::Projected(f(ty)),
            Self::Unsupported => Self::Unsupported,
            Self::NotBodyLocal => Self::NotBodyLocal,
        }
    }
}

impl<'a, 'query, D, I> BodyTypeRefProjector<'a, 'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(
        subst: &'a InferenceTypeSubst,
        resolver: &'a TypeRefResolutionQuery<'query, D, I>,
    ) -> Self {
        Self {
            subst,
            resolver,
            self_associated_ty: None,
            type_param_associated_ty: None,
        }
    }

    pub(crate) fn with_self_associated_ty(
        mut self,
        projector: SelfAssociatedTypeProjector<'a>,
    ) -> Self {
        self.self_associated_ty = Some(projector);
        self
    }

    pub(crate) fn with_type_param_associated_ty(
        mut self,
        projector: AssociatedTypeProjector<'a>,
    ) -> Self {
        self.type_param_associated_ty = Some(projector);
        self
    }

    /// Project a written type ref, falling back to ordinary type-ref projection when body-local
    /// associated projection is absent or unsupported.
    pub(crate) fn ty_or_fallback(&mut self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        if let LocalProjection::Projected(projected_ty) = self.project_body_local_ty(ty)? {
            return Ok(projected_ty);
        }

        self.fallback_ty(ty)
    }

    /// Project a written type ref when unsupported body-local projection should stop the caller.
    ///
    /// Impl associated aliases use this mode for shapes like `S::Item`: if no support predicate
    /// proves which impl provides `Item`, the whole alias projection must stay unknown rather than
    /// falling back to an ordinary `<unknown>`.
    pub(crate) fn ty_if_supported(
        &mut self,
        ty: &TypeRef,
    ) -> Result<Option<Ty>, PackageStoreError> {
        match self.project_body_local_ty(ty)? {
            LocalProjection::Projected(projected_ty) => Ok(Some(projected_ty)),
            LocalProjection::Unsupported => Ok(None),
            LocalProjection::NotBodyLocal => self.fallback_ty(ty).map(Some),
        }
    }

    /// Project a generic argument while preserving inference slots in type arguments.
    pub(crate) fn generic_arg_or_fallback(
        &mut self,
        arg: &ItemGenericArg,
        resolved_arg: &GenericArg,
    ) -> Result<GenericArg, PackageStoreError> {
        if let (ItemGenericArg::Type(ty), GenericArg::Type(resolved_ty)) = (arg, resolved_arg) {
            let projected_ty = self.ty_from_resolved(ty, resolved_ty)?;
            return Ok(GenericArg::Type(Box::new(projected_ty)));
        }

        Ok(InferenceTypeRefProjector::new(self.subst).generic_arg_from_arg(arg, resolved_arg))
    }

    /// Project a type ref when the caller already has the ordinary resolved `Ty`.
    ///
    /// This is useful for trait-bound args, where the resolver returns both written args and their
    /// resolved counterparts together. We still give body-local associated projections a chance
    /// before falling back to the ordinary resolved type.
    fn ty_from_resolved(
        &mut self,
        ty: &TypeRef,
        resolved_ty: &Ty,
    ) -> Result<Ty, PackageStoreError> {
        if let LocalProjection::Projected(projected_ty) = self.project_body_local_ty(ty)? {
            return Ok(projected_ty);
        }

        Ok(InferenceTypeRefProjector::new(self.subst).ty_from_type_ref(ty, resolved_ty))
    }

    fn fallback_ty(&mut self, ty: &TypeRef) -> Result<Ty, PackageStoreError> {
        let resolved_ty = self.resolver.resolve(ty)?;
        Ok(InferenceTypeRefProjector::new(self.subst).ty_from_type_ref(ty, &resolved_ty))
    }

    /// Try body-local projection before ordinary resolver fallback.
    ///
    /// This only handles syntax that needs caller-provided evidence, such as `Self::Item`,
    /// `S::Item`, or wrappers around them. `NotBodyLocal` means the ordinary projector should
    /// handle the type instead.
    fn project_body_local_ty(
        &mut self,
        ty: &TypeRef,
    ) -> Result<LocalProjection, PackageStoreError> {
        // Check `Self::Assoc`.
        if let Some(assoc_name) = self_associated_type_name(ty) {
            let Some(projector) = self.self_associated_ty.as_mut() else {
                return Ok(LocalProjection::NotBodyLocal);
            };
            return Ok(LocalProjection::from_attempted_projection(projector(
                assoc_name,
            )?));
        }

        // Check `T::Assoc`.
        if let Some((param_name, assoc_name)) = ty.as_type_param_assoc_path()
            && self.subst.type_param(param_name.as_str()).is_some()
            && let Some(projector) = self.type_param_associated_ty.as_mut()
        {
            return Ok(LocalProjection::from_attempted_projection(projector(
                param_name, None, assoc_name,
            )?));
        }

        // Check `<T>::Assoc` and `<T as Trait>::Assoc`.
        if let TypeRef::Path(path) = ty
            && let Some(anchor) = &path.anchor
        {
            let Some(assoc_segment) = path.segments.first() else {
                return Ok(LocalProjection::Unsupported);
            };
            if path.segments.len() != 1 || !assoc_segment.args.is_empty() {
                return Ok(LocalProjection::Unsupported);
            }
            return match anchor {
                TypePathAnchor::Type(self_ty) => {
                    self.project_type_anchor_associated_ty(self_ty, &assoc_segment.name)
                }
                TypePathAnchor::QualifiedTrait { self_ty, trait_ty } => {
                    self.project_qualified_associated_ty(self_ty, trait_ty, &assoc_segment.name)
                }
            };
        }

        // Check tuple.
        if let TypeRef::Tuple(fields) = ty {
            return self.project_body_local_tuple(fields);
        }

        // Check reference.
        if let TypeRef::Reference {
            mutability, inner, ..
        } = ty
        {
            return Ok(self
                .project_body_local_ty(inner)?
                .map_projected(|inner_ty| Ty::Reference {
                    mutability: *mutability,
                    inner: Box::new(inner_ty),
                }));
        }

        // Check slice.
        if let TypeRef::Slice(inner) = ty {
            return Ok(self
                .project_body_local_ty(inner)?
                .map_projected(|inner_ty| Ty::Slice(Box::new(inner_ty))));
        }

        // Check array.
        if let TypeRef::Array { inner, len } = ty {
            return Ok(self
                .project_body_local_ty(inner)?
                .map_projected(|inner_ty| Ty::Array {
                    inner: Box::new(inner_ty),
                    len: len.clone(),
                }));
        }

        Ok(LocalProjection::NotBodyLocal)
    }

    fn project_type_anchor_associated_ty(
        &mut self,
        self_ty: &TypeRef,
        assoc_name: &Name,
    ) -> Result<LocalProjection, PackageStoreError> {
        let param_name = match self.type_param_assoc_name(self_ty) {
            Ok(param_name) => param_name,
            Err(projection) => return Ok(projection),
        };
        let projector = self.type_param_assoc_projector_unchecked();

        Ok(LocalProjection::from_attempted_projection(projector(
            &param_name,
            None,
            assoc_name,
        )?))
    }

    fn project_qualified_associated_ty(
        &mut self,
        self_ty: &TypeRef,
        trait_ty: &TypeRef,
        assoc_name: &Name,
    ) -> Result<LocalProjection, PackageStoreError> {
        let param_name = match self.type_param_assoc_name(self_ty) {
            Ok(param_name) => param_name,
            Err(projection) => return Ok(projection),
        };
        let Some((trait_ref, resolved_args)) = self.resolver.resolve_trait_bound(trait_ty)? else {
            return Ok(LocalProjection::Unsupported);
        };
        let Some(args) = self.project_qualified_trait_args(trait_ty, &resolved_args) else {
            return Ok(LocalProjection::Unsupported);
        };
        let projector = self.type_param_assoc_projector_unchecked();

        Ok(LocalProjection::from_attempted_projection(projector(
            &param_name,
            Some((trait_ref, args)),
            assoc_name,
        )?))
    }

    fn type_param_assoc_name(&self, self_ty: &TypeRef) -> Result<Name, LocalProjection> {
        if self.type_param_associated_ty.is_none() {
            return Err(LocalProjection::NotBodyLocal);
        }
        let Some(param_name) = self_ty.type_param_name() else {
            return Err(LocalProjection::Unsupported);
        };
        if self.subst.type_param(param_name.as_str()).is_none() {
            return Err(LocalProjection::Unsupported);
        }

        Ok(param_name)
    }

    fn type_param_assoc_projector_unchecked(&mut self) -> &mut AssociatedTypeProjector<'a> {
        self.type_param_associated_ty
            .as_mut()
            .expect("type-param associated projector must be checked")
    }

    fn project_qualified_trait_args(
        &self,
        trait_ty: &TypeRef,
        resolved_args: &[GenericArg],
    ) -> Option<Vec<GenericArg>> {
        let TypeRef::Path(path) = trait_ty else {
            return None;
        };
        let segment = path.segments.last()?;
        if segment.args.len() != resolved_args.len() {
            return None;
        }

        Some(
            segment
                .args
                .iter()
                .zip(resolved_args)
                .map(|(arg, resolved_arg)| {
                    InferenceTypeRefProjector::new(self.subst)
                        .generic_arg_from_arg(arg, resolved_arg)
                })
                .collect(),
        )
    }

    /// Project tuple fields when at least one field uses body-local evidence.
    ///
    /// Tuple projection is mixed: projected fields keep their body-local answer, and ordinary
    /// fields use fallback projection. That lets `(S::Item, bool)` keep the projected `S::Item`
    /// without losing the normal `bool` field.
    fn project_body_local_tuple(
        &mut self,
        fields: &[TypeRef],
    ) -> Result<LocalProjection, PackageStoreError> {
        let mut projected_fields = Vec::with_capacity(fields.len());
        let mut saw_projection = false;

        for field in fields {
            match self.project_body_local_ty(field)? {
                LocalProjection::Projected(field_ty) => {
                    saw_projection = true;
                    projected_fields.push(Some(field_ty));
                }
                LocalProjection::Unsupported => return Ok(LocalProjection::Unsupported),
                LocalProjection::NotBodyLocal => projected_fields.push(None),
            }
        }

        if !saw_projection {
            return Ok(LocalProjection::NotBodyLocal);
        }

        // Fill ordinary fields after body-local fields are known.
        let fields = fields
            .iter()
            .zip(projected_fields)
            .map(|(field, projected_ty)| match projected_ty {
                Some(projected_ty) => Ok(projected_ty),
                None => self.fallback_ty(field),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LocalProjection::Projected(Ty::Tuple(fields)))
    }
}
