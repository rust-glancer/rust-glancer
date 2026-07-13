//! Type-path queries over DefMap and item-store providers.
//!
//! DefMap lookup answers "which definitions does this path name?", while the item store
//! answers "which semantic item does this local definition lower to?". Type algorithms use this
//! query to stay independent from the concrete crate/body storage that provided those answers.

use rg_def_map::DefMapSource;
use rg_ir_model::items::{GenericArg as ItemGenericArg, TypeBound, TypePath, TypeRef};
use rg_ir_model::{ModuleRef, Path, SemanticItemRef, TraitRef, TypeDefRef, TypePathResolution};
use rg_semantic_ir::{ItemResolutionQuery, ItemStoreQuery, ItemStoreSource, TypePathContext};
use rg_std::UniqueVec;

use crate::{GenericArg, OpaqueTraitBound, PrimitiveTy, Ty, TypeSubst};

/// Resolves paths into semantic-shaped item refs using independent DefMap and ItemStore sources.
#[derive(Clone)]
pub struct ItemPathQuery<'a, D, I> {
    definitions: ItemResolutionQuery<'a, D, I>,
}

impl<'a, D, I> ItemPathQuery<'a, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'a, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            definitions: ItemResolutionQuery::new(def_maps, items),
        }
    }

    /// Gives algorithms access to item data after path resolution has selected semantic refs.
    pub fn items(&self) -> &ItemStoreQuery<'a, I> {
        self.definitions.items()
    }

    /// Resolves syntax-level type data into the shared type vocabulary for one module/impl site.
    pub fn resolve_type_ref(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        unresolved_path_fallback: Ty,
        subst: &TypeSubst,
    ) -> Result<Ty, D::Error> {
        match ty {
            TypeRef::Unit => Ok(Ty::Unit),
            TypeRef::Never => Ok(Ty::Never),
            TypeRef::Path(type_path) => {
                let Some(path) = Path::from_type_path(type_path) else {
                    return Ok(unresolved_path_fallback);
                };
                if let Some(name) = path.single_name()
                    && let Some(ty) = subst.type_param(name)
                {
                    return Ok(ty);
                }

                let args = self.generic_args_from_type_path(type_path, context, subst)?;
                let resolution = self.resolve_type_path(context, &path)?;
                let is_unknown = matches!(resolution, TypePathResolution::Unknown);
                Ok(
                    Ty::from_type_path_resolution(resolution, args).unwrap_or_else(|| {
                        if is_unknown {
                            path.single_name()
                                .and_then(PrimitiveTy::from_name)
                                .map(Ty::Primitive)
                                .unwrap_or(unresolved_path_fallback)
                        } else {
                            unresolved_path_fallback
                        }
                    }),
                )
            }
            TypeRef::Reference {
                mutability, inner, ..
            } => Ok(Ty::reference(
                *mutability,
                self.resolve_type_ref(inner, context, Ty::syntax((**inner).clone()), subst)?,
            )),
            TypeRef::Unknown(_) | TypeRef::Infer => Ok(Ty::Unknown),
            TypeRef::Tuple(types) if types.is_empty() => Ok(Ty::Unit),
            TypeRef::Tuple(types) => Ok(Ty::tuple(
                types
                    .iter()
                    .map(|ty| self.resolve_type_ref(ty, context, Ty::syntax(ty.clone()), subst))
                    .collect::<Result<_, _>>()?,
            )),
            TypeRef::Slice(inner) => Ok(Ty::slice(self.resolve_type_ref(
                inner,
                context,
                Ty::syntax((**inner).clone()),
                subst,
            )?)),
            TypeRef::Array { inner, len } => Ok(Ty::array(
                self.resolve_type_ref(inner, context, Ty::syntax((**inner).clone()), subst)?,
                len.clone(),
            )),
            TypeRef::ImplTrait(bounds) => {
                let opaque_bounds = self.opaque_trait_bounds(bounds, context, subst)?;
                Ok(if opaque_bounds.is_empty() {
                    Ty::syntax(ty.clone())
                } else {
                    Ty::opaque(opaque_bounds)
                })
            }
            _ => Ok(Ty::syntax(ty.clone())),
        }
    }

    /// Resolves a type-position path into the type resolution shape used by type projection.
    pub fn resolve_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        self.definitions.resolve_type_path(context, path)
    }

    /// Resolves a type-position path into canonical item refs, preserving `Self` handling.
    pub fn semantic_items_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<UniqueVec<SemanticItemRef>, D::Error> {
        self.definitions.semantic_items_for_type_path(context, path)
    }

    /// Filters a type-position path to nominal type definitions.
    pub fn type_defs_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<TypeDefRef>, D::Error> {
        self.definitions.type_defs_for_path(from, path)
    }

    /// Filters a type-position path to trait definitions.
    pub fn traits_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<UniqueVec<TraitRef>, D::Error> {
        self.definitions.traits_for_path(from, path)
    }

    fn generic_args_from_type_path(
        &self,
        type_path: &TypePath,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<Vec<GenericArg>, D::Error> {
        // Rust generic args belong to the final path segment for the cases we model here, e.g.
        // `crate::Wrapper<User>` stores `User` on `Wrapper`.
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };

        let mut generic_args = Vec::new();
        for arg in &segment.args {
            let generic_arg = match arg {
                ItemGenericArg::Type(ty) => GenericArg::Type(Box::new(self.resolve_type_ref(
                    ty,
                    context,
                    Ty::syntax(ty.clone()),
                    subst,
                )?)),
                ItemGenericArg::Lifetime(lifetime) => GenericArg::Lifetime(lifetime.clone()),
                ItemGenericArg::Const(value) => GenericArg::Const(value.clone()),
                ItemGenericArg::FnTraitArgs { params, ret } => GenericArg::FnTraitArgs {
                    params: params
                        .iter()
                        .map(|ty| self.resolve_type_ref(ty, context, Ty::syntax(ty.clone()), subst))
                        .collect::<Result<_, _>>()?,
                    ret: Box::new(self.resolve_type_ref(
                        ret,
                        context,
                        Ty::syntax((**ret).clone()),
                        subst,
                    )?),
                },
                ItemGenericArg::AssocType { name, ty } => GenericArg::AssocType {
                    name: name.clone(),
                    ty: match ty {
                        Some(ty) => Some(Box::new(self.resolve_type_ref(
                            ty,
                            context,
                            Ty::syntax(ty.clone()),
                            subst,
                        )?)),
                        None => None,
                    },
                },
                ItemGenericArg::Unsupported(text) => GenericArg::Unsupported(text.clone()),
            };

            generic_args.push(generic_arg);
        }
        Ok(generic_args)
    }

    fn opaque_trait_bounds(
        &self,
        bounds: &[TypeBound],
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<UniqueVec<OpaqueTraitBound>, D::Error> {
        let mut opaque_bounds = UniqueVec::new();

        for bound in bounds {
            match bound {
                TypeBound::Trait(TypeRef::Path(bound_path)) => {
                    let Some(path) = Path::from_type_path(bound_path) else {
                        continue;
                    };
                    let TypePathResolution::Trait(trait_ref) =
                        self.resolve_type_path(context, &path)?
                    else {
                        continue;
                    };
                    let args = self.generic_args_from_type_path(bound_path, context, subst)?;
                    opaque_bounds.push(OpaqueTraitBound { trait_ref, args });
                }
                TypeBound::Trait(_) | TypeBound::Lifetime(_) | TypeBound::Unsupported(_) => {}
            }
        }

        Ok(opaque_bounds)
    }
}
