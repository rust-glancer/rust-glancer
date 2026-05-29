//! Concrete navigation target projection.

use rg_ir_model::{
    ModuleRef,
    identity::{DeclarationRef, DeclarationRefRepr},
};
use rg_ir_view::{
    IndexedViewDb,
    item::declaration::{Declaration, DeclarationView},
    item::module::ModuleView,
};

use crate::model::{NavigationTarget, NavigationTargetKind};

/// Converts stable IR identities into concrete editor navigation targets.
///
/// This projection does not decide what the cursor means. It receives already-resolved def-map,
/// semantic IR, or body IR IDs and projects them into the public `NavigationTarget` shape.
pub(crate) struct NavigationTargetProjection<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> NavigationTargetProjection<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    pub(crate) fn targets_for_declarations(
        &self,
        declarations: impl IntoIterator<Item = DeclarationRef>,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let mut targets = Vec::new();
        for declaration in declarations {
            if let Some(target) = self.target_for_declaration(declaration)?
                && !targets.contains(&target)
            {
                targets.push(target);
            }
        }
        Ok(targets)
    }

    fn target_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match declaration.repr() {
            DeclarationRefRepr::Module(module) => self.target_for_module(module),
            DeclarationRefRepr::NameDef(_)
            | DeclarationRefRepr::Item(_)
            | DeclarationRefRepr::Function(_)
            | DeclarationRefRepr::Field(_)
            | DeclarationRefRepr::EnumVariant(_)
            | DeclarationRefRepr::Binding(_)
            | DeclarationRefRepr::Impl(_) => Ok(DeclarationView::new(self.0)
                .declaration(declaration)?
                .map(Self::navigation_target)),
        }
    }

    fn target_for_module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<NavigationTarget>> {
        if let Some(file_id) = ModuleView::new(self.0).root_file(module_ref)? {
            // Root modules have no declaration name to jump to, so they navigate to the owning
            // file. Named modules are ordinary declarations.
            return Ok(Some(NavigationTarget {
                target: module_ref.origin.origin_target(),
                kind: NavigationTargetKind::Module,
                name: "crate".to_string(),
                file_id,
                span: None,
            }));
        }

        Ok(DeclarationView::new(self.0)
            .declaration(DeclarationRef::module(module_ref))?
            .map(|declaration| NavigationTarget {
                target: declaration.target(),
                kind: NavigationTargetKind::from(declaration.kind()),
                name: declaration.name().to_string(),
                file_id: declaration.file_id(),
                span: Some(declaration.span()),
            }))
    }

    fn navigation_target(declaration: Declaration) -> NavigationTarget {
        NavigationTarget {
            target: declaration.target(),
            kind: NavigationTargetKind::from(declaration.kind()),
            name: declaration.name().to_string(),
            file_id: declaration.file_id(),
            span: Some(declaration.selection_span()),
        }
    }
}
