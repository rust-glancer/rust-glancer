//! Shared semantic context and source edits for module-level imports.
//!
//! Rust looks up type names and value names separately. For example, `User` in `let _: User` needs
//! a type import, while `make_user` in `make_user()` needs a value import. Completion already knows
//! both the grammar position and the enclosing module. `ImportContext` exposes that same answer to
//! completion and code actions so both features search for the same kinds of imports. The `edit`
//! module owns conservative insertion and coalescing of `use` items.

mod edit;

pub(crate) use edit::{ImportEditPlan, ImportEditPlanner};

use anyhow::Context as _;
use rg_ir_model::{BodyRef, CrateRef, ModuleRef};
use rg_ir_view::{
    IndexedViewDb,
    lookup::name::NameNamespace,
    source::{
        IndexedQualifiedPathContext, IndexedQualifiedPathScope, IndexedQualifiedPathSite,
        IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope, IndexedUnqualifiedNameSite,
        SourceCompletionView,
    },
    ty::locals::BodyView,
};
use rg_parse::{FileId, enclosing_inline_module_path};
use rg_syntax::{AstNode as _, ast};

/// The two semantic facts needed to search for and place an import.
///
/// `module` says which module will receive the `use`; `namespace` says whether the source position
/// needs a type or a value. Keeping them together prevents import consumers from independently
/// interpreting completion-site scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImportContext {
    module: ModuleRef,
    namespace: NameNamespace,
}

impl ImportContext {
    /// Read the import module and namespace from an ordinary unqualified completion site.
    ///
    /// Const and pattern positions return `None`: deciding which declarations are valid there
    /// needs more information than choosing the type or value namespace used by these imports.
    pub(crate) fn for_unqualified_site(
        db: &IndexedViewDb<'_>,
        site: &IndexedUnqualifiedNameSite,
    ) -> anyhow::Result<Option<Self>> {
        let context = match site.scope() {
            IndexedUnqualifiedNameScope::Body {
                scope,
                context: IndexedUnqualifiedNameContext::Type { .. },
                ..
            } => Self::for_body(db, scope.body_ir(), NameNamespace::Types)?,
            IndexedUnqualifiedNameScope::Body {
                scope,
                context: IndexedUnqualifiedNameContext::Value,
                ..
            } => Self::for_body(db, scope.body_ir(), NameNamespace::Values)?,
            IndexedUnqualifiedNameScope::Signature {
                scope,
                context: IndexedUnqualifiedNameContext::Type { .. },
                ..
            } => Some(Self::new(scope.context().module, NameNamespace::Types)),
            IndexedUnqualifiedNameScope::Signature {
                scope,
                context: IndexedUnqualifiedNameContext::Value,
                ..
            } => Some(Self::new(scope.context().module, NameNamespace::Values)),
            IndexedUnqualifiedNameScope::Module {
                module,
                context: IndexedUnqualifiedNameContext::Type { .. },
                ..
            } => Some(Self::new(*module, NameNamespace::Types)),
            IndexedUnqualifiedNameScope::Module {
                module,
                context: IndexedUnqualifiedNameContext::Value,
                ..
            } => Some(Self::new(*module, NameNamespace::Values)),
            IndexedUnqualifiedNameScope::Body { .. }
            | IndexedUnqualifiedNameScope::Signature { .. }
            | IndexedUnqualifiedNameScope::Module { .. }
            | IndexedUnqualifiedNameScope::Import { .. } => return Ok(None),
        };
        Ok(context)
    }

    /// Read the import module and namespace for the last name of a qualified path.
    ///
    /// A path in a signature always names a type. Imports, patterns, and const-specific positions
    /// return `None` because the qualified-path action has no rewrite policy for them.
    pub(crate) fn for_qualified_site(
        db: &IndexedViewDb<'_>,
        site: &IndexedQualifiedPathSite,
    ) -> anyhow::Result<Option<Self>> {
        let context = match site.scope() {
            IndexedQualifiedPathScope::Body {
                scope,
                context: IndexedQualifiedPathContext::Type,
            } => Self::for_body(db, scope.body_ir(), NameNamespace::Types)?,
            IndexedQualifiedPathScope::Body {
                scope,
                context: IndexedQualifiedPathContext::Value,
            } => Self::for_body(db, scope.body_ir(), NameNamespace::Values)?,
            IndexedQualifiedPathScope::Signature { scope } => {
                Some(Self::new(scope.context().module, NameNamespace::Types))
            }
            IndexedQualifiedPathScope::Body {
                context:
                    IndexedQualifiedPathContext::Const | IndexedQualifiedPathContext::Pattern(_),
                ..
            }
            | IndexedQualifiedPathScope::Import { .. } => return Ok(None),
        };
        Ok(context)
    }

    /// Find the module for an unresolved first segment such as `HashMap` in `HashMap::new()`.
    ///
    /// Every path segment before the last resolves in Rust's type namespace. This method checks
    /// that `segment` is the root of a longer path, then maps its current inline-module syntax to
    /// the corresponding saved module without requiring the surrounding expression to be saved.
    /// The caller separately checks that the name does not already resolve.
    pub(crate) fn for_qualified_root(
        db: &IndexedViewDb<'_>,
        crate_ref: CrateRef,
        file_id: FileId,
        segment: &ast::PathSegment,
    ) -> anyhow::Result<Option<Self>> {
        let path = segment.parent_path();
        if path.qualifier().is_some()
            || path.parent_path().is_none()
            || segment.coloncolon_token().is_some()
        {
            return Ok(None);
        }

        let inline_module_path = enclosing_inline_module_path(segment.syntax())
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let Some(module_site) = SourceCompletionView::new(db)
            .module_syntax_source_site(crate_ref, file_id, &inline_module_path)
            .context("read qualified-root import module")?
        else {
            return Ok(None);
        };
        Ok(Some(Self::new(module_site.module(), NameNamespace::Types)))
    }

    pub(crate) fn module(self) -> ModuleRef {
        self.module
    }

    pub(crate) fn namespace(self) -> NameNamespace {
        self.namespace
    }

    fn new(module: ModuleRef, namespace: NameNamespace) -> Self {
        Self { module, namespace }
    }

    fn for_body(
        db: &IndexedViewDb<'_>,
        body: BodyRef,
        namespace: NameNamespace,
    ) -> anyhow::Result<Option<Self>> {
        let Some(module) = BodyView::new(db)
            .owner_module(body)
            .context("read import owner module")?
        else {
            return Ok(None);
        };
        Ok(Some(Self::new(module, namespace)))
    }
}
