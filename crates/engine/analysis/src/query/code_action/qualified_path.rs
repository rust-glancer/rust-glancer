//! Replace a fully written item path with an import and its last name.
//!
//! ```text
//! fn load(_: crate::models::User) {}
//!
//! // becomes:
//! use crate::models::User;
//! fn load(_: User) {}
//! ```
//!
//! The important question is what `User` would mean at this exact source position after
//! `crate::models::` is removed. If no `User` is visible, the action adds an import. If an existing
//! `User` already means the same item, the action only removes the qualifier. If `User` could mean
//! a different item or a generic parameter, no action is offered.
//!
//! Only the last name of the complete path is considered. The provider does not rewrite an
//! intermediate `State` inside `crate::models::State::Ready`, and enum variants are not import
//! targets for this action.

use anyhow::Context as _;
use rg_ir_model::{GenericDefRef, identity::DeclarationRef};
use rg_ir_view::{
    SymbolKind,
    display::syntax::SyntaxRenderer,
    lookup::{
        importable::{ImportableName, ImportableNameSearch},
        name::{GenericScopeNameKind, NameLookupView, NameNamespace, ValueOrTypeNamespace},
        resolution::ResolutionView,
    },
    source::{IndexedQualifiedPathScope, IndexedSourceSurface},
    ty::locals::{BodyLexicalName, BodyNameScope, BodyView},
};
use rg_parse::{Span, TextSpan};
use rg_syntax::{AstNode as _, ast};

use crate::{
    Analysis, CodeAction, CodeActionEdit, CodeActionKind, CodeActionQuery, CodeActionTrigger,
    query::{
        completion::{CompletionSiteDetector, PathCompletionSite},
        import::{ImportContext, ImportEditPlan, ImportEditPlanner},
    },
    source_symbol::{SourceSymbolResolver, SourceSymbolRole},
};

use super::syntax::{CodeActionSyntax, PathNameSyntax};

/// What the last name of a qualified path would mean after its qualifier is removed.
///
/// For `crate::models::User`, the name being checked is `User` and the intended declaration is the
/// struct reached by the complete path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortNameState {
    /// Nothing named `User` is visible, so the action must add a `use` for the intended item.
    Free,
    /// Every visible `User` found by the checks refers to the intended item already.
    ResolvesToTarget,
    /// A visible `User` refers elsewhere, or a generic parameter named `User` would shadow it.
    Conflict,
}

/// Builds the rewrite only when removing a qualifier cannot change which item the path names.
pub(super) struct QualifiedPathCodeActionProvider<'analysis, 'db, 'source> {
    analysis: &'analysis Analysis<'db>,
    query: CodeActionQuery<'source>,
}

impl<'analysis, 'db, 'source> QualifiedPathCodeActionProvider<'analysis, 'db, 'source> {
    pub(super) fn new(analysis: &'analysis Analysis<'db>, query: CodeActionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Build the edits for one path whose last name keeps referring to the same item.
    ///
    /// Keeping the leaf token untouched preserves raw identifiers and generic arguments such as
    /// `crate::models::User<'a>`; the edit removes only `crate::models::`.
    pub(super) fn code_action(
        &self,
        syntax: &CodeActionSyntax<'_>,
    ) -> anyhow::Result<Option<CodeAction>> {
        if self.query.trigger != CodeActionTrigger::Invoked {
            return Ok(None);
        }

        // 1. Check that the request touches the last name of a path shape this action can edit.
        // Completion's site classifier retains the semantic scope needed to decide whether that
        // name is used as a type or value and which module would own a new `use` item.
        let Some(path_name) = self.qualified_path_at_request(syntax) else {
            return Ok(None);
        };
        let path = path_name.path();
        let leaf = path_name.name();
        let leaf_span = Span::from_text_range(leaf.syntax().text_range());
        let Some(site) = CompletionSiteDetector::new(self.analysis)
            .qualified_path_for_source(
                self.query.crate_ref,
                self.query.file_id,
                syntax.source(),
                leaf_span.text.end,
            )
            .context("classify qualified path action site")?
        else {
            return Ok(None);
        };
        if site.replace_span().text.end != leaf_span.text.end {
            return Ok(None);
        }

        // 2. Translate that scope into an import module and namespace, then record the item named
        // by the complete path before changing any source. Every later check compares against this
        // declaration; matching the text `User` alone is not enough.
        let Some(import_context) =
            ImportContext::for_qualified_site(self.analysis.view_db(), site.source())
                .context("read qualified path import context")?
        else {
            return Ok(None);
        };
        let Some(target) = self
            .resolved_target(leaf_span)
            .context("resolve qualified path action target")?
        else {
            return Ok(None);
        };

        // 3. Find one `use` path for that exact item, then ask what the remaining last name would
        // resolve to at this source position. A different visible `User` makes the rewrite unsafe.
        let label = leaf.text();
        let label = label.strip_prefix("r#").unwrap_or(&label);
        let Some(candidate) = self
            .exact_import_candidate(
                import_context.module(),
                import_context.namespace(),
                label,
                target,
            )
            .context("find exact qualified path import")?
        else {
            return Ok(None);
        };
        let short_name_state = self
            .short_name_state(
                &site,
                import_context.module(),
                import_context.namespace(),
                label,
                target,
            )
            .context("check qualified path short-name conflicts")?;
        if short_name_state == ShortNameState::Conflict {
            return Ok(None);
        }

        // 4. The qualifier deletion is always needed. Add an import only when the last name was
        // previously unused; an existing name for the same declaration needs no second `use`.
        let qualifier_edit = CodeActionEdit {
            replace: Span {
                text: TextSpan {
                    start: u32::from(path.syntax().text_range().start()),
                    end: leaf_span.text.start,
                },
            },
            new_text: String::new(),
        };
        let edition = self
            .analysis
            .view_db()
            .crate_edition(self.query.crate_ref)
            .context("read qualified path action edition")?;
        let rendered_path = SyntaxRenderer::new(edition)
            .path(candidate.path())
            .to_string();
        let mut edits = vec![qualifier_edit];
        if short_name_state == ShortNameState::Free {
            let planner =
                ImportEditPlanner::for_source(syntax.source(), syntax.file(), leaf_span.text.start);
            match planner.plan(candidate.path(), &rendered_path) {
                ImportEditPlan::AlreadyImported => {}
                ImportEditPlan::Edit(edit) => edits.push(edit),
                ImportEditPlan::Unavailable => return Ok(None),
            }
        }
        edits.sort_by_key(|edit| (edit.replace.text.start, edit.replace.text.end));

        Ok(Some(CodeAction {
            title: "Replace qualified path with `use`".to_string(),
            kind: CodeActionKind::RefactorRewrite,
            is_preferred: false,
            edits,
        }))
    }

    /// Select a complete path whose last name is the only token this action needs to keep.
    ///
    /// A request on `User` in `crate::models::User` returns that full path and the `User` leaf. A
    /// request on the intermediate `State` in `crate::models::State::Ready` is rejected because
    /// deleting text before `State` would leave the unhandled `::Ready` suffix. `Self::...`,
    /// type-qualified paths such as `<T as Trait>::Item`, and paths inside `use` items also need
    /// different rewrite rules and are left alone.
    fn qualified_path_at_request(&self, syntax: &CodeActionSyntax<'_>) -> Option<PathNameSyntax> {
        let path_name = syntax.path_name_at_start()?;
        let path = path_name.path();
        if !syntax.request_applies_to(path_name.name())
            || path.parent_path().is_some()
            || path.qualifier().is_none()
            || path
                .first_segment()
                .is_some_and(|first| first.self_type_token().is_some())
            || path
                .segments()
                .any(|segment| segment.type_anchor().is_some())
            || path
                .syntax()
                .ancestors()
                .any(|node| ast::Use::can_cast(node.kind()))
        {
            return None;
        }
        Some(path_name)
    }

    /// Find the one declaration named by the complete path before it is shortened.
    ///
    /// Source indexing identifies the final token as a reference, and resolution uses its path
    /// context to find the declaration. Zero results mean the path is unresolved; multiple results
    /// mean we cannot prove which item the rewrite must preserve. Both cases decline the action.
    fn resolved_target(&self, leaf_span: Span) -> anyhow::Result<Option<DeclarationRef>> {
        let Some(symbol) = self
            .analysis
            .source_symbol_at_for_query(
                self.query.crate_ref,
                self.query.file_id,
                leaf_span.text.start,
            )
            .context("find qualified path source symbol")?
        else {
            return Ok(None);
        };
        if symbol.role() != SourceSymbolRole::Reference
            || symbol.surface() != &IndexedSourceSurface::Plain
            || !symbol.span().touches(leaf_span.text.start)
        {
            return Ok(None);
        }

        let declarations = SourceSymbolResolver::new(self.analysis.view_db())
            .declarations_for_symbol(symbol.into_symbol())
            .context("resolve qualified path source symbol")?;
        let mut unique = Vec::new();
        for declaration in declarations {
            if !unique.contains(&declaration) {
                unique.push(declaration);
            }
        }
        Ok((unique.len() == 1).then(|| unique[0]))
    }

    /// Find one usable `use` path that reaches the same declaration as the written path.
    ///
    /// Exact-name search may find unrelated items also called `User`, aliases, or several paths to
    /// one item. This step keeps the requested type/value namespace, excludes targets such as enum
    /// variants and associated items, compares declaration identity, and proceeds only when one
    /// candidate remains.
    fn exact_import_candidate(
        &self,
        importing_module: rg_ir_model::ModuleRef,
        namespace: NameNamespace,
        label: &str,
        target: DeclarationRef,
    ) -> anyhow::Result<Option<ImportableName>> {
        let resolution = ResolutionView::new(self.analysis.view_db());
        let mut matches = Vec::new();
        for candidate in ImportableNameSearch::new(self.analysis.view_db())
            .search_exact(importing_module, label)
            .context("search qualified path import candidates")?
        {
            if candidate.name().namespace() != namespace
                || !matches!(
                    candidate.name().kind(),
                    SymbolKind::Const
                        | SymbolKind::Enum
                        | SymbolKind::Function
                        | SymbolKind::Module
                        | SymbolKind::Static
                        | SymbolKind::Struct
                        | SymbolKind::Trait
                        | SymbolKind::TypeAlias
                        | SymbolKind::Union
                )
                || resolution
                    .canonical_declaration(candidate.name().declaration())
                    .context("resolve qualified path import candidate")?
                    != target
            {
                continue;
            }
            matches.push(candidate);
        }
        Ok((matches.len() == 1).then(|| matches.remove(0)))
    }

    /// Decide what the remaining last name would resolve to after the qualifier is deleted.
    ///
    /// For `crate::models::User`, this method looks for visible declarations and generic
    /// parameters named `User` at the use site:
    ///
    /// - no `User` means `Free`, so adding the chosen import will introduce the name;
    /// - only declarations for the intended item mean `ResolvesToTarget`;
    /// - any other declaration or shadowing generic means `Conflict`.
    ///
    /// Body-local bindings are checked first, then generics and names visible from enclosing
    /// modules. The result accumulates across those places so one conflict cannot be hidden by a
    /// later matching declaration.
    fn short_name_state(
        &self,
        site: &PathCompletionSite,
        importing_module: rg_ir_model::ModuleRef,
        namespace: NameNamespace,
        label: &str,
        target: DeclarationRef,
    ) -> anyhow::Result<ShortNameState> {
        let mut state = ShortNameState::Free;
        match site.source().scope() {
            IndexedQualifiedPathScope::Body { scope, .. } => {
                let body = BodyView::new(self.analysis.view_db());
                let lexical_namespace = match namespace {
                    NameNamespace::Types => ValueOrTypeNamespace::Types,
                    NameNamespace::Values => ValueOrTypeNamespace::Values,
                    NameNamespace::Macros => return Ok(ShortNameState::Conflict),
                };
                // The qualified site does not retain the exact binding boundary. Considering all
                // bindings in the enclosing scopes may decline a path shadowed only later in the
                // body, but it cannot silently change resolution after shortening the path.
                for name in body
                    .lexical_names(BodyNameScope::new(
                        scope.body_ir(),
                        scope.scope_id(),
                        lexical_namespace,
                        usize::MAX,
                    ))
                    .context("read qualified path lexical conflicts")?
                {
                    let (candidate_label, declaration) = match name {
                        BodyLexicalName::Binding { binding, label, .. } => {
                            (label, DeclarationRef::body_binding(binding))
                        }
                        BodyLexicalName::TypeItem { item, label, .. }
                        | BodyLexicalName::ValueItem { item, label, .. } => {
                            (label, DeclarationRef::from(item))
                        }
                        BodyLexicalName::Function {
                            function, label, ..
                        } => (label, DeclarationRef::from(function)),
                    };
                    if candidate_label == label {
                        state = self
                            .merge_short_name_declaration(state, declaration, target)
                            .context("compare qualified path lexical name")?;
                    }
                }

                if let Some(owner) = body
                    .generic_owner(scope.body_ir())
                    .context("read qualified path body generic owner")?
                    && self
                        .generic_name_conflicts(owner, namespace, label)
                        .context("check qualified path body generic names")?
                {
                    return Ok(ShortNameState::Conflict);
                }
                for (_, module) in body
                    .lexical_scope_modules(scope.body_ir(), scope.scope_id())
                    .context("read qualified path body module scopes")?
                {
                    state = self
                        .module_short_name_state(state, module, namespace, label, target)
                        .context("check qualified path body module scope")?;
                }
            }
            IndexedQualifiedPathScope::Signature { scope } => {
                if self
                    .generic_name_conflicts(scope.generic_owner(), namespace, label)
                    .context("check qualified path signature generic names")?
                {
                    return Ok(ShortNameState::Conflict);
                }
            }
            IndexedQualifiedPathScope::Import { .. } => return Ok(ShortNameState::Conflict),
        }
        self.module_short_name_state(state, importing_module, namespace, label, target)
            .context("check qualified path importing module scope")
    }

    /// Check whether an owner generic would capture the name left after shortening the path.
    ///
    /// For example, inside `fn load<User>()`, changing `crate::models::User` to `User` would name
    /// the type parameter rather than the struct. Type parameters matter in the type namespace;
    /// const parameters matter in the value namespace. Lifetimes cannot capture an item name.
    fn generic_name_conflicts(
        &self,
        owner: GenericDefRef,
        namespace: NameNamespace,
        label: &str,
    ) -> anyhow::Result<bool> {
        Ok(NameLookupView::new(self.analysis.view_db())
            .generic_scope_names(owner)
            .context("read qualified path generic scope names")?
            .into_iter()
            .any(|name| {
                name.label() == label
                    && matches!(
                        (namespace, name.kind()),
                        (NameNamespace::Types, GenericScopeNameKind::Type)
                            | (NameNamespace::Values, GenericScopeNameKind::Const)
                    )
            }))
    }

    /// Check module-level declarations with the proposed name and update the answer built so far.
    ///
    /// Module lookup includes ordinary items, imports, and re-exports. The same item can appear
    /// through more than one lookup route, so matching uses declaration identity rather than the
    /// lookup record itself.
    fn module_short_name_state(
        &self,
        mut state: ShortNameState,
        module: rg_ir_model::ModuleRef,
        namespace: NameNamespace,
        label: &str,
        target: DeclarationRef,
    ) -> anyhow::Result<ShortNameState> {
        for name in NameLookupView::new(self.analysis.view_db())
            .unqualified_module_names(module)
            .context("read qualified path module names")?
            .into_iter()
            .filter(|name| name.namespace() == namespace && name.label() == label)
        {
            state = self
                .merge_short_name_declaration(state, name.declaration(), target)
                .context("compare qualified path module name")?;
        }
        Ok(state)
    }

    /// Add one visible declaration while deciding what `User` would mean here.
    ///
    /// A declaration for the intended item changes `Free` to `ResolvesToTarget`. A declaration for
    /// any other item changes the answer to `Conflict`, and that conflict remains even if another
    /// lookup later finds the intended item too.
    fn merge_short_name_declaration(
        &self,
        state: ShortNameState,
        declaration: DeclarationRef,
        target: DeclarationRef,
    ) -> anyhow::Result<ShortNameState> {
        let declaration = ResolutionView::new(self.analysis.view_db())
            .canonical_declaration(declaration)
            .context("resolve qualified path short-name declaration")?;
        Ok(if declaration == target {
            if state == ShortNameState::Conflict {
                state
            } else {
                ShortNameState::ResolvesToTarget
            }
        } else {
            ShortNameState::Conflict
        })
    }
}
