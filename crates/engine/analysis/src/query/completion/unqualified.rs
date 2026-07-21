//! Unqualified completion assembly for body, signature, and import-root positions.
//!
//! A name without `.` or `::` can come from several nested scopes: body locals, declaration
//! generics, the containing module, a prelude, an extern root, or builtin primitive types. This
//! module combines those families in Rust's shadowing order before applying editor sort policy.

use std::collections::HashSet;

use anyhow::Context as _;
use rg_ir_view::{
    display::syntax::SyntaxRenderer,
    item::details::{DeclarationDetailsContext, DeclarationDetailsView},
    lookup::name::NameNamespace,
    member::MemberView,
};

use crate::{
    Analysis,
    completion_site::{UnqualifiedCompletionContext, UnqualifiedCompletionSite},
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CallCompletionKind, CompletionQuery,
    candidates::{
        CompletionCandidateSource, GenericScopeCompletionCandidate, LexicalCompletionCandidate,
        ModuleCompletionCandidate,
    },
    completion_sort::{CompletionSortPolicy, CompletionSortPriority},
    function::{FunctionCompletionRenderer, FunctionCompletionRequest},
    module_scope::{ModuleCompletionRenderer, ModuleCompletionRequest},
    primitive::PrimitiveTypeCompletionResolver,
};

/// Combines unqualified candidate families while preserving namespace-aware shadowing.
pub(super) struct UnqualifiedCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> UnqualifiedCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects unqualified completions, such as `inp$0`, `Us$0`, or `use st$0`.
    ///
    /// Generic declarations participate in the same flow. For example, `T$0` in
    /// `fn load<T>(_: T$0)` finds the function's type parameter, while `N$0` in
    /// `Array<N$0>` may find a const parameter because a bare generic argument is ambiguous.
    pub(super) fn completions(
        &self,
        site: UnqualifiedCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let context = site.context();
        let filter = UnqualifiedCompletionFilter::from(context);
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let mut completions = Vec::new();
        let mut hidden = HashSet::new();
        let syntax = SyntaxRenderer::new(
            self.analysis
                .view_db()
                .crate_edition(self.query.crate_ref)?,
        );

        let completion_candidates = CompletionCandidateSource::new(self.analysis.view_db());

        // Body-local declarations are nearest to the cursor. Record their namespace occupancy so
        // an outer generic or module name with the same spelling cannot leak into the result.
        for candidate in completion_candidates.lexical_candidates_for_unqualified(&site)? {
            if !filter.accepts_scope_namespace(candidate.namespace()) {
                continue;
            }
            self.push_lexical_completion(
                syntax,
                candidate,
                filter,
                edit,
                &mut hidden,
                &mut completions,
            )?;
        }

        // Signature generics follow lexical names but precede module/prelude candidates. This also
        // supplies inherited owner parameters and impl `Self` for declaration-owned sites.
        for candidate in completion_candidates
            .generic_scope_candidates_for_unqualified(&site)
            .context("collect generic scope completions")?
        {
            if hidden.contains(&(candidate.label().to_string(), candidate.namespace())) {
                continue;
            }
            hidden.insert((candidate.label().to_string(), candidate.namespace()));
            Self::push_generic_scope_completion(syntax, candidate, filter, edit, &mut completions);
        }

        // Module candidates already retain whether they came from the immediate scope, prelude, or
        // extern root; rendering uses that origin to keep the familiar local-first order.
        self.push_module_completions(
            completion_candidates.module_candidates_for_unqualified(&site)?,
            ModuleCompletionOptions {
                filter,
                edit,
                visible_scope_sort: match context {
                    UnqualifiedCompletionContext::Type | UnqualifiedCompletionContext::Value => {
                        VisibleScopeSort::ByOrigin
                    }
                    UnqualifiedCompletionContext::Import => VisibleScopeSort::General,
                },
                call_completion: match context {
                    UnqualifiedCompletionContext::Type | UnqualifiedCompletionContext::Value => {
                        CallCompletionKind::Call
                    }
                    UnqualifiedCompletionContext::Import => CallCompletionKind::Plain,
                },
            },
            &hidden,
            &mut completions,
        )?;

        // Primitive spellings are useful only in type positions and only when path resolution says
        // the builtin has not been shadowed in this exact body or signature scope.
        if matches!(context, UnqualifiedCompletionContext::Type) {
            completions.extend(PrimitiveTypeCompletionResolver::completions(
                completion_candidates.primitive_type_candidates_for_unqualified(&site)?,
                edit,
            ));
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn push_generic_scope_completion(
        syntax: SyntaxRenderer,
        candidate: GenericScopeCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) {
        let target = candidate.target();
        let kind = candidate.kind();
        let label = syntax.identifier(candidate.label()).to_string();
        let detail = match (kind, target) {
            (CompletionKind::TypeParameter, CompletionTarget::ImplSelf(_)) => {
                format!("self type {label}")
            }
            (CompletionKind::TypeParameter, _) => format!("type parameter {label}"),
            (CompletionKind::Const, _) => format!("const parameter {label}"),
            _ => return,
        };
        completions.push(CompletionItem {
            label: label.clone(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(detail),
            documentation: None,
            sort_text: filter.sort_policy().sort_text(
                Some(CompletionSortPriority::GenericScope),
                &label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
    }

    fn push_lexical_completion(
        &self,
        syntax: SyntaxRenderer,
        candidate: LexicalCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        hidden: &mut HashSet<(String, NameNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for namespace in candidate.shadow_namespaces() {
            hidden.insert((candidate.label().to_string(), *namespace));
        }

        if let Some(function_ref) = candidate.function_ref() {
            let members = MemberView::new(self.analysis.view_db());
            let Some(function) = members.function(function_ref)? else {
                return Ok(());
            };
            let completion = FunctionCompletionRenderer::new(self.query, syntax).completion(
                FunctionCompletionRequest {
                    function,
                    label_override: Some(candidate.label()),
                    kind: candidate.kind(),
                    applicability: CompletionApplicability::Known,
                    edit,
                    call_completion: CallCompletionKind::Plain,
                    sort_policy: filter.sort_policy(),
                    sort_priority: Some(CompletionSortPriority::body_scope(
                        candidate.scope_distance(),
                    )),
                },
            );
            completions.push(completion.item);
            return Ok(());
        }

        let Some(declaration_ref) = candidate.declaration_ref() else {
            return Ok(());
        };
        let Some(details) = DeclarationDetailsView::new(self.analysis.view_db(), syntax.edition())
            .details_for_declaration(declaration_ref, &DeclarationDetailsContext::default())?
        else {
            return Ok(());
        };
        let detail = details.signature().map(ToString::to_string);
        let documentation = details.docs().map(ToString::to_string);
        let target = candidate.target();
        let kind = candidate.kind();
        let label = syntax.identifier(candidate.label()).to_string();
        completions.push(CompletionItem {
            label: label.clone(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail,
            documentation,
            sort_text: filter.sort_policy().sort_text(
                Some(CompletionSortPriority::body_scope(
                    candidate.scope_distance(),
                )),
                &label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn push_module_completions(
        &self,
        candidates: Vec<ModuleCompletionCandidate>,
        options: ModuleCompletionOptions,
        hidden: &HashSet<(String, NameNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = ModuleCompletionRenderer::new(self.analysis, self.query)?;
        for candidate in candidates {
            if !options
                .filter
                .accepts_scope_namespace(candidate.namespace())
                || hidden.contains(&(candidate.label().to_string(), candidate.namespace()))
            {
                continue;
            }

            let Some(completion) = renderer.completion(ModuleCompletionRequest {
                candidate: &candidate,
                edit: options.edit,
                call_completion: options.call_completion,
                sort_policy: options.filter.sort_policy(),
                sort_priority: match options.visible_scope_sort {
                    VisibleScopeSort::ByOrigin => {
                        Some(CompletionSortPriority::visible_scope(candidate.origin()))
                    }
                    VisibleScopeSort::General => None,
                },
            })?
            else {
                continue;
            };
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnqualifiedCompletionFilter {
    Types,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleScopeSort {
    /// Keep import-root completions in their ordinary global order.
    General,
    /// Rank module-scope names after body-local names but before prelude and extern roots.
    ByOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModuleCompletionOptions {
    filter: UnqualifiedCompletionFilter,
    edit: CompletionEdit,
    visible_scope_sort: VisibleScopeSort,
    call_completion: CallCompletionKind,
}

impl UnqualifiedCompletionFilter {
    fn accepts_scope_namespace(self, namespace: NameNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, NameNamespace::Types),
            Self::All => true,
        }
    }

    fn sort_policy(self) -> CompletionSortPolicy {
        match self {
            Self::Types => CompletionSortPolicy::TypePosition,
            Self::All => CompletionSortPolicy::General,
        }
    }
}

impl From<UnqualifiedCompletionContext> for UnqualifiedCompletionFilter {
    fn from(context: UnqualifiedCompletionContext) -> Self {
        match context {
            UnqualifiedCompletionContext::Type => Self::Types,
            UnqualifiedCompletionContext::Value | UnqualifiedCompletionContext::Import => Self::All,
        }
    }
}
