//! Concrete navigation target projection.

use anyhow::Context as _;
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

    pub(crate) fn target_for_declaration(
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
        let declaration = declarations
            .declaration(DeclarationRef::module(module_ref))
            .context("read module declaration")?;
        let name = match &declaration {
            Some(declaration) => declarations
                .declaration_site_name(declaration)
                .context("render module declaration name")?
                .to_string(),
            None => "crate".to_string(),
        };

        if let Some(file_id) = declarations
            .module_definition_file(module_ref)
            .context("find module definition file")?
        {
            // `mod foo;` points to the file containing foo's contents. Like a crate root, that
            // file has no name span to select, so navigation lands at the start of the file.
            return Ok(Some(NavigationTarget {
                crate_ref: module_ref.origin.origin_crate(),
                source: NavigationTargetSource::Saved,
                kind: NavigationTargetKind::Module,
                name,
                file_id,
                span: None,
            }));
        }

        // Inline modules are defined at their declaration. Keep the declaration as a fallback
        // for `mod foo;` too when its file could not be resolved.
        let Some(declaration) = declaration else {
            return Ok(None);
        };
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

    /// A declaration is current only when its identity belongs to this request's source overlay.
    /// Numeric ranges cannot answer this: saved and current text may put unrelated declarations at
    /// the same offsets.
    fn source_for_declaration(&self, declaration: DeclarationRef) -> NavigationTargetSource {
        match declaration.origin() {
            origin @ DefMapRef::Body(_) if self.0.is_current_origin(origin) => {
                NavigationTargetSource::Current
            }
            DefMapRef::Crate(_) | DefMapRef::Body(_) => NavigationTargetSource::Saved,
        }
    }
}
