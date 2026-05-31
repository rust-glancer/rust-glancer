//! Composite enum-variant view over indexed enum declarations.

use rg_body_ir::BodyTypePathResolution;
use rg_def_map::Path;
use rg_ir_model::{
    BodyRef, EnumVariantRef, ScopeId, TypeDefId, TypeDefRef, hir::items::EnumVariantData,
};
use rg_semantic_ir::Documentation;

use crate::{IndexedViewDb, item::query::ItemQuery};

/// Borrowed data for one resolved enum variant, independent from the storage layer it came from.
#[derive(Debug, Clone, Copy)]
pub struct EnumVariant<'a> {
    variant: EnumVariantRef,
    data: EnumVariantData<'a>,
}

impl<'a> EnumVariant<'a> {
    pub fn variant_ref(&self) -> EnumVariantRef {
        self.variant
    }

    pub fn name(&self) -> &'a str {
        self.data.variant.name.as_str()
    }

    pub fn docs_text(&self) -> Option<String> {
        self.docs().map(Documentation::text)
    }

    fn docs(&self) -> Option<&'a Documentation> {
        self.data.variant.docs.as_ref()
    }
}

pub struct EnumVariantView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> EnumVariantView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    pub fn variants_for_body_type_path(
        &self,
        body: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<EnumVariant<'_>>> {
        let resolution = self.db.body_ir.resolve_type_path_in_scope(
            &self.db.def_map,
            &self.db.semantic_ir,
            body,
            scope,
            path,
        )?;
        let mut variants = Vec::new();

        match resolution {
            BodyTypePathResolution::TypeDefs(types) | BodyTypePathResolution::SelfType(types) => {
                for ty in types {
                    variants.extend(self.semantic_variants(ty)?);
                }
            }
            BodyTypePathResolution::Primitive(_)
            | BodyTypePathResolution::TypeAliases(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => {}
        }

        Ok(variants)
    }

    fn semantic_variants(&self, ty: TypeDefRef) -> anyhow::Result<Vec<EnumVariant<'_>>> {
        let TypeDefId::Enum(enum_id) = ty.id else {
            return Ok(Vec::new());
        };
        let Some(data) = ItemQuery::new(self.db).enum_data_for_type_def(ty)? else {
            return Ok(Vec::new());
        };
        let variant_refs = (0..data.variants.len()).map(|index| EnumVariantRef {
            origin: ty.origin,
            enum_id,
            index,
        });

        let mut variants = Vec::new();
        for variant_ref in variant_refs {
            let Some(variant) = self.variant(variant_ref)? else {
                continue;
            };
            variants.push(variant);
        }

        Ok(variants)
    }

    pub fn variant(&self, variant: EnumVariantRef) -> anyhow::Result<Option<EnumVariant<'_>>> {
        Ok(ItemQuery::new(self.db)
            .enum_variant_data(variant)?
            .map(|data| EnumVariant { variant, data }))
    }
}
