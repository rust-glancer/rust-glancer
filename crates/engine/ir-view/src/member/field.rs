//! Field lookup and constructor-shape projection for nominal types and enum variants.

use anyhow::Context as _;
use rg_ir_model::{
    BodyRef, CrateRef, EnumVariantFieldRef, EnumVariantRef, FieldRef, Path, ScopeId, TypeDefRef,
    identity::DeclarationRef,
};
use rg_semantic_ir::{ItemStoreQuery, TypePathResolution};
use rg_ty::{MemberQuery, TyContext};

use super::{ConstructorShape, MemberEnumVariantField, MemberField, MemberView};
use crate::{
    body::BodyResolutionView,
    ty::{IndexedType, TyView},
};

impl<'a, 'db> MemberView<'a, 'db> {
    /// Return fields visible for a type at a crate use site.
    pub fn field_candidates_for_ty<'view>(
        &'view self,
        use_site: CrateRef,
        ty: &IndexedType,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        let item_lookup_query = self
            .db
            .item_lookup_query(use_site)
            .context("assemble field candidate item lookup")?;
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            item_lookup_query,
            self.db.trait_selection(use_site),
        ));
        for field_ref in member_query
            .fields_for_ty(ty.raw())
            .context("resolve field candidates for type")?
        {
            let Some(field) = self.field(field_ref).context("read field candidate data")? else {
                continue;
            };
            fields.push(field);
        }
        Ok(fields)
    }

    /// Resolve a body type path and return its declared fields.
    pub fn field_candidates_for_body_type_path<'view>(
        &'view self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let Some(resolution) = BodyResolutionView::new(self.db)
            .type_path_resolution(body, scope, path)
            .context("resolve body field type path")?
        else {
            return Ok(Vec::new());
        };

        let mut fields = Vec::new();
        let item_lookup_query = self
            .db
            .item_lookup_query(body.crate_ref)
            .context("assemble body field item lookup")?;
        let member_query = MemberQuery::new(TyContext::new(
            self.db,
            self.db,
            item_lookup_query,
            self.db.trait_selection_for_body(body),
        ));
        if let TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty) = resolution {
            for field_ref in member_query
                .fields_for_type_def(ty)
                .context("resolve fields for body type path")?
            {
                let Some(field) = self.field(field_ref).context("read body type path field")?
                else {
                    continue;
                };
                fields.push(field);
            }
        }

        Ok(fields)
    }

    /// Return fields declared directly by a resolved nominal type.
    pub fn field_candidates_for_type_def<'view>(
        &'view self,
        owner: TypeDefRef,
    ) -> anyhow::Result<Vec<MemberField<'view>>> {
        let mut fields = Vec::new();
        for field_ref in ItemStoreQuery::new(self.db)
            .fields_for_type(owner)
            .context("read type definition fields")?
        {
            let Some(field) = self
                .field(field_ref)
                .context("read type definition field")?
            else {
                continue;
            };
            fields.push(field);
        }
        Ok(fields)
    }

    /// Return fields declared directly by one resolved enum variant.
    pub fn field_candidates_for_enum_variant<'view>(
        &'view self,
        owner: EnumVariantRef,
    ) -> anyhow::Result<Vec<MemberEnumVariantField<'view>>> {
        let mut fields = Vec::new();
        for field_ref in ItemStoreQuery::new(self.db)
            .fields_for_enum_variant(owner)
            .context("read enum variant fields")?
        {
            let Some(field) = self
                .enum_variant_field(field_ref)
                .context("read enum variant field")?
            else {
                continue;
            };
            fields.push(field);
        }
        Ok(fields)
    }

    /// Return borrowed data for one field.
    pub fn field(&self, field: FieldRef) -> anyhow::Result<Option<MemberField<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .field_data(field)
            .context("read field data")?
            .map(|data| MemberField { field, data }))
    }

    /// Return borrowed data for a field declared by an enum variant.
    pub fn enum_variant_field(
        &self,
        field: EnumVariantFieldRef,
    ) -> anyhow::Result<Option<MemberEnumVariantField<'_>>> {
        Ok(ItemStoreQuery::new(self.db)
            .enum_variant_field_data(field)
            .context("read enum variant field data")?
            .map(|data| MemberEnumVariantField { field, data }))
    }

    /// Return the constructor shape behind a visible declaration spelling.
    ///
    /// Type aliases may resolve to one nominal struct. Keeping that projection here lets pattern
    /// completion make a semantic decision without inspecting declaration labels or signatures.
    pub fn constructor_shape_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<ConstructorShape>> {
        if let DeclarationRef::EnumVariant(variant) = declaration {
            return Ok(self
                .enum_variant(variant)
                .context("read constructor enum variant")?
                .map(|variant| variant.constructor_shape()));
        }
        let Some(ty) = TyView::new(self.db)
            .ty_for_declaration(declaration)
            .context("resolve constructor declaration type")?
        else {
            return Ok(None);
        };
        let Some(owner) = ty.unique_nominal_type_def().into_option() else {
            return Ok(None);
        };
        self.constructor_shape_for_type_def(owner)
    }

    /// Return the constructor shape for a nominal struct type.
    pub fn constructor_shape_for_type_def(
        &self,
        owner: TypeDefRef,
    ) -> anyhow::Result<Option<ConstructorShape>> {
        Ok(ItemStoreQuery::new(self.db)
            .struct_fields_for_type_def(owner)
            .context("read constructor fields")?
            .map(ConstructorShape::from_fields))
    }
}
