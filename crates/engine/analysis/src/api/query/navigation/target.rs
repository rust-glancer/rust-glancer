//! Concrete navigation target projection.

use rg_def_map::{ModuleOrigin, ModuleRef};

use crate::{
    api::{
        Analysis,
        view::declaration::{DeclarationRef, DeclarationView},
    },
    model::{NavigationTarget, NavigationTargetKind},
};

/// Converts stable IR identities into concrete editor navigation targets.
///
/// This projection does not decide what the cursor means. It receives already-resolved def-map,
/// semantic IR, or body IR IDs and projects them into the public `NavigationTarget` shape.
pub(crate) struct NavigationTargetProjection<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> NavigationTargetProjection<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
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
        match declaration {
            DeclarationRef::Module(module) => self.target_for_module(module),
            DeclarationRef::LocalDef(_) | DeclarationRef::Semantic(_) | DeclarationRef::Body(_) => {
                Ok(DeclarationView::new(self.0)
                    .declaration(declaration)?
                    .map(NavigationTarget::from))
            }
        }
    }

    fn target_for_module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<NavigationTarget>> {
        let Some(module) = self.0.def_map.module(module_ref)? else {
            return Ok(None);
        };
        if let ModuleOrigin::Root { file_id } = module.origin {
            // Root modules have no declaration name to jump to, so they navigate to the owning
            // file. Named modules are ordinary declarations.
            return Ok(Some(NavigationTarget {
                target: module_ref.target,
                kind: NavigationTargetKind::Module,
                name: "crate".to_string(),
                file_id,
                span: None,
            }));
        };

        Ok(DeclarationView::new(self.0)
            .declaration(DeclarationRef::Module(module_ref))?
            .map(|declaration| NavigationTarget {
                target: declaration.target(),
                kind: NavigationTargetKind::from(declaration.kind()),
                name: declaration.name().to_string(),
                file_id: declaration.file_id(),
                span: Some(declaration.span()),
            }))
    }
}
