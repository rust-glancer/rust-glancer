//! Renders stable analysis identities as user-facing Rust paths.

use rg_ir_model::{
    ConstRef, EnumVariantRef, FunctionRef, ModuleRef, StaticRef, TraitRef, TypeAliasRef, TypeDefRef,
};

use crate::api::{Analysis, view::path::PathView};

pub(crate) struct PathRenderer<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> PathRenderer<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn module_path(&self, module_ref: ModuleRef) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).module_path(module_ref)
    }

    pub(crate) fn type_def_path(&self, ty: TypeDefRef) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).type_def_path(ty)
    }

    pub(crate) fn trait_path(&self, trait_ref: TraitRef) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).trait_path(trait_ref)
    }

    pub(crate) fn function_path(
        &self,
        function_ref: FunctionRef,
    ) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).function_path(function_ref)
    }

    pub(crate) fn type_alias_path(
        &self,
        type_alias_ref: TypeAliasRef,
    ) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).type_alias_path(type_alias_ref)
    }

    pub(crate) fn const_path(&self, const_ref: ConstRef) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).const_path(const_ref)
    }

    pub(crate) fn static_path(&self, static_ref: StaticRef) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).static_path(static_ref)
    }

    pub(crate) fn enum_variant_path(
        &self,
        variant_ref: EnumVariantRef,
    ) -> anyhow::Result<Option<String>> {
        PathView::new(self.0).enum_variant_path(variant_ref)
    }
}
