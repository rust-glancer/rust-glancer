//! Type-path queries over DefMap and item-store providers.
//!
//! DefMap lookup answers "which definitions does this path name?", while the item store
//! answers "which semantic item does this local definition lower to?". Type algorithms use this
//! query to stay independent from the concrete target/body storage that provided those answers.

use rg_ir_model::items::{GenericArg as ItemGenericArg, Mutability, TypePath, TypeRef};
use rg_ir_model::{DefId, ModuleRef, SemanticItemRef, TraitRef, TypeDefRef, TypePathResolution};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemStoreQuery, ItemStoreSource, Path, TypePathContext,
};

use crate::{GenericArg, PrimitiveTy, RefMutability, Ty, TypeSubst};

/// Resolves paths into semantic-shaped item refs using independent DefMap and ItemStore sources.
#[derive(Clone)]
pub struct ItemPathQuery<'a, D, I> {
    def_maps: DefMapQuery<D>,
    items: ItemStoreQuery<'a, I>,
}

impl<'a, D, I> ItemPathQuery<'a, D, I>
where
    D: DefMapSource,
    I: ItemStoreSource<'a, Error = D::Error>,
{
    pub fn new(def_maps: D, items: I) -> Self {
        Self {
            def_maps: DefMapQuery::new(def_maps),
            items: ItemStoreQuery::new(items),
        }
    }

    /// Gives algorithms access to item data after path resolution has selected semantic refs.
    pub fn items(&self) -> &ItemStoreQuery<'a, I> {
        &self.items
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
                let path = Path::from_type_path(type_path);
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
                match mutability {
                    Mutability::Shared => RefMutability::Shared,
                    Mutability::Mutable => RefMutability::Mutable,
                },
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
            _ => Ok(Ty::syntax(ty.clone())),
        }
    }

    /// Resolves a type-position path into the type resolution shape used by type projection.
    pub fn resolve_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<TypePathResolution, D::Error> {
        if path.is_self_type() {
            let Some(impl_ref) = context.impl_ref else {
                return Ok(TypePathResolution::Unknown);
            };
            let types = self
                .items
                .impl_data(impl_ref)?
                .map(|data| data.resolved_self_tys.clone())
                .unwrap_or_default();
            return Ok(if types.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::SelfType(types)
            });
        }

        let type_defs = self.type_defs_for_path(context.module, path)?;
        if type_defs.is_empty() {
            let traits = self.traits_for_path(context.module, path)?;
            Ok(if traits.is_empty() {
                TypePathResolution::Unknown
            } else {
                TypePathResolution::Traits(traits)
            })
        } else {
            Ok(TypePathResolution::TypeDefs(type_defs))
        }
    }

    /// Resolves a type-position path into canonical item refs, preserving `Self` handling.
    pub fn semantic_items_for_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, D::Error> {
        if path.is_self_type() {
            if let Some(impl_ref) = context.impl_ref
                && let Some(data) = self.items.impl_data(impl_ref)?
            {
                let items = data
                    .resolved_self_tys
                    .iter()
                    .copied()
                    .map(SemanticItemRef::from)
                    .collect();
                return Ok(items);
            } else {
                return Ok(Vec::new());
            };
        }

        self.semantic_items_for_path(context.module, path)
    }

    /// Filters a type-position path to nominal type definitions.
    pub fn type_defs_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<TypeDefRef>, D::Error> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::TypeDef(ty) => Some(ty),
                _ => None,
            })
            .collect())
    }

    /// Filters a type-position path to trait definitions.
    pub fn traits_for_path(&self, from: ModuleRef, path: &Path) -> Result<Vec<TraitRef>, D::Error> {
        Ok(self
            .semantic_items_for_path(from, path)?
            .into_iter()
            .filter_map(|item| match item {
                SemanticItemRef::Trait(trait_ref) => Some(trait_ref),
                _ => None,
            })
            .collect())
    }

    /// Resolves through the type namespace and projects local definitions into item refs.
    fn semantic_items_for_path(
        &self,
        from: ModuleRef,
        path: &Path,
    ) -> Result<Vec<SemanticItemRef>, D::Error> {
        let result = self.def_maps.resolve_path_in_type_namespace(from, path)?;
        let mut resolved_items = Vec::new();
        for def in result.resolved {
            if let DefId::Local(local_def) = def
                && let Some(item) = self.items.semantic_item_for_local_def(local_def)?
            {
                push_unique(&mut resolved_items, item);
            }
        }

        Ok(resolved_items)
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
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
