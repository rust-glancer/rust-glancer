//! Builds semantic generic identities from syntax-shaped item declarations.
//!
//! The item store remains useful for display because it keeps names, bounds, and defaults. This
//! query adds the owner relationships and implicit parameters needed by semantic types, then
//! exposes one canonical order to type lowering, substitutions, inference, and Chalk.

use rg_ir_model::{
    ConstParamRef, GenericDefRef, GenericParamRef, ImplRef, ItemOwner, LifetimeParamRef,
    LocalLifetimeParamId, LocalTypeOrConstParamId, TraitDefRef, TypeParamRef,
};
use rg_item_tree::{GenericArg as ItemGenericArg, TypeOrConstParamData, TypeRef};

use super::{ItemStoreQuery, ItemStoreSource};
use crate::{GenericParamSource, GenericParamView, Generics};

/// Definition-level generic parameter query.
///
/// Semantic types depend on this query for identity and ordering, while the underlying item data
/// remains syntax-shaped and available to declaration renderers.
#[derive(Clone)]
pub struct GenericsQuery<'a, S> {
    items: ItemStoreQuery<'a, S>,
}

impl<'a, S> GenericsQuery<'a, S>
where
    S: ItemStoreSource<'a> + Clone,
{
    pub fn new(source: S) -> Self {
        Self {
            items: ItemStoreQuery::new(source),
        }
    }

    /// Returns every parameter visible in `owner` in canonical argument order.
    pub fn generics(&self, owner: GenericDefRef) -> Result<Generics<'a>, S::Error> {
        let parent = self
            .parent_generic_def(owner)?
            .map(|parent| self.generics(parent))
            .transpose()?;
        let mut own_params = Vec::new();

        // Trait `Self` is a real semantic parameter even though it has no source declaration.
        let type_or_const_offset = if matches!(owner, GenericDefRef::Trait(_)) {
            own_params.push(GenericParamView::new(
                GenericParamRef::Type(TypeParamRef {
                    owner,
                    local_id: LocalTypeOrConstParamId(0),
                }),
                GenericParamSource::TraitSelf,
            ));
            1
        } else {
            0
        };

        let mut explicit_type_or_const_len = 0;
        if let Some(item) = self.items.semantic_item_view(owner.into())?
            && let Some(params) = item.generic_params()
        {
            explicit_type_or_const_len = params.type_or_consts.len();
            own_params.extend(params.lifetimes.iter().enumerate().map(|(index, param)| {
                GenericParamView::new(
                    GenericParamRef::Lifetime(LifetimeParamRef {
                        owner,
                        local_id: LocalLifetimeParamId(index),
                    }),
                    GenericParamSource::Lifetime(param),
                )
            }));

            own_params.extend(
                params
                    .type_or_consts
                    .iter()
                    .enumerate()
                    .map(|(index, param)| {
                        let local_id = LocalTypeOrConstParamId(index + type_or_const_offset);
                        match param {
                            TypeOrConstParamData::Type(param) => GenericParamView::new(
                                GenericParamRef::Type(TypeParamRef { owner, local_id }),
                                GenericParamSource::Type(param),
                            ),
                            TypeOrConstParamData::Const(param) => GenericParamView::new(
                                GenericParamRef::Const(ConstParamRef { owner, local_id }),
                                GenericParamSource::Const(param),
                            ),
                        }
                    }),
            );
        }

        // Each argument-position `impl Trait` is an anonymous type parameter owned by the
        // function. Enumerating the signature in source order gives those parameters stable
        // owner-local identities without putting semantic IDs into syntax HIR.
        if let GenericDefRef::Function(function) = owner
            && let Some(data) = self.items.function_data(function)?
        {
            let mut bounds = Vec::new();
            for param in data.signature.params() {
                if let Some(ty) = &param.ty {
                    Self::collect_argument_impl_traits(ty, &mut bounds);
                }
            }
            let first_id = explicit_type_or_const_len + type_or_const_offset;
            own_params.extend(bounds.into_iter().enumerate().map(|(index, bounds)| {
                GenericParamView::new(
                    GenericParamRef::Type(TypeParamRef {
                        owner,
                        local_id: LocalTypeOrConstParamId(first_id + index),
                    }),
                    GenericParamSource::ArgumentImplTrait(bounds),
                )
            }));
        }

        Ok(Generics::new(owner, parent, own_params))
    }

    fn parent_generic_def(&self, owner: GenericDefRef) -> Result<Option<GenericDefRef>, S::Error> {
        let can_inherit = matches!(
            owner,
            GenericDefRef::Function(_) | GenericDefRef::TypeAlias(_) | GenericDefRef::Const(_)
        );
        if !can_inherit {
            return Ok(None);
        }
        let Some(item) = self.items.semantic_item_view(owner.into())? else {
            return Ok(None);
        };
        let Some(parent) = item.item_owner() else {
            return Ok(None);
        };

        Ok(match parent {
            ItemOwner::Module(_) => None,
            ItemOwner::Trait(id) => Some(
                TraitDefRef {
                    origin: owner.origin(),
                    id,
                }
                .into(),
            ),
            ItemOwner::Impl(id) => Some(
                ImplRef {
                    origin: owner.origin(),
                    id,
                }
                .into(),
            ),
        })
    }

    fn collect_argument_impl_traits<'ty>(
        ty: &'ty TypeRef,
        out: &mut Vec<&'ty [rg_item_tree::TypeBound]>,
    ) {
        match ty {
            TypeRef::ImplTrait(bounds) => out.push(bounds),
            TypeRef::Tuple(types) => {
                for ty in types {
                    Self::collect_argument_impl_traits(ty, out);
                }
            }
            TypeRef::Reference { inner, .. }
            | TypeRef::RawPointer { inner, .. }
            | TypeRef::Slice(inner)
            | TypeRef::Array { inner, .. } => Self::collect_argument_impl_traits(inner, out),
            TypeRef::FnPointer { params, ret } => {
                for ty in params {
                    Self::collect_argument_impl_traits(ty, out);
                }
                Self::collect_argument_impl_traits(ret, out);
            }
            TypeRef::Path(path) => {
                for segment in &path.segments {
                    for arg in &segment.args {
                        match arg {
                            ItemGenericArg::Type(ty) => Self::collect_argument_impl_traits(ty, out),
                            ItemGenericArg::FnTraitArgs { params, ret } => {
                                for ty in params {
                                    Self::collect_argument_impl_traits(ty, out);
                                }
                                Self::collect_argument_impl_traits(ret, out);
                            }
                            ItemGenericArg::AssocType { ty: Some(ty), .. } => {
                                Self::collect_argument_impl_traits(ty, out);
                            }
                            ItemGenericArg::Lifetime(_)
                            | ItemGenericArg::Const(_)
                            | ItemGenericArg::AssocType { ty: None, .. }
                            | ItemGenericArg::Unsupported(_) => {}
                        }
                    }
                }
            }
            TypeRef::DynTrait(_)
            | TypeRef::Unknown(_)
            | TypeRef::Never
            | TypeRef::Unit
            | TypeRef::Infer => {}
        }
    }
}
