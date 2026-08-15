//! Concrete navigation target projection.

use rg_ir_model::{DefMapRef, ModuleRef, identity::DeclarationRef};
use rg_ir_view::{
    IndexedViewDb,
    item::declaration::{Declaration, DeclarationView},
};

use crate::model::{NavigationTarget, NavigationTargetKind, NavigationTargetSource};

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
        declaration_ref: DeclarationRef,
    ) -> anyhow::Result<Option<NavigationTarget>> {
        match declaration_ref {
            DeclarationRef::Module(module) => self.target_for_module(module),
            DeclarationRef::LocalDef(_)
            | DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => {
                let Some(declaration) =
                    DeclarationView::new(self.0).declaration(declaration_ref)?
                else {
                    return Ok(None);
                };
                Ok(Some(self.navigation_target(
                    declaration,
                    self.source_for_declaration(declaration_ref),
                )?))
            }
        }
    }

    fn target_for_module(&self, module_ref: ModuleRef) -> anyhow::Result<Option<NavigationTarget>> {
        let declarations = DeclarationView::new(self.0);
        if let Some(file_id) = declarations.root_module_file(module_ref)? {
            // Root modules have no declaration name to jump to, so they navigate to the owning
            // file. Named modules are ordinary declarations.
            return Ok(Some(NavigationTarget {
                crate_ref: module_ref.origin.origin_crate(),
                source: NavigationTargetSource::Saved,
                kind: NavigationTargetKind::Module,
                name: "crate".to_string(),
                file_id,
                span: None,
            }));
        }

        let Some(declaration) = declarations.declaration(DeclarationRef::module(module_ref))?
        else {
            return Ok(None);
        };
        let name = declarations
            .declaration_site_name(&declaration)?
            .to_string();
        Ok(Some(NavigationTarget {
            crate_ref: declaration.crate_ref(),
            source: self.source_for_declaration(DeclarationRef::Module(module_ref)),
            kind: NavigationTargetKind::from(declaration.kind()),
            name,
            file_id: declaration.file_id(),
            span: Some(declaration.selection_span()),
        }))
    }

    fn navigation_target(
        &self,
        declaration: Declaration,
        source: NavigationTargetSource,
    ) -> anyhow::Result<NavigationTarget> {
        let name = DeclarationView::new(self.0)
            .declaration_site_name(&declaration)?
            .to_string();
        Ok(NavigationTarget {
            crate_ref: declaration.crate_ref(),
            source,
            kind: NavigationTargetKind::from(declaration.kind()),
            name,
            file_id: declaration.file_id(),
            span: Some(declaration.selection_span()),
        })
    }

    /// A declaration is current only when its identity belongs to a rebuilt current body.
    /// Numeric ranges cannot answer this: saved and current text may put unrelated declarations at
    /// the same offsets.
    fn source_for_declaration(&self, declaration: DeclarationRef) -> NavigationTargetSource {
        match declaration.origin() {
            DefMapRef::Body(body_ref) if self.0.is_current_body(body_ref) => {
                NavigationTargetSource::Current
            }
            DefMapRef::Crate(_) | DefMapRef::Body(_) => NavigationTargetSource::Saved,
        }
    }
}
