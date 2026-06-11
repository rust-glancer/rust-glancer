//! Field access resolution.

use rg_ir_model::{
    EnumVariantRef, ExprId, FieldRef, TypeDefId,
    identity::DeclarationRef,
    items::{FieldItem, FieldKey, FieldList},
};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{AutoderefMode, NominalTy, Ty};

use crate::{
    ir::resolved::BodyResolution,
    resolution::{BodyResolutionContext, TypeRefUseSite, support::unique_ty_or_unknown},
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedFieldTarget {
    Declared(DeclaredFieldTarget),
    Structural { ty: Ty },
}

/// Declared field selected from a nominal owner type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredFieldTarget {
    field: FieldRef,
    ty: Option<Ty>,
}

impl DeclaredFieldTarget {
    /// Return the field type if the declaration was available.
    pub(crate) fn ty(&self) -> Option<&Ty> {
        self.ty.as_ref()
    }
}

/// Field lookup result at the selected autoderef depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedFieldTargets {
    targets: UniqueVec<ResolvedFieldTarget>,
}

impl ResolvedFieldTargets {
    /// Start with no field targets.
    fn new() -> Self {
        Self {
            targets: UniqueVec::new(),
        }
    }

    /// Return whether field lookup found no targets.
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Return declarations for named fields, or unknown for structural fields.
    pub(crate) fn resolution(&self) -> BodyResolution {
        let mut fields = UniqueVec::new();
        for target in &self.targets {
            let ResolvedFieldTarget::Declared(target) = target else {
                continue;
            };
            fields.push(target.field);
        }

        if fields.is_empty() {
            BodyResolution::Unknown
        } else {
            BodyResolution::Declarations(fields.into_iter().map(DeclarationRef::from).collect())
        }
    }

    /// Return the unique field type, or unknown for zero or multiple types.
    pub(crate) fn ty(&self) -> Ty {
        let mut tys = UniqueVec::new();
        for target in &self.targets {
            match target {
                ResolvedFieldTarget::Declared(target) => {
                    let Some(ty) = target.ty() else {
                        continue;
                    };
                    tys.push(ty.clone());
                }
                ResolvedFieldTarget::Structural { ty } => {
                    tys.push(ty.clone());
                }
            }
        }

        unique_ty_or_unknown(tys)
    }

    /// Add a declared field target.
    fn push_declared(&mut self, target: DeclaredFieldTarget) {
        self.targets.push(ResolvedFieldTarget::Declared(target));
    }

    /// Add a structural field type with no declaration.
    fn push_structural_ty(&mut self, ty: Ty) {
        self.targets.push(ResolvedFieldTarget::Structural { ty });
    }
}

/// Resolves field access for nominal and structural receiver types.
pub(crate) struct BodyFieldQuery<'query, D, I> {
    context: BodyResolutionContext<'query, D, I>,
}

impl<'query, D, I> BodyFieldQuery<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    pub(crate) fn new(context: BodyResolutionContext<'query, D, I>) -> Self {
        Self { context }
    }

    /// Resolve a field access expression through receiver autoderef.
    pub(crate) fn resolve(
        &self,
        base: ExprId,
        field: &FieldKey,
    ) -> Result<ResolvedFieldTargets, PackageStoreError> {
        let mut current_depth = None;
        let mut targets = ResolvedFieldTargets::new();

        for candidate in self.context.autoderef().candidates(
            AutoderefMode::FieldLookup,
            self.context.body().expr_ty_unchecked(base),
        ) {
            let candidate = candidate?;
            // Field lookup stops at the first autoderef depth that has matches. Same-depth
            // alternatives stay together so ambiguous receivers do not become order-dependent.
            if current_depth.is_some_and(|depth| depth != candidate.depth()) && !targets.is_empty()
            {
                return Ok(targets);
            }
            current_depth = Some(candidate.depth());

            if let Some(ty) = Self::structural_field_ty(candidate.ty(), field) {
                targets.push_structural_ty(ty);
            }

            for nominal_ty in candidate.ty().as_nominals() {
                if let Some(target) = self.declared(nominal_ty, field)? {
                    targets.push_declared(target);
                }
            }
        }

        Ok(targets)
    }

    /// Resolve a declared field directly from its owner type.
    pub(crate) fn declared(
        &self,
        owner_ty: &NominalTy,
        field: &FieldKey,
    ) -> Result<Option<DeclaredFieldTarget>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(field_ref) = item_query.field_for_type(owner_ty.def, field)? else {
            return Ok(None);
        };
        let Some(field_data) = item_query.field_data(field_ref)? else {
            return Ok(Some(DeclaredFieldTarget {
                field: field_ref,
                ty: None,
            }));
        };

        let ty = self
            .context
            .type_refs(TypeRefUseSite::Module(field_data.owner_module))
            .with_subst(&self.context.generics().subst_for_nominal_ty(owner_ty)?)
            .resolve(&field_data.field.ty)?;

        Ok(Some(DeclaredFieldTarget {
            field: field_ref,
            ty: Some(ty),
        }))
    }

    /// Return the type of an enum variant field for a known enum type.
    pub(crate) fn enum_variant_field_ty(
        &self,
        enum_ty: &NominalTy,
        variant_ref: EnumVariantRef,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let TypeDefId::Enum(enum_id) = enum_ty.def.id else {
            return Ok(None);
        };
        if variant_ref.origin != enum_ty.def.origin || variant_ref.enum_id != enum_id {
            return Ok(None);
        }

        let item_query = self.context.item_query();
        let Some(variant_data) = item_query.enum_variant_data(variant_ref)? else {
            return Ok(None);
        };
        let Some(field) = Self::variant_field(&variant_data.variant.fields, field_key) else {
            return Ok(None);
        };

        Ok(Some(
            self.context
                .type_refs(TypeRefUseSite::Module(variant_data.owner_module))
                .with_subst(&self.context.generics().subst_for_nominal_ty(enum_ty)?)
                .resolve(&field.ty)?,
        ))
    }

    /// Read a tuple field type from a structural tuple receiver.
    fn structural_field_ty(ty: &Ty, field: &FieldKey) -> Option<Ty> {
        match (ty, field) {
            (Ty::Tuple(fields), FieldKey::Tuple(index)) => fields.get(*index).cloned(),
            _ => None,
        }
    }

    /// Find a named or tuple field inside a variant declaration.
    fn variant_field<'field>(
        fields: &'field FieldList,
        key: &FieldKey,
    ) -> Option<&'field FieldItem> {
        match key {
            FieldKey::Named(_) => fields
                .fields()
                .iter()
                .find(|field| field.key.as_ref() == Some(key)),
            FieldKey::Tuple(index) => fields
                .fields()
                .get(*index)
                .filter(|field| field.key.as_ref() == Some(key)),
        }
    }
}
