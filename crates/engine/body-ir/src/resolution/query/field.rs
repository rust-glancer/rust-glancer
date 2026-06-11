//! Field access recovery for body expression resolution.
//!
//! Field lookup combines receiver autoderef, structural tuple fields, declared semantic fields,
//! and owner-generic substitution. Keeping those dimensions together avoids spreading
//! field-specific lookup rules through expression traversal.

use rg_ir_model::{ExprId, FieldRef, identity::DeclarationRef, items::FieldKey};
use rg_ir_storage::{DefMapSource, ItemStoreSource};
use rg_package_store::PackageStoreError;
use rg_std::UniqueVec;
use rg_ty::{AutoderefMode, NominalTy, Ty, TypeSubst};

use crate::{
    ir::resolved::BodyResolution,
    resolution::{BodyResolutionContext, TypeRefUseSite, support::unique_ty_or_unknown},
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedFieldTarget {
    Declared(DeclaredFieldTarget),
    Structural { ty: Ty },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredFieldTarget {
    field: FieldRef,
    ty: Option<Ty>,
}

impl DeclaredFieldTarget {
    pub(crate) fn ty(&self) -> Option<&Ty> {
        self.ty.as_ref()
    }
}

/// Field lookup result at the first autoderef depth that produced any field target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedFieldTargets {
    targets: UniqueVec<ResolvedFieldTarget>,
}

impl ResolvedFieldTargets {
    fn new() -> Self {
        Self {
            targets: UniqueVec::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

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

    fn push_declared(&mut self, target: DeclaredFieldTarget) {
        self.targets.push(ResolvedFieldTarget::Declared(target));
    }

    fn push_structural_ty(&mut self, ty: Ty) {
        self.targets.push(ResolvedFieldTarget::Structural { ty });
    }
}

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
            .type_path_query()
            .type_ref(TypeRefUseSite::Module(field_data.owner_module))
            .with_subst(&self.semantic_type_subst(owner_ty)?)
            .resolve(&field_data.field.ty)?;

        Ok(Some(DeclaredFieldTarget {
            field: field_ref,
            ty: Some(ty),
        }))
    }

    fn structural_field_ty(ty: &Ty, field: &FieldKey) -> Option<Ty> {
        match (ty, field) {
            (Ty::Tuple(fields), FieldKey::Tuple(index)) => fields.get(*index).cloned(),
            _ => None,
        }
    }

    fn semantic_type_subst(&self, ty: &NominalTy) -> Result<TypeSubst, PackageStoreError> {
        Ok(self
            .context
            .item_query()
            .generic_params_for_type_def(ty.def)?
            .map(|generics| TypeSubst::from_generics(generics, &ty.args))
            .unwrap_or_else(TypeSubst::new))
    }
}
