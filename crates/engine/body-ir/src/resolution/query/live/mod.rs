//! Body queries over live solver types. Declaration lookup stays in the existing semantic query
//! layer; substitutions and projections keep the caller's inference variables throughout.

mod call;

use rg_def_map::DefMapSource;
use rg_ir_model::{EnumVariantRef, FieldKey, ItemOwner, ScopeId, TraitDefRef, TypeDefId};
use rg_item_tree::{GenericArg as ItemGenericArg, LangItem, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{Generics, ItemStoreSource};
use rg_std::UniqueVec;
use rg_ty::{
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{
        AdtTy, DefId, GenericArgs, InferenceSubstitution, InferenceTable, List, ProjectionTy, Ty,
        TyShape,
    },
};

use crate::{BodyPath, body::facts::BodyResolution, resolution::BodyResolutionContext};

/// Looks up declarations without turning the body's live types into saved types along the way.
/// For a field on `Wrapper<?T>`, its declared `T` must become that same `?T`, so later assignments
/// are visible both through the receiver and through the field result.
pub(crate) struct LiveBodyQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    pub(crate) fn type_ref<'s>(
        &self,
        scope: ScopeId,
        ty: &TypeRef,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, PackageStoreError> {
        let paths = self.context.item_paths();
        let query = TypeLoweringQuery::new(&paths, &self.context);
        let owner = self.context.body().owner().generic_def();
        query.lower_inference_type(
            ty,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Scope(scope)),
            table,
        )
    }

    pub(crate) fn generic_args<'s>(
        &self,
        scope: ScopeId,
        generics: &Generics<'_>,
        args: &[ItemGenericArg],
        table: &InferenceTable<'s>,
    ) -> Result<GenericArgs<'s>, PackageStoreError> {
        let paths = self.context.item_paths();
        let query = TypeLoweringQuery::new(&paths, &self.context);
        let owner = self.context.body().owner().generic_def();
        query.lower_inference_args(
            generics,
            args,
            TypeLoweringEnv::new(owner, TypeLoweringAnchor::Scope(scope)),
            table,
        )
    }

    /// Adjustments only inspect the receiver shape. Trait-backed dereference registers a normal
    /// projection in the caller's context, so a target such as `Wrapper<?T>::Target` retains ?T.
    pub(crate) fn receivers<'s>(
        &self,
        ty: Ty<'s>,
        table: &InferenceTable<'s>,
        unsize_array: bool,
    ) -> Result<Vec<Ty<'s>>, PackageStoreError> {
        let mut result = UniqueVec::new();
        let cx = table.interner();
        let mut ty = ty;
        for _ in 0..32 {
            ty = table.resolve_root_var(ty);
            if !result.push(ty) {
                break;
            }
            match ty.shape() {
                TyShape::Reference { inner, .. } => ty = inner,
                TyShape::Array { inner, .. } if unsize_array => {
                    result.push(cx.slice(inner));
                    break;
                }
                TyShape::Adt(_) => {
                    let lookup = self.context.item_lookup_query();
                    let Some(alias) = lookup.lang_type_alias(LangItem::DerefTarget) else {
                        break;
                    };
                    let Some(deref) = lookup.lang_trait(LangItem::Deref) else {
                        break;
                    };
                    let Some(data) = self.context.item_query().type_alias_data(alias)? else {
                        break;
                    };
                    // Language items are indexed separately. Only the associated type owned by
                    // this Deref trait can supply its target.
                    if alias.origin != deref.origin || data.owner != ItemOwner::Trait(deref.id) {
                        break;
                    }
                    let target = table.normalize(cx.projection(ProjectionTy {
                        associated_ty: alias,
                        args: List::new(cx, &[ty.into()]),
                    }));
                    let _ = table.fulfill();
                    let target = table.resolve_root_var(target);
                    if target.is_var() || target.has_unknown() {
                        break;
                    }
                    ty = target;
                }
                _ => break,
            }
        }
        Ok(result.into_vec())
    }

    /// Find the associated declaration and give its value a live destination in the table.
    /// For `<Iter<?T> as Iterator>::Item`, `Some` means the question could be registered, not
    /// that its answer is already known. The returned type can settle as later goals are solved.
    pub(crate) fn projection<'s>(
        &self,
        ty: Ty<'s>,
        trait_ref: TraitDefRef,
        name: &str,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, PackageStoreError> {
        let Some(associated_ty) = self
            .context
            .item_query()
            .declared_associated_type_by_name(trait_ref, name)?
        else {
            return Ok(None);
        };
        let cx = table.interner();
        Ok(Some(table.normalize(cx.projection(ProjectionTy {
            associated_ty,
            args: List::new(cx, &[ty.into()]),
        }))))
    }

    pub(crate) fn field<'s>(
        &self,
        ty: Ty<'s>,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<(BodyResolution, Ty<'s>)>, PackageStoreError> {
        for receiver in self.receivers(ty, table, false)? {
            if let (TyShape::Tuple(fields), FieldKey::Tuple(index)) = (receiver.shape(), field)
                && let Some(ty) = fields.get(*index)
            {
                return Ok(Some((BodyResolution::Unknown, *ty)));
            }
            let Some(adt) = receiver.as_adt() else {
                continue;
            };
            let Some(field_ref) = self.context.item_query().field_for_type(adt.def, field)? else {
                continue;
            };
            let paths = self.context.item_paths();
            let lowering = TypeLoweringQuery::new(&paths, &self.context);
            let ty = lowering
                .field_ty(table.interner(), field_ref)?
                .map(|ty| self.instantiate_field(adt, ty, table))
                .unwrap_or(table.interner().unknown());
            return Ok(Some((
                BodyResolution::Declarations([field_ref.into()].into_iter().collect()),
                ty,
            )));
        }
        Ok(None)
    }

    fn instantiate_field<'s>(
        &self,
        adt: AdtTy<'s>,
        ty: Ty<'s>,
        table: &InferenceTable<'s>,
    ) -> Ty<'s> {
        let owner = DefId::Adt(adt.def);
        InferenceSubstitution::from_args(table.params(owner).iter().copied(), adt.args)
            .apply(table.interner(), ty)
    }

    pub(crate) fn enum_variant_field<'s>(
        &self,
        adt: AdtTy<'s>,
        variant: EnumVariantRef,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, PackageStoreError> {
        if adt.def.id != TypeDefId::Enum(variant.enum_id) || adt.def.origin != variant.origin {
            return Ok(None);
        }
        let Some(data) = self.context.item_query().enum_variant_data(variant)? else {
            return Ok(None);
        };
        let Some(index) = data
            .variant
            .fields
            .fields()
            .iter()
            .position(|f| f.key.as_ref() == Some(field))
        else {
            return Ok(None);
        };
        let paths = self.context.item_paths();
        let lowering = TypeLoweringQuery::new(&paths, &self.context);
        Ok(lowering
            .enum_variant_field_ty(table.interner(), variant, index)?
            .map(|ty| self.instantiate_field(adt, ty, table)))
    }

    pub(crate) fn pattern_field<'s>(
        &self,
        path: Option<&BodyPath>,
        mut ty: Ty<'s>,
        field: &FieldKey,
        table: &InferenceTable<'s>,
    ) -> Result<Option<Ty<'s>>, PackageStoreError> {
        while let TyShape::Reference { inner, .. } = table.resolve_root_var(ty).shape() {
            ty = inner;
        }
        let Some(adt) = table.resolve_root_var(ty).as_adt() else {
            return Ok(None);
        };
        if matches!(adt.def.id, TypeDefId::Enum(_)) {
            let path = path.and_then(BodyPath::as_def_map_path);
            let Some(name) = path.as_ref().and_then(|p| p.segments().last()) else {
                return Ok(None);
            };
            let Some(variant) = self
                .context
                .item_query()
                .enum_variant_ref_for_type_def(adt.def, name.as_str())?
            else {
                return Ok(None);
            };
            return self.enum_variant_field(adt, variant, field, table);
        }
        Ok(self.field(ty, field, table)?.map(|(_, ty)| ty))
    }
}
