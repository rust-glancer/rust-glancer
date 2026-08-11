//! Field access resolution.

use rg_def_map::DefMapSource;
use rg_ir_model::{EnumVariantRef, FieldKey, FieldRef, TypeDefId, identity::DeclarationRef};
use rg_item_tree::{FieldItem, FieldList};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_std::{ExpectedUnique, UniqueVec};
use rg_ty::{AdtTy, AutoderefMode, ReferencePeelingCandidates, Ty};

use crate::{BodyPath, ir::resolved::BodyResolution, resolution::BodyResolutionContext};

/// One field projection at the selected receiver-adjustment depth.
///
/// Nominal fields retain their declaration identity for navigation and display. Tuple fields are
/// structural language elements, so they contribute a type without inventing a declaration.
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
    /// Return the selected semantic field declaration.
    pub(crate) fn field(&self) -> FieldRef {
        self.field
    }

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
            match target {
                ResolvedFieldTarget::Declared(target) => {
                    fields.push(target.field);
                }
                ResolvedFieldTarget::Structural { .. } => {
                    return BodyResolution::Unknown;
                }
            };
        }

        if fields.is_empty() {
            BodyResolution::Unknown
        } else {
            BodyResolution::Declarations(fields.into_iter().map(DeclarationRef::from).collect())
        }
    }

    /// Return the selected field type only when receiver adjustment was unambiguous.
    pub(crate) fn single_ty(&self) -> Option<&Ty> {
        match self.targets.as_one()? {
            ResolvedFieldTarget::Declared(target) => target.ty(),
            ResolvedFieldTarget::Structural { ty } => Some(ty),
        }
    }

    /// Add a declared field target.
    fn push_declared(&mut self, target: DeclaredFieldTarget) {
        self.targets.push(ResolvedFieldTarget::Declared(target));
    }

    /// Add a structural field type with no declaration.
    fn push_structural(&mut self, ty: Ty) {
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

    /// Resolve a field through the supplied live receiver type.
    pub(crate) fn resolve_for_ty(
        &self,
        base_ty: &Ty,
        field: &FieldKey,
    ) -> Result<ResolvedFieldTargets, PackageStoreError> {
        let mut current_depth = None;
        let mut targets = ResolvedFieldTargets::new();

        for candidate in self
            .context
            .autoderef()
            .candidates(AutoderefMode::FieldLookup, base_ty)
        {
            let candidate = candidate?;
            // Field lookup stops at the first autoderef depth that has matches. Same-depth
            // alternatives stay together so ambiguous receivers do not become order-dependent.
            if current_depth.is_some_and(|depth| depth != candidate.depth()) && !targets.is_empty()
            {
                return Ok(targets);
            }
            current_depth = Some(candidate.depth());

            if let Some(ty) = Self::structural_field_ty(candidate.ty(), field) {
                targets.push_structural(ty);
            }

            for nominal_ty in candidate.ty().as_adts() {
                if let Some(target) = self.declared(nominal_ty, field)? {
                    targets.push_declared(target);
                }
            }
        }

        Ok(targets)
    }

    /// Project the field type destructured by a record or tuple-variant pattern.
    ///
    /// Pattern matching peels written references but does not use receiver autoderef. Struct and
    /// union fields come directly from the expected nominal type; enum fields additionally use the
    /// pattern path's final segment to select the variant.
    pub(crate) fn pattern_field_ty(
        &self,
        path: Option<&BodyPath>,
        expected_ty: &Ty,
        field_key: &FieldKey,
    ) -> Result<Option<Ty>, PackageStoreError> {
        let variant_path = path.and_then(BodyPath::as_def_map_path);
        let variant_name = variant_path
            .as_ref()
            .and_then(|path| path.segments().last())
            .map(rg_text::Name::as_str);
        let mut candidates = ExpectedUnique::new();

        for candidate in ReferencePeelingCandidates::new(expected_ty) {
            for nominal_ty in candidate.ty().as_adts() {
                let field_ty = match nominal_ty.def.id {
                    TypeDefId::Struct(_) | TypeDefId::Union(_) => self
                        .declared(nominal_ty, field_key)?
                        .and_then(|target| target.ty().cloned()),
                    TypeDefId::Enum(_) => {
                        let Some(variant_name) = variant_name else {
                            continue;
                        };
                        let Some(variant_ref) = self
                            .context
                            .item_query()
                            .enum_variant_ref_for_type_def(nominal_ty.def, variant_name)?
                        else {
                            continue;
                        };
                        self.enum_variant_field_ty(nominal_ty, variant_ref, field_key)?
                    }
                };
                if let Some(field_ty) = field_ty {
                    candidates.push(field_ty);
                }
            }
        }

        Ok(candidates.into_option())
    }

    /// Resolve a declared field directly from its owner type.
    pub(crate) fn declared(
        &self,
        owner_ty: &AdtTy,
        field: &FieldKey,
    ) -> Result<Option<DeclaredFieldTarget>, PackageStoreError> {
        let item_query = self.context.item_query();
        let Some(field_ref) = item_query.field_for_type(owner_ty.def, field)? else {
            return Ok(None);
        };
        let Some(_) = item_query.field_data(field_ref)? else {
            return Ok(Some(DeclaredFieldTarget {
                field: field_ref,
                ty: None,
            }));
        };

        let subst = self.context.generics().subst_for_nominal_ty(owner_ty)?;
        let ty = self
            .context
            .signatures()
            .field_ty(field_ref)?
            .map(|ty| subst.apply(&ty));

        Ok(Some(DeclaredFieldTarget {
            field: field_ref,
            ty,
        }))
    }

    /// Return the type of an enum variant field for a known enum type.
    pub(crate) fn enum_variant_field_ty(
        &self,
        enum_ty: &AdtTy,
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
        let Some((field_index, _field)) =
            Self::variant_field(&variant_data.variant.fields, field_key)
        else {
            return Ok(None);
        };
        let subst = self.context.generics().subst_for_nominal_ty(enum_ty)?;
        Ok(self
            .context
            .signatures()
            .enum_variant_field_ty(variant_ref, field_index)?
            .map(|ty| subst.apply(&ty)))
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
    ) -> Option<(usize, &'field FieldItem)> {
        match key {
            FieldKey::Named(_) => fields
                .fields()
                .iter()
                .enumerate()
                .find(|(_, field)| field.key.as_ref() == Some(key)),
            FieldKey::Tuple(index) => fields
                .fields()
                .get(*index)
                .filter(|field| field.key.as_ref() == Some(key))
                .map(|field| (*index, field)),
        }
    }
}
