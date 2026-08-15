//! Attribute paths and grammar-specific input completion.
//!
//! ```text
//! #[der$0]                         -> built-in attributes and attribute proc macros
//! #[derive(Cl$0, Debug)]          -> built-in derives and derive proc macros, except `Debug`
//! #[cfg(feature = "ser$0")]       -> features declared by the Cargo package
//! #[tools::tra$0]                 -> attribute proc macros below `tools`
//! ```
//!
//! The syntax classifier has already selected the attribute grammar and collected entries written
//! beside the cursor. This resolver combines that request-local state with the small
//! language-owned vocabulary, package features, and macro-namespace lookup. Ordinary item
//! completion never runs inside these grammars, which prevents unrelated values and types from
//! leaking into an attribute argument list.

use anyhow::Context as _;
use rg_ir_view::lookup::name::MacroKind;

use crate::{
    Analysis,
    model::{
        CompletionEdit, CompletionInsertText, CompletionItem, CompletionKind,
        SyntheticCompletionTarget,
    },
    query::completion::site::{
        AttributeCompletionContext, AttributeCompletionKind, CompletionSourceAttachment,
    },
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, DefinitionCompletionRenderer,
        DefinitionCompletionRequest, SyntheticCompletionCandidate, SyntheticCompletionRenderer,
    },
    syntax::CompletionSyntaxContext,
};

/// Selects the candidate vocabulary owned by one recognized attribute grammar.
///
/// Depending on the grammar, rows can come from Rust's built-in vocabulary, Cargo package
/// features, or proc macros visible in the macro namespace.
pub(super) struct AttributeCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> AttributeCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Build rows for the already-classified attribute path or argument grammar.
    pub(super) fn completions(
        &self,
        context: &AttributeCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let prefix = if matches!(context.kind(), AttributeCompletionKind::CfgFeature { .. }) {
            syntax
                .string_word_prefix(true)
                .unwrap_or_else(|| syntax.prefix())
        } else {
            syntax.prefix()
        };
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut completions = SyntheticCompletionRenderer::new(prefix.text(), edit)
            .completions(self.synthetic_candidates(context.kind()));

        let (qualifier, macro_kind) = match context.kind() {
            AttributeCompletionKind::Path { qualifier } => {
                (qualifier.as_ref(), Some(MacroKind::Attribute))
            }
            AttributeCompletionKind::Derive { qualifier, .. } => {
                (qualifier.as_ref(), Some(MacroKind::Derive))
            }
            AttributeCompletionKind::Lint { .. }
            | AttributeCompletionKind::Repr { .. }
            | AttributeCompletionKind::Cfg
            | AttributeCompletionKind::CfgFeature { .. }
            | AttributeCompletionKind::Diagnostic { .. }
            | AttributeCompletionKind::Compatibility { .. } => (None, None),
        };
        let Some(macro_kind) = macro_kind else {
            return Ok(completions);
        };
        let Some(source_site) = CompletionSourceAttachment::new(
            self.analysis,
            self.query.crate_ref,
            self.query.file_id,
        )
        .module_site_at(self.query.offset, &syntax.inline_module_path())
        .context("find attribute completion module")?
        else {
            return Ok(completions);
        };
        let candidates = CompletionCandidateSource::new(self.analysis.view_db())
            .macro_candidates_at(source_site.module(), qualifier, macro_kind)
            .context("collect attribute macro candidates")?;
        let renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create attribute completion renderer")?;
        for candidate in candidates {
            if !candidate.label().starts_with(prefix.text())
                || matches!(context.kind(), AttributeCompletionKind::Derive { existing, .. }
                    if Self::contains_entry(existing, candidate.label()))
            {
                continue;
            }
            if let Some(completion) = renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion: CallCompletionKind::Plain,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: None,
                })
                .context("render attribute macro completion")?
            {
                completions.push(completion);
            }
        }
        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn synthetic_candidates(
        &self,
        kind: &AttributeCompletionKind,
    ) -> Vec<SyntheticCompletionCandidate> {
        let labels: Vec<(&str, Option<&str>)> = match kind {
            AttributeCompletionKind::Path { qualifier: None } => [
                "allow",
                "cfg",
                "cfg_attr",
                "cold",
                "deprecated",
                "derive",
                "doc",
                "inline",
                "must_use",
                "non_exhaustive",
                "path",
                "repr",
                "should_panic",
                "test",
                "track_caller",
            ]
            .into_iter()
            .map(|label| (label, None))
            .collect(),
            AttributeCompletionKind::Path { qualifier: Some(_) } => Vec::new(),
            AttributeCompletionKind::Derive {
                qualifier: None,
                existing,
            } => [
                "Clone",
                "Copy",
                "Debug",
                "Default",
                "Eq",
                "Hash",
                "Ord",
                "PartialEq",
                "PartialOrd",
            ]
            .into_iter()
            .filter(|label| !Self::contains_entry(existing, label))
            .map(|label| (label, None))
            .collect(),
            AttributeCompletionKind::Derive {
                qualifier: Some(_), ..
            } => Vec::new(),
            AttributeCompletionKind::Lint { existing } => [
                "warnings",
                "unused",
                "unused_variables",
                "dead_code",
                "unreachable_code",
                "missing_docs",
                "unsafe_code",
                "non_snake_case",
                "clippy::all",
                "clippy::pedantic",
                "rustdoc::broken_intra_doc_links",
            ]
            .into_iter()
            .filter(|label| !Self::contains_entry(existing, label))
            .map(|label| (label, None))
            .collect(),
            AttributeCompletionKind::Repr { existing } => [
                ("C", None),
                ("transparent", None),
                ("packed", Some("packed(${1:1})")),
                ("align", Some("align(${1:8})")),
                ("u8", None),
                ("u16", None),
                ("u32", None),
                ("u64", None),
                ("usize", None),
                ("i8", None),
                ("i16", None),
                ("i32", None),
                ("i64", None),
                ("isize", None),
            ]
            .into_iter()
            .filter(|(label, _)| !Self::contains_entry(existing, label))
            .collect(),
            AttributeCompletionKind::Cfg => [
                ("all", Some("all(${1:predicate})")),
                ("any", Some("any(${1:predicate})")),
                ("not", Some("not(${1:predicate})")),
                ("feature", Some("feature = \"${1:name}\"")),
                ("target_arch", Some("target_arch = \"${1:arch}\"")),
                ("target_os", Some("target_os = \"${1:os}\"")),
                ("target_family", Some("target_family = \"${1:family}\"")),
                ("target_env", Some("target_env = \"${1:env}\"")),
                ("target_vendor", Some("target_vendor = \"${1:vendor}\"")),
                ("target_endian", Some("target_endian = \"${1:endian}\"")),
                (
                    "target_pointer_width",
                    Some("target_pointer_width = \"${1:width}\""),
                ),
                ("debug_assertions", None),
                ("test", None),
                ("unix", None),
                ("windows", None),
            ]
            .into_iter()
            .collect(),
            AttributeCompletionKind::CfgFeature { existing } => self
                .analysis
                .declared_features(self.query.crate_ref.package)
                .iter()
                .filter(|feature| !existing.contains(feature))
                .map(|feature| (feature.as_str(), None))
                .collect(),
            AttributeCompletionKind::Diagnostic { existing } => [
                ("message", Some("message = \"${1:text}\"")),
                ("label", Some("label = \"${1:text}\"")),
                ("note", Some("note = \"${1:text}\"")),
            ]
            .into_iter()
            .filter(|(label, _)| !Self::contains_entry(existing, label))
            .collect(),
            AttributeCompletionKind::Compatibility {
                attribute,
                existing,
            } => {
                let values: &[(&str, Option<&str>)] = match attribute.as_str() {
                    "deprecated" => &[
                        ("since", Some("since = \"${1:version}\"")),
                        ("note", Some("note = \"${1:text}\"")),
                        ("suggestion", Some("suggestion = \"${1:text}\"")),
                    ],
                    "stable" | "rustc_const_stable" => &[
                        ("feature", Some("feature = \"${1:name}\"")),
                        ("since", Some("since = \"${1:version}\"")),
                    ],
                    "unstable" => &[
                        ("feature", Some("feature = \"${1:name}\"")),
                        ("issue", Some("issue = \"${1:number}\"")),
                        ("reason", Some("reason = \"${1:text}\"")),
                        ("soft", None),
                    ],
                    _ => &[],
                };
                values
                    .iter()
                    .copied()
                    .filter(|(label, _)| !Self::contains_entry(existing, label))
                    .collect()
            }
        };

        labels
            .into_iter()
            .map(|(label, snippet)| {
                let kind = if matches!(kind, AttributeCompletionKind::Path { .. }) {
                    CompletionKind::Attribute
                } else {
                    CompletionKind::Value
                };
                let mut candidate = SyntheticCompletionCandidate::new(
                    label,
                    kind,
                    SyntheticCompletionTarget::Attribute,
                )
                .with_detail(match kind {
                    CompletionKind::Attribute => format!("attribute {label}"),
                    _ => format!("attribute input {label}"),
                });
                if let Some(snippet) = snippet {
                    candidate = candidate.with_insert_text(
                        if self.query.client_capabilities.snippet_support {
                            CompletionInsertText::Snippet(snippet.to_string())
                        } else {
                            CompletionInsertText::Text(label.to_string())
                        },
                    );
                }
                candidate
            })
            .collect()
    }

    fn contains_entry(existing: &[String], label: &str) -> bool {
        existing.iter().any(|entry| {
            let key = entry.split_once('=').map_or(entry.as_str(), |(key, _)| key);
            key.trim() == label
                || key
                    .trim()
                    .rsplit("::")
                    .next()
                    .is_some_and(|name| name == label)
        })
    }
}
