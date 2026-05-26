//! Concrete navigation target projection.

use rg_def_map::{ModuleOrigin, ModuleRef};

use crate::{
    api::{
        Analysis,
        view::declaration::{Declaration, DeclarationRef, DeclarationView},
    },
    model::{NavigationTarget, NavigationTargetKind},
};

/// Converts stable IR identities into concrete editor navigation targets.
///
/// This resolver does not decide what the cursor means. It receives already-resolved def-map,
/// semantic IR, or body IR IDs and projects them into the public `NavigationTarget` shape.
pub(crate) struct NavigationTargetResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> NavigationTargetResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    fn navigation_target_for_module(
        &self,
        module_ref: ModuleRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
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

        Ok(self
            .declaration(DeclarationRef::Module(module_ref))?
            .map(|declaration| NavigationTarget {
                target: declaration.target(),
                kind: NavigationTargetKind::from(declaration.kind()),
                name: declaration.name().to_string(),
                file_id: declaration.file_id(),
                span: Some(declaration.span()),
            }))
    }

    pub(crate) fn navigation_targets_for_declarations(
        &self,
        declarations: Vec<DeclarationRef>,
    ) -> anyhow::Result<Vec<NavigationTarget>> {
        let mut targets = Vec::new();
        for declaration in declarations {
            if let Some(target) = self.navigation_target_for_declaration(declaration)? {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
        }
        Ok(targets)
    }

    pub(crate) fn navigation_target_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match declaration {
            DeclarationRef::Module(module) => self.navigation_target_for_module(module),
            DeclarationRef::LocalDef(_) | DeclarationRef::Semantic(_) | DeclarationRef::Body(_) => {
                Ok(self.declaration(declaration)?.map(NavigationTarget::from))
            }
        }
    }

    fn declaration(&self, declaration: DeclarationRef) -> anyhow::Result<Option<Declaration>> {
        DeclarationView::new(self.0).declaration(declaration)
    }
}
