//! Unqualified completion assembly for body, signature, and import-root positions.
//!
//! A name without `.` or `::` can come from several nested scopes: body locals, declaration
//! generics, the containing module, a prelude, an extern root, or builtin primitive types. This
//! module combines those families in Rust's namespace-aware shadowing order before applying
//! editor sort policy.
//!
//! Context then narrows what may remain. Type, value, const, import, and pattern positions use
//! different namespaces and insertion forms. Patterns and expressions can add variants from the
//! expected enum. Ordinary type/value positions may also attach an auto-import edit for a
//! declaration outside the visible scope.

use std::collections::HashSet;

use anyhow::Context as _;
use rg_ir_model::{DefMapRef, EnumVariantRef, Path, TypeDefRef};
use rg_ir_view::{
    display::syntax::SyntaxRenderer,
    item::{
        details::{DeclarationDetailsContext, DeclarationDetailsView},
        path::PathView,
    },
    lookup::{
        importable::ImportableNameSearch,
        name::{GenericScopeNameKind, NameLookupView, NameNamespace},
    },
    member::{ConstructorShape, MemberView},
    source::IndexedUnqualifiedNameScope,
    ty::{IndexedType, TyView, locals::BodyView},
};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionImport, CompletionInsertText,
        CompletionItem, CompletionKind, CompletionTarget,
    },
    query::completion::{
        CompletionContext,
        candidates::{
            CompletionCandidateSource, DefinitionCompletionCandidate,
            GenericScopeCompletionCandidate, LexicalCompletionCandidate,
        },
        pattern::{PatternCandidateRole, PatternCompletionPolicy},
        render::{
            CallCompletionKind, CompletionSortPolicy, CompletionSortPriority,
            DefinitionCompletionRenderer, DefinitionCompletionRequest, FunctionCompletionRenderer,
            FunctionCompletionRequest, PrimitiveTypeCompletionRenderer, constructor_insert_text,
        },
        site::{NameCompletionContext, PatternCompletionKind, UnqualifiedCompletionSite},
        syntax::ValueExpression,
    },
};

/// Combines unqualified candidate families while preserving namespace-aware shadowing.
pub(super) struct UnqualifiedCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionContext<'source>,
}

impl<'a, 'db, 'source> UnqualifiedCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionContext<'source>) -> Self {
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
            .generic_scope_candidates_for_unqualified(&site, None)
            .context("collect generic scope completions")?
        {
            rg_std::check_cancel!(self.analysis, "completion candidate");
            if hidden.contains(&(candidate.label().to_string(), candidate.namespace())) {
                continue;
            }
            hidden.insert((candidate.label().to_string(), candidate.namespace()));
            Self::push_generic_scope_completion(syntax, candidate, filter, edit, &mut completions);
        }

        let expected_variants = if let Some(policy) = pattern_policy {
            let variants =
                completion_candidates.expected_enum_variants_for_unqualified_pattern(&site)?;
            let variants = self.visible_expected_variants(
                &completion_candidates,
                variants,
                &lexical_candidates,
                &module_candidates,
                &hidden,
            )?;
            Some((variants, ExpectedVariantSite::Pattern(policy)))
        } else if let Some((expected, owner, expression)) = self
            .query
            .expected_type
            .and_then(|ranking| ranking.enum_constructor_site())
        {
            let variants = self.expected_expression_variants(
                &site,
                syntax,
                owner,
                expected,
                &module_candidates,
            )?;
            Some((variants, ExpectedVariantSite::Expression(expression)))
        } else {
            None
        };

        let auto_import_candidates = if context.supports_auto_import()
            && let Some(deadline) = self.query.auto_import_deadline(site.member_prefix())
        {
            // Only matching spellings need occupancy checks. Names from different dependencies
            // can share a label as long as neither conflicts with a name already in scope.
            completion_candidates.auto_import_candidates_for_unqualified(
                &site,
                deadline,
                |label, namespace| {
                    hidden.contains(&(label.to_string(), namespace))
                        || module_candidates.iter().any(|candidate| {
                            candidate.label() == label && candidate.namespace() == namespace
                        })
                },
            )?
        } else {
            Vec::new()
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

        if !auto_import_candidates.is_empty() {
            self.push_auto_import_completions(
                syntax,
                auto_import_candidates,
                filter,
                edit,
                &mut completions,
            )?;
        }

        if let Some((variants, variant_site)) = expected_variants {
            self.push_expected_variant_completions(
                syntax,
                variants,
                variant_site,
                edit,
                &mut completions,
            )
            .context("render expected enum variant completions")?;
        }

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
        variants: Vec<EnumVariantRef>,
        lexical: &[LexicalCompletionCandidate],
        module: &[DefinitionCompletionCandidate],
        hidden: &HashSet<(String, NameNamespace)>,
    ) -> anyhow::Result<Vec<(EnumVariantRef, String)>> {
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
                rg_std::check_cancel!(self.analysis, "completion candidate");
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
                    rg_std::check_cancel!(self.analysis, "completion candidate");
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

    /// Prefer an in-scope spelling, including a renamed import. An enum can also
    /// be reachable by path even when only its module is in scope.
    fn expected_expression_variants(
        &self,
        site: &UnqualifiedCompletionSite,
        syntax: SyntaxRenderer,
        owner: TypeDefRef,
        expected: &IndexedType,
        module: &[DefinitionCompletionCandidate],
    ) -> anyhow::Result<Vec<(EnumVariantRef, String)>> {
        let IndexedUnqualifiedNameScope::Body {
            scope,
            generic_owner,
            ..
        } = site.source().scope()
        else {
            return Ok(Vec::new());
        };
        let db = self.analysis.view_db();
        let members = MemberView::new(db);
        let variants = members.enum_variant_candidates(owner)?;
        let Some(&first) = variants.first() else {
            return Ok(Vec::new());
        };
        let Some(variant) = members.enum_variant(first)? else {
            return Ok(Vec::new());
        };
        let generic_owner = match generic_owner {
            Some(owner) => Some(*owner),
            None => BodyView::new(db).generic_owner(scope.body_ir())?,
        };
        let generics = match generic_owner {
            Some(owner) => NameLookupView::new(db).generic_scope_names(owner, None)?,
            None => Vec::new(),
        };
        let types = TyView::new(db);
        let resolves_to_enum = |path: &Path| -> anyhow::Result<bool> {
            // Ordinary path lookup sees local items. Type parameters can
            // shadow the enum too, but are resolved during type lowering.
            if let Some(name) = path.single_name()
                && generics.iter().any(|generic| {
                    generic.kind() == GenericScopeNameKind::Type && generic.label() == name
                })
            {
                return Ok(false);
            }
            types.bare_constructor_matches(*scope, path, expected)
        };

        // Value lookup did not collect lexical type names. Resolve each spelling
        // in the type namespace to account for shadowing by a local item.
        let candidates = CompletionCandidateSource::new(db);
        let mut qualifier = None;
        let enum_names = module
            .iter()
            .filter(|candidate| candidate.kind() == CompletionKind::Enum);
        // A visible type alias may be the only spelling of an enum in a private module.
        let aliases = module
            .iter()
            .filter(|candidate| candidate.kind() == CompletionKind::TypeAlias);
        for candidate in enum_names.chain(aliases) {
            rg_std::check_cancel!(self.analysis, "expected enum qualifier");
            if candidate.kind() == CompletionKind::Enum
                && candidates.type_def_for_target(candidate.target())? != Some(owner)
            {
                continue;
            }
            if resolves_to_enum(&Path::unqualified_name(candidate.label()))? {
                qualifier = Some(syntax.identifier(candidate.label()).to_string());
                break;
            }
        }
        if qualifier.is_none()
            && matches!(owner.origin, DefMapRef::Body(_))
            && resolves_to_enum(&Path::unqualified_name(variant.owner_name()))?
        {
            qualifier = Some(syntax.identifier(variant.owner_name()).to_string());
        }
        // TODO: Discover alternative qualified paths through re-exports and crate aliases.
        if qualifier.is_none()
            && let Some(canonical) = PathView::new(db, syntax.edition()).type_def_path(owner)?
        {
            let canonical = if owner.origin.as_crate_ref() == Some(self.query.crate_ref) {
                canonical
                    .split_once("::")
                    .map(|(_, tail)| format!("crate::{tail}"))
            } else {
                Some(format!("::{canonical}"))
            };
            if let Some(text) = canonical
                && let Some(path) = Path::from_text(&text, None)
                && let Some(module) = BodyView::new(db).owner_module(scope.body_ir())?
                && ImportableNameSearch::new(db).can_suggest_type_path(module, &path)?
                && resolves_to_enum(&path)?
            {
                qualifier = Some(text);
            }
        }
        Ok(qualifier.map_or_else(Vec::new, |qualifier| {
            variants
                .into_iter()
                .map(|variant| (variant, qualifier.clone()))
                .collect()
        }))
    }

    fn push_expected_variant_completions(
        &self,
        syntax: SyntaxRenderer,
        variants: Vec<(EnumVariantRef, String)>,
        site: ExpectedVariantSite,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let members = MemberView::new(self.analysis.view_db());
        for (variant_ref, qualifier) in variants {
            rg_std::check_cancel!(self.analysis, "expected enum variant completion");
            let Some(variant) = members
                .enum_variant(variant_ref)
                .context("read expected enum variant completion")?
            else {
                continue;
            };
            if !variant.is_suggestible_from(self.query.crate_ref) {
                continue;
            }
            let shape = variant.constructor_shape();
            if !site.accepts(&shape) {
                continue;
            }
            let target = CompletionTarget::EnumVariant(variant_ref);
            let label = syntax.identifier(variant.label()).to_string();
            let existing = completions.iter().position(|existing| {
                existing.label == label
                    && (existing.target == target
                        || existing.target == CompletionTarget::Declaration(variant_ref.into()))
            });
            if let Some(item) = existing.map(|index| &mut completions[index])
                && item.import.is_none()
            {
                if let ExpectedVariantSite::Expression(expression) = site {
                    if matches!(expression, ValueExpression::Path) {
                        item.insert_text = constructor_insert_text(
                            &label,
                            &shape,
                            self.query.client_capabilities.snippet_support,
                            true,
                        );
                    }
                    CompletionSortPolicy::promote_expected_type(&mut item.sort_text);
                }
                continue;
            }
            let qualifier = match site {
                ExpectedVariantSite::Pattern(_) => syntax.identifier(&qualifier).to_string(),
                ExpectedVariantSite::Expression(_) => qualifier,
            };
            let insert = format!("{qualifier}::{label}");
            let mut completion = CompletionItem {
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
                insert_text: match site {
                    ExpectedVariantSite::Pattern(policy) => {
                        policy.constructor_insert_text(&insert, &shape, false)
                    }
                    ExpectedVariantSite::Expression(ValueExpression::Path) => {
                        constructor_insert_text(
                            &insert,
                            &shape,
                            self.query.client_capabilities.snippet_support,
                            false,
                        )
                    }
                    ExpectedVariantSite::Expression(
                        ValueExpression::Call | ValueExpression::Record,
                    ) => CompletionInsertText::Text(insert),
                },
                edit: Some(edit),
                import: None,
            };
            if matches!(site, ExpectedVariantSite::Expression(_)) {
                CompletionSortPolicy::promote_expected_type(&mut completion.sort_text);
            }
            if let Some(index) = existing {
                completions[index] = completion;
            } else {
                completions.push(completion);
            }
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
            import: None,
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
            let completion =
                FunctionCompletionRenderer::new(self.analysis.view_db(), self.query, syntax)
                    .completion(FunctionCompletionRequest {
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
                    })?;
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
            import: None,
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
            rg_std::check_cancel!(self.analysis, "completion candidate");
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

    /// Show the selected path while leaving source-edit planning to completion resolve.
    fn push_auto_import_completions(
        &self,
        syntax: SyntaxRenderer,
        candidates: Vec<(DefinitionCompletionCandidate, Path)>,
        filter: UnqualifiedCompletionFilter,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create auto-import completion renderer")?;
        for (candidate, path) in candidates {
            rg_std::check_cancel!(self.analysis, "completion candidate");
            if !filter.accepts_scope_candidate(candidate.namespace(), candidate.kind()) {
                continue;
            }
            let rendered_path = syntax.path(&path);
            let path_len = path.component_count();
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
            completion.import = Some(CompletionImport {
                path,
                anonymous: false,
            });
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                continue;
            }
            completions.push(completion);
        }
        Ok(())
    }

    #[rg_std::cancelable("completion candidate", token = self.analysis)]
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

#[derive(Clone, Copy)]
enum ExpectedVariantSite {
    Pattern(PatternCompletionPolicy),
    Expression(ValueExpression),
}

impl ExpectedVariantSite {
    fn accepts(self, shape: &ConstructorShape) -> bool {
        match self {
            Self::Pattern(policy) => policy
                .candidate(CompletionKind::EnumVariant, None, Some(shape.clone()))
                .is_some(),
            Self::Expression(ValueExpression::Path) => true,
            Self::Expression(expression) => expression.matches_constructor(shape),
        }
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
