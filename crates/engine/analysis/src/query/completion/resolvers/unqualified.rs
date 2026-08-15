//! Unqualified completion assembly for body, signature, and import-root positions.
//!
//! A name without `.` or `::` can come from several nested scopes: body locals, declaration
//! generics, the containing module, a prelude, an extern root, or builtin primitive types. This
//! module combines those families in Rust's namespace-aware shadowing order before applying
//! editor sort policy.
//!
//! Context then narrows what may remain. Type, value, const, import, and pattern positions use
//! different namespaces and insertion forms; patterns can add expected enum variants and
//! constructor syntax. Ordinary type/value positions may also attach an auto-import edit for a
//! declaration outside the visible scope.

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
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
    query::completion::site::{
        NameCompletionContext, PatternCompletionKind, UnqualifiedCompletionSite,
    },
};

use super::super::{
    CompletionQuery,
    candidates::{
        CompletionCandidateSource, DefinitionCompletionCandidate, GenericScopeCompletionCandidate,
        LexicalCompletionCandidate,
    },
    import_edit::AutoImportEditPlanner,
    pattern::{PatternCandidateRole, PatternCompletionPolicy},
    render::{
        CallCompletionKind, CompletionSortPolicy, CompletionSortPriority,
        DefinitionCompletionRenderer, DefinitionCompletionRequest, FunctionCompletionRenderer,
        FunctionCompletionRequest, PrimitiveTypeCompletionRenderer,
    },
    syntax::CompletionSyntaxContext,
};

// TODO(#160): Expose import discovery as an explicit code action before returning it to ordinary
// completion. The synchronous module walk starts at a two-character prefix and adds hundreds of
// milliseconds to each request in a large workspace. Restore completion integration only after
// that lookup has a suitable latency boundary and has been profiled against the idle-memory goal.
const ENABLE_AUTO_IMPORT_COMPLETIONS: bool = false;

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
        syntax_context: Option<&CompletionSyntaxContext<'_>>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let context = site.context();
        let filter = UnqualifiedCompletionFilter::from(context);
        let pattern_policy = match context {
            NameCompletionContext::Pattern(kind) => Some(PatternCompletionPolicy::new(
                kind,
                self.query.client_capabilities.snippet_support,
            )),
            NameCompletionContext::Type
            | NameCompletionContext::Value
            | NameCompletionContext::Const
            | NameCompletionContext::Import => None,
        };
        let edit = CompletionEdit {
            replace: site.replace_span(),
        };
        let mut completions = Vec::new();
        let mut hidden = HashSet::new();
        let syntax = SyntaxRenderer::new(
            self.analysis
                .view_db()
                .crate_edition(self.query.crate_ref)
                .context("read unqualified completion edition")?,
        );

        let completion_candidates = CompletionCandidateSource::new(self.analysis.view_db());
        let lexical_candidates = completion_candidates
            .lexical_candidates_for_unqualified(&site)
            .context("collect lexical completion candidates")?;
        let module_candidates = completion_candidates
            .module_candidates_for_unqualified(&site)
            .context("collect module completion candidates")?;
        let auto_import_candidates = if ENABLE_AUTO_IMPORT_COMPLETIONS {
            completion_candidates
                .auto_import_candidates_for_unqualified(&site)
                .context("collect auto-import completion candidates")?
        } else {
            Vec::new()
        };
        let expected_variants = completion_candidates
            .expected_enum_variants_for_unqualified_pattern(&site)
            .context("collect expected enum variant candidates")?;

        // Body-local declarations are nearest to the cursor. Record their namespace occupancy so
        // an outer generic or module name with the same spelling cannot leak into the result.
        for candidate in lexical_candidates.iter().cloned() {
            let role = self
                .candidate_role(
                    &completion_candidates,
                    filter,
                    pattern_policy,
                    candidate.namespace(),
                    candidate.kind(),
                    candidate.target(),
                )
                .context("classify lexical completion candidate")?;
            let Some(role) = role else {
                for namespace in candidate.shadow_namespaces() {
                    hidden.insert((candidate.label().to_string(), *namespace));
                }
                continue;
            };
            self.push_lexical_completion(
                syntax,
                candidate,
                filter,
                pattern_policy,
                role,
                edit,
                &mut hidden,
                &mut completions,
            )
            .context("render lexical completion candidate")?;
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

        let expected_variants = self
            .visible_expected_variants(
                &completion_candidates,
                expected_variants,
                &lexical_candidates,
                &module_candidates,
                &hidden,
            )
            .context("filter visible expected enum variants")?;

        // Any accepted local/module spelling would make a same-namespace import conflict or add no
        // value. Keep this set fixed while rendering auto-imports so distinct global declarations
        // with the same label remain available for path-based disambiguation.
        let auto_import_occupied = if auto_import_candidates.is_empty() {
            None
        } else {
            let mut occupied = hidden.clone();
            for candidate in &module_candidates {
                if filter.accepts_scope_candidate(candidate.namespace(), candidate.kind()) {
                    occupied.insert((candidate.label().to_string(), candidate.namespace()));
                }
            }
            Some(occupied)
        };

        // Module candidates already retain whether they came from the immediate scope, prelude, or
        // extern root; rendering uses that origin to keep the familiar local-first order.
        self.push_module_completions(
            &completion_candidates,
            module_candidates,
            ModuleCompletionOptions {
                filter,
                edit,
                visible_scope_sort: match context {
                    NameCompletionContext::Type
                    | NameCompletionContext::Value
                    | NameCompletionContext::Const
                    | NameCompletionContext::Pattern(_) => VisibleScopeSort::ByOrigin,
                    NameCompletionContext::Import => VisibleScopeSort::General,
                },
                call_completion: match context {
                    NameCompletionContext::Type | NameCompletionContext::Value => {
                        CallCompletionKind::Call
                    }
                    NameCompletionContext::Const => CallCompletionKind::Plain,
                    NameCompletionContext::Pattern(PatternCompletionKind::Name) => {
                        CallCompletionKind::Call
                    }
                    NameCompletionContext::Pattern(
                        PatternCompletionKind::TupleConstructor
                        | PatternCompletionKind::RecordConstructor,
                    ) => CallCompletionKind::Plain,
                    NameCompletionContext::Import => CallCompletionKind::Plain,
                },
            },
            pattern_policy,
            &hidden,
            &mut completions,
        )
        .context("render module completion candidates")?;

        if let Some(auto_import_occupied) = auto_import_occupied {
            // Exact request syntax is shared with the coordinator. Semantic-only API callers do
            // not have that tree, so load saved source only for this source-edit policy and still
            // build at most one syntax tree on their request path.
            let loaded_source = if syntax_context.is_none() {
                self.analysis
                    .saved_source_text_for_file(self.query.crate_ref.package, self.query.file_id)
                    .context("load auto-import source text")?
            } else {
                None
            };
            let loaded_syntax = loaded_source
                .as_deref()
                .and_then(|source| CompletionSyntaxContext::at(Some(source), self.query.offset));
            if let Some(syntax_context) = syntax_context.or(loaded_syntax.as_ref()) {
                let planner = AutoImportEditPlanner::new(syntax_context, edit);
                self.push_auto_import_completions(
                    syntax,
                    auto_import_candidates,
                    filter,
                    &auto_import_occupied,
                    &planner,
                    edit,
                    &mut completions,
                )
                .context("render auto-import completion candidates")?;
            }
        }

        self.push_expected_variant_completions(
            syntax,
            expected_variants,
            pattern_policy,
            edit,
            &mut completions,
        )
        .context("render expected enum variant completions")?;

        // Primitive spellings are useful only in type positions and only when path resolution says
        // the builtin has not been shadowed in this exact body or signature scope.
        if matches!(context, NameCompletionContext::Type) {
            completions.extend(PrimitiveTypeCompletionRenderer::completions(
                completion_candidates
                    .primitive_type_candidates_for_unqualified(&site)
                    .context("collect primitive type completion candidates")?,
                edit,
            ));
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Keep only expected variants whose enum has an exact visible spelling at this cursor.
    ///
    /// This avoids inventing `Enum::Variant` when the enum is only in scope through an alias, or
    /// when a nearer lexical type shadows the module declaration with that name.
    fn visible_expected_variants(
        &self,
        source: &CompletionCandidateSource<'_, '_>,
        variants: Vec<rg_ir_model::EnumVariantRef>,
        lexical: &[LexicalCompletionCandidate],
        module: &[DefinitionCompletionCandidate],
        hidden: &HashSet<(String, NameNamespace)>,
    ) -> anyhow::Result<Vec<(rg_ir_model::EnumVariantRef, String)>> {
        let members = MemberView::new(self.analysis.view_db());
        let mut visible = Vec::new();
        for variant_ref in variants {
            let Some(variant) = members
                .enum_variant(variant_ref)
                .context("read expected enum variant")?
            else {
                continue;
            };
            let owner = variant.owner();

            let mut lexical_name = None;
            for candidate in lexical {
                if candidate.kind() == CompletionKind::Enum
                    && source
                        .type_def_for_target(candidate.target())
                        .context("resolve lexical expected enum target")?
                        == Some(owner)
                {
                    lexical_name = Some(candidate.label().to_string());
                    break;
                }
            }
            let qualifier = if let Some(label) = lexical_name {
                Some(label)
            } else {
                let mut qualifier = None;
                for candidate in module {
                    if candidate.kind() != CompletionKind::Enum
                        || hidden.contains(&(candidate.label().to_string(), candidate.namespace()))
                    {
                        continue;
                    }
                    if source
                        .type_def_for_target(candidate.target())
                        .context("resolve module expected enum target")?
                        == Some(owner)
                    {
                        qualifier = Some(candidate.label().to_string());
                        break;
                    }
                }
                qualifier
            };
            let Some(qualifier) = qualifier else {
                continue;
            };
            visible.push((variant_ref, qualifier));
        }
        Ok(visible)
    }

    fn push_expected_variant_completions(
        &self,
        syntax: SyntaxRenderer,
        variants: Vec<(rg_ir_model::EnumVariantRef, String)>,
        pattern_policy: Option<PatternCompletionPolicy>,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let members = MemberView::new(self.analysis.view_db());
        for (variant_ref, qualifier) in variants {
            let Some(variant) = members
                .enum_variant(variant_ref)
                .context("read expected enum variant completion")?
            else {
                continue;
            };
            let shape = variant.constructor_shape();
            if pattern_policy.is_some_and(|policy| {
                policy
                    .candidate(CompletionKind::EnumVariant, None, Some(shape.clone()))
                    .is_none()
            }) {
                continue;
            }
            let target = CompletionTarget::EnumVariant(variant_ref);
            let label = syntax.identifier(variant.label()).to_string();
            if completions
                .iter()
                .any(|existing| existing.target == target && existing.label == label)
            {
                continue;
            }
            let qualifier = syntax.identifier(&qualifier);
            let insert = format!("{qualifier}::{label}");
            completions.push(CompletionItem {
                label: label.clone(),
                filter_text: None,
                kind: CompletionKind::EnumVariant,
                target,
                applicability: CompletionApplicability::Known,
                detail: Some(format!("variant {insert}")),
                documentation: variant.docs_text(),
                sort_text: CompletionSortPolicy::General.sort_text(
                    Some(CompletionSortPriority::ExpectedType),
                    &label,
                    CompletionKind::EnumVariant,
                    CompletionApplicability::Known,
                    target,
                ),
                insert_text: pattern_policy.map_or_else(
                    || CompletionInsertText::Text(insert.clone()),
                    |policy| policy.constructor_insert_text(&insert, &shape, false),
                ),
                edit: Some(edit),
                additional_edits: Vec::new(),
            });
        }
        Ok(())
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
            filter_text: None,
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
            additional_edits: Vec::new(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_lexical_completion(
        &self,
        syntax: SyntaxRenderer,
        candidate: LexicalCompletionCandidate,
        filter: UnqualifiedCompletionFilter,
        pattern_policy: Option<PatternCompletionPolicy>,
        pattern_role: PatternCandidateRole,
        edit: CompletionEdit,
        hidden: &mut HashSet<(String, NameNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        for namespace in candidate.shadow_namespaces() {
            hidden.insert((candidate.label().to_string(), *namespace));
        }

        if let Some(function_ref) = candidate.function_ref() {
            let members = MemberView::new(self.analysis.view_db());
            let Some(function) = members
                .function(function_ref)
                .context("read lexical function completion")?
            else {
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
            .details_for_declaration(declaration_ref, &DeclarationDetailsContext::default())
            .context("read lexical declaration completion details")?
        else {
            return Ok(());
        };
        let detail = details.signature().map(ToString::to_string);
        let documentation = details.docs().map(ToString::to_string);
        let target = candidate.target();
        let kind = candidate.kind();
        let label = syntax.identifier(candidate.label()).to_string();
        let mut completion = CompletionItem {
            label: label.clone(),
            filter_text: None,
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
            additional_edits: Vec::new(),
        };
        if let (Some(policy), PatternCandidateRole::Constructor(shape)) =
            (pattern_policy, pattern_role)
        {
            completion.insert_text =
                policy.constructor_insert_text(&completion.label, &shape, true);
        }
        completions.push(completion);
        Ok(())
    }

    /// Render visible module names after applying grammar and lexical-shadowing policy.
    fn push_module_completions(
        &self,
        source: &CompletionCandidateSource<'_, '_>,
        candidates: Vec<DefinitionCompletionCandidate>,
        options: ModuleCompletionOptions,
        pattern_policy: Option<PatternCompletionPolicy>,
        hidden: &HashSet<(String, NameNamespace)>,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create module completion renderer")?;
        for candidate in candidates {
            if pattern_policy.is_some()
                && candidate.kind() == CompletionKind::Macro
                && !candidate.is_invocation_macro()
            {
                continue;
            }
            let role = self
                .candidate_role(
                    source,
                    options.filter,
                    pattern_policy,
                    candidate.namespace(),
                    candidate.kind(),
                    candidate.target(),
                )
                .context("classify module completion candidate")?;
            let Some(role) = role else {
                continue;
            };
            if hidden.contains(&(candidate.label().to_string(), candidate.namespace())) {
                continue;
            }

            let Some(mut completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit: options.edit,
                    call_completion: options.call_completion,
                    sort_policy: options.filter.sort_policy(),
                    sort_priority: match options.visible_scope_sort {
                        VisibleScopeSort::ByOrigin => candidate
                            .module_origin()
                            .map(CompletionSortPriority::visible_scope),
                        VisibleScopeSort::General => None,
                    },
                })
                .context("render module definition completion")?
            else {
                continue;
            };
            if let (Some(policy), PatternCandidateRole::Constructor(shape)) = (pattern_policy, role)
            {
                completion.insert_text =
                    policy.constructor_insert_text(&completion.label, &shape, true);
            }
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }
        Ok(())
    }

    /// Attach one safe source import to each otherwise-eligible out-of-scope declaration.
    #[allow(clippy::too_many_arguments)]
    fn push_auto_import_completions(
        &self,
        syntax: SyntaxRenderer,
        candidates: Vec<DefinitionCompletionCandidate>,
        filter: UnqualifiedCompletionFilter,
        occupied: &HashSet<(String, NameNamespace)>,
        planner: &AutoImportEditPlanner<'_, '_>,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create auto-import completion renderer")?;
        for candidate in candidates {
            if !filter.accepts_scope_candidate(candidate.namespace(), candidate.kind())
                || occupied.contains(&(candidate.label().to_string(), candidate.namespace()))
            {
                continue;
            }
            let Some(path) = candidate.import_path() else {
                continue;
            };
            let rendered_path = syntax.path(path).to_string();
            let Some(additional_edit) = planner.plan(path, &rendered_path) else {
                continue;
            };
            let path_len = candidate
                .import_path_len()
                .expect("auto-import candidates should retain path length");
            let Some(mut completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion: CallCompletionKind::Call,
                    sort_policy: filter.sort_policy(),
                    sort_priority: Some(CompletionSortPriority::AutoImport { path_len }),
                })
                .context("render auto-import definition completion")?
            else {
                continue;
            };
            completion.detail = Some(match completion.detail {
                Some(detail) => format!("{detail} (use {rendered_path})"),
                None => format!("use {rendered_path}"),
            });
            completion.additional_edits.push(additional_edit);
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }
        Ok(())
    }

    fn candidate_role(
        &self,
        source: &CompletionCandidateSource<'_, '_>,
        filter: UnqualifiedCompletionFilter,
        pattern_policy: Option<PatternCompletionPolicy>,
        namespace: NameNamespace,
        kind: CompletionKind,
        target: CompletionTarget,
    ) -> anyhow::Result<Option<PatternCandidateRole>> {
        let Some(policy) = pattern_policy else {
            return Ok(filter
                .accepts_scope_candidate(namespace, kind)
                .then_some(PatternCandidateRole::Plain));
        };
        Ok(policy.candidate(
            kind,
            Some(namespace),
            source
                .pattern_constructor_shape(target)
                .context("read pattern completion constructor shape")?,
        ))
    }
}

/// Namespace and declaration-kind restrictions selected from the surrounding name grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnqualifiedCompletionFilter {
    Types,
    Consts,
    Pattern(PatternCompletionKind),
    All,
}

/// Whether visible module names retain their scope origin as a sorting signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleScopeSort {
    /// Keep import-root completions in their ordinary global order.
    General,
    /// Rank module-scope names after body-local names but before prelude and extern roots.
    ByOrigin,
}

/// Rendering choices shared by one batch of visible module candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModuleCompletionOptions {
    filter: UnqualifiedCompletionFilter,
    edit: CompletionEdit,
    visible_scope_sort: VisibleScopeSort,
    call_completion: CallCompletionKind,
}

impl UnqualifiedCompletionFilter {
    fn accepts_scope_candidate(self, namespace: NameNamespace, kind: CompletionKind) -> bool {
        match self {
            Self::Types => matches!(namespace, NameNamespace::Types),
            Self::Consts => matches!(kind, CompletionKind::Module | CompletionKind::Const),
            Self::Pattern(PatternCompletionKind::Name) => matches!(
                kind,
                CompletionKind::Module
                    | CompletionKind::Struct
                    | CompletionKind::Enum
                    | CompletionKind::EnumVariant
                    | CompletionKind::TypeAlias
                    | CompletionKind::Const
                    | CompletionKind::Macro
            ),
            Self::Pattern(
                PatternCompletionKind::TupleConstructor | PatternCompletionKind::RecordConstructor,
            ) => matches!(
                kind,
                CompletionKind::Module
                    | CompletionKind::Struct
                    | CompletionKind::Enum
                    | CompletionKind::EnumVariant
            ),
            Self::All => true,
        }
    }

    fn sort_policy(self) -> CompletionSortPolicy {
        match self {
            Self::Types => CompletionSortPolicy::TypePosition,
            Self::Consts => CompletionSortPolicy::General,
            Self::Pattern(_) => CompletionSortPolicy::General,
            Self::All => CompletionSortPolicy::General,
        }
    }
}

impl From<NameCompletionContext> for UnqualifiedCompletionFilter {
    fn from(context: NameCompletionContext) -> Self {
        match context {
            NameCompletionContext::Type => Self::Types,
            NameCompletionContext::Const => Self::Consts,
            NameCompletionContext::Pattern(kind) => Self::Pattern(kind),
            NameCompletionContext::Value | NameCompletionContext::Import => Self::All,
        }
    }
}
