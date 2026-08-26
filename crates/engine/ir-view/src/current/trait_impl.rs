//! Trait-specific queries over one request-local impl.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, ModuleRef, TraitDefRef};
use rg_parse::{FileId, LineIndex};
use rg_syntax::ast;
use rg_ty::{SemanticSignatureQuery, TraitApplication};

use super::CurrentImplView;
use crate::{
    IndexedViewDb,
    trait_impl::{MissingTraitMember, TraitImplView},
};

/// Semantic interpretation of one trait impl that exists only in the request source.
///
/// `CurrentImplView` supplies the ordinary impl identity and path semantics. This wrapper retains
/// the resolved trait application needed to compare the edited member list with the trait's saved
/// declarations.
pub struct CurrentTraitImplView<'db> {
    current: CurrentImplView<'db>,
    trait_ref: TraitDefRef,
    application: TraitApplication,
}

impl<'db> CurrentTraitImplView<'db> {
    /// Lower one current trait impl and resolve its trait application.
    pub fn new(
        db: &IndexedViewDb<'db>,
        crate_ref: CrateRef,
        file_id: FileId,
        fallback_module: ModuleRef,
        line_index: &LineIndex,
        impl_: &ast::Impl,
    ) -> anyhow::Result<Option<Self>> {
        if impl_.trait_().is_none() {
            return Ok(None);
        }
        let Some(current) =
            CurrentImplView::new(db, crate_ref, file_id, fallback_module, line_index, impl_)?
        else {
            return Ok(None);
        };
        let signatures =
            SemanticSignatureQuery::with_resolver(current.db(), current.db(), current.db());
        let Some(header) = signatures
            .impl_header(current.impl_ref())
            .context("resolve current trait impl header")?
        else {
            return Ok(None);
        };
        let Some(trait_lowering) = header.trait_ref else {
            return Ok(None);
        };
        let application = trait_lowering.application;
        let trait_ref = application.def;

        Ok(Some(Self {
            current,
            trait_ref,
            application,
        }))
    }

    /// Return trait declarations absent from the impl currently shown in the editor.
    pub fn missing_members(&self) -> anyhow::Result<Vec<MissingTraitMember>> {
        TraitImplView::new(self.current.db()).missing_members_for_application(
            self.current.impl_ref(),
            self.trait_ref,
            &self.application,
        )
    }
}
