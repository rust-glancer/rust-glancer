//! Completion resolvers for recognized strings and macro matcher fragments.
//!
//! These rows are deliberately isolated from ordinary expression completion. A string literal is
//! eligible only after the syntax classifier identifies its Rust-owned meaning, and semantic
//! format captures still come from the indexed scope surrounding that literal.
//!
//! ```text
//! format!("{na$0}")       -> named format arguments and visible value captures
//! env!("CARGO_PKG_$0")    -> Cargo-provided environment variables
//! extern "C-un$0" fn call -> supported ABI strings
//! ($value: ex$0)          -> macro matcher fragments
//! ```
//!
//! Each resolver replaces only the meaningful word or string contents; quotes, raw-string
//! markers, braces, and macro punctuation remain owned by the surrounding source.

use std::collections::HashSet;

use anyhow::Context as _;
use rg_ir_view::{
    display::syntax::SyntaxRenderer,
    source::{IndexedUnqualifiedNameContext, SourceCompletionView},
};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionItem, CompletionKind,
        SyntheticCompletionTarget,
    },
    query::completion::site::{SpecializedStringCompletionContext, UnqualifiedCompletionSite},
};

use super::super::{
    CompletionQuery,
    candidates::CompletionCandidateSource,
    render::{
        CallCompletionKind, CompletionSortPolicy, CompletionSortPriority,
        DefinitionCompletionRenderer, DefinitionCompletionRequest, SyntheticCompletionCandidate,
        SyntheticCompletionRenderer,
    },
    syntax::CompletionSyntaxContext,
};

/// Builds completion rows for syntax-owned mini-languages.
///
/// Most contexts use a small fixed vocabulary. Format captures are the exception: they add
/// semantic value names from the body surrounding the format string.
pub(super) struct SpecializedCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> SpecializedCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Dispatch one recognized string literal to its ABI, environment, or format resolver.
    pub(super) fn string_completions(
        &self,
        context: &SpecializedStringCompletionContext,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        match context {
            SpecializedStringCompletionContext::Format { named_arguments } => {
                self.format_completions(named_arguments, syntax)
            }
            SpecializedStringCompletionContext::Environment => {
                Ok(self.environment_completions(syntax))
            }
            SpecializedStringCompletionContext::Abi => Ok(self.abi_completions(syntax)),
        }
    }

    /// Complete the fragment specifier in a macro binding such as `$value:ex$0`.
    pub(super) fn macro_fragment_completions(
        &self,
        syntax: &CompletionSyntaxContext<'_>,
    ) -> Vec<CompletionItem> {
        let prefix = syntax.prefix();
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        SyntheticCompletionRenderer::new(prefix.text(), edit).completions(
            MACRO_FRAGMENTS.into_iter().map(|fragment| {
                SyntheticCompletionCandidate::new(
                    fragment,
                    CompletionKind::Value,
                    SyntheticCompletionTarget::MacroFragment,
                )
                .with_detail(format!("macro fragment {fragment}"))
            }),
        )
    }

    /// Complete only values that Rust's format capture syntax can resolve by name.
    fn format_completions(
        &self,
        named_arguments: &[String],
        syntax: &CompletionSyntaxContext<'_>,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(prefix) = syntax.string_word_prefix(false) else {
            return Ok(Vec::new());
        };
        let edit = CompletionEdit {
            replace: prefix.span(),
        };
        let mut completions = SyntheticCompletionRenderer::new(prefix.text(), edit).completions(
            named_arguments.iter().map(|argument| {
                SyntheticCompletionCandidate::new(
                    argument,
                    CompletionKind::Variable,
                    SyntheticCompletionTarget::SpecializedValue,
                )
                .with_detail(format!("format argument {argument}"))
            }),
        );
        let mut occupied = completions
            .iter()
            .map(|completion| completion.label.clone())
            .collect::<HashSet<_>>();

        let source = SourceCompletionView::new(self.analysis.view_db());
        let Some(source_site) = source
            .body_syntax_name_site_at(
                self.query.crate_ref,
                self.query.file_id,
                self.query.offset,
                IndexedUnqualifiedNameContext::Value,
                prefix.span(),
                prefix.text().to_string(),
            )
            .context("find format capture completion site")?
        else {
            return Ok(completions);
        };
        let site = UnqualifiedCompletionSite::new(source_site);
        let candidates = CompletionCandidateSource::new(self.analysis.view_db());
        let syntax_renderer = SyntaxRenderer::new(
            self.analysis
                .view_db()
                .crate_edition(self.query.crate_ref)
                .context("read format completion edition")?,
        );

        // Locals and parameters have the nearest scope. Keep their semantic targets and use label
        // occupancy to suppress an outer const/static with the same capture spelling.
        for candidate in candidates
            .lexical_candidates_for_unqualified(&site)
            .context("collect lexical format capture candidates")?
        {
            if !matches!(
                candidate.kind(),
                CompletionKind::Variable | CompletionKind::Const | CompletionKind::Static
            ) {
                continue;
            }
            let label = syntax_renderer.identifier(candidate.label()).to_string();
            if !label.starts_with(prefix.text()) || !occupied.insert(label.clone()) {
                continue;
            }
            let kind = candidate.kind();
            let target = candidate.target();
            completions.push(CompletionItem {
                label: label.clone(),
                filter_text: None,
                kind,
                target,
                applicability: CompletionApplicability::Known,
                detail: Some(Self::capture_detail(kind, &label)),
                documentation: None,
                sort_text: CompletionSortPolicy::General.sort_text(
                    Some(CompletionSortPriority::body_scope(
                        candidate.scope_distance(),
                    )),
                    &label,
                    kind,
                    CompletionApplicability::Known,
                    target,
                ),
                insert_text: crate::model::CompletionInsertText::Plain,
                edit: Some(edit),
                additional_edits: Vec::new(),
            });
        }

        // Const parameters are value-like captures but live in the declaration's generic scope.
        for candidate in candidates
            .generic_scope_candidates_for_unqualified(&site)
            .context("collect generic format capture candidates")?
        {
            if candidate.kind() != CompletionKind::Const {
                continue;
            }
            let label = syntax_renderer.identifier(candidate.label()).to_string();
            if !label.starts_with(prefix.text()) || !occupied.insert(label.clone()) {
                continue;
            }
            let target = candidate.target();
            completions.push(CompletionItem {
                label: label.clone(),
                filter_text: None,
                kind: CompletionKind::Const,
                target,
                applicability: CompletionApplicability::Known,
                detail: Some(format!("const parameter {label}")),
                documentation: None,
                sort_text: CompletionSortPolicy::General.sort_text(
                    Some(CompletionSortPriority::GenericScope),
                    &label,
                    CompletionKind::Const,
                    CompletionApplicability::Known,
                    target,
                ),
                insert_text: crate::model::CompletionInsertText::Plain,
                edit: Some(edit),
                additional_edits: Vec::new(),
            });
        }

        // Module lookup supplies visible consts and statics only. Functions, macros, and imports
        // are intentionally absent because format capture syntax accepts a bare value name.
        let definition_renderer = DefinitionCompletionRenderer::new(self.analysis, self.query)
            .context("create format capture renderer")?;
        for candidate in candidates
            .module_candidates_for_unqualified(&site)
            .context("collect module format capture candidates")?
        {
            if !matches!(
                candidate.kind(),
                CompletionKind::Const | CompletionKind::Static
            ) || !candidate.label().starts_with(prefix.text())
                || !occupied.insert(candidate.label().to_string())
            {
                continue;
            }
            let priority = candidate
                .module_origin()
                .map(CompletionSortPriority::visible_scope);
            if let Some(completion) = definition_renderer
                .completion(DefinitionCompletionRequest {
                    candidate: &candidate,
                    edit,
                    call_completion: CallCompletionKind::Plain,
                    sort_policy: CompletionSortPolicy::General,
                    sort_priority: priority,
                })
                .context("render module format capture completion")?
            {
                completions.push(completion);
            }
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    fn environment_completions(&self, syntax: &CompletionSyntaxContext<'_>) -> Vec<CompletionItem> {
        let Some((prefix, edit)) = Self::whole_string_request(syntax, false) else {
            return Vec::new();
        };
        SyntheticCompletionRenderer::new(prefix, edit).completions(
            CARGO_ENVIRONMENT_VARIABLES
                .into_iter()
                .map(|(name, detail)| {
                    SyntheticCompletionCandidate::new(
                        name,
                        CompletionKind::Value,
                        SyntheticCompletionTarget::SpecializedValue,
                    )
                    .with_detail(detail)
                }),
        )
    }

    fn abi_completions(&self, syntax: &CompletionSyntaxContext<'_>) -> Vec<CompletionItem> {
        let Some((prefix, edit)) = Self::whole_string_request(syntax, true) else {
            return Vec::new();
        };

        // TODO: Filter feature-gated ABIs once crate language-feature state is indexed. Retaining
        // the compiler-recognized vocabulary is still useful for compiler and nightly workspaces.
        SyntheticCompletionRenderer::new(prefix, edit).completions(SUPPORTED_ABIS.into_iter().map(
            |abi| {
                SyntheticCompletionCandidate::new(
                    abi,
                    CompletionKind::Value,
                    SyntheticCompletionTarget::SpecializedValue,
                )
                .with_detail(format!("extern ABI {abi}"))
            },
        ))
    }

    /// Whole-string resolvers replace the content, never the surrounding quotes or raw markers.
    fn whole_string_request<'syntax>(
        syntax: &'syntax CompletionSyntaxContext<'_>,
        allows_hyphen: bool,
    ) -> Option<(&'syntax str, CompletionEdit)> {
        let content = syntax.string_content_span()?;
        let typed = rg_parse::Span {
            text: rg_parse::TextSpan {
                start: content.text.start,
                end: syntax.prefix().span().text.end,
            },
        };
        let prefix = syntax.source_text(typed)?;
        if !prefix
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric() || (allows_hyphen && ch == '-'))
        {
            return None;
        }
        Some((prefix, CompletionEdit { replace: content }))
    }

    fn capture_detail(kind: CompletionKind, label: &str) -> String {
        match kind {
            CompletionKind::Variable => format!("format capture {label}"),
            CompletionKind::Const => format!("const capture {label}"),
            CompletionKind::Static => format!("static capture {label}"),
            _ => format!("format capture {label}"),
        }
    }
}

const MACRO_FRAGMENTS: [&str; 15] = [
    "ident",
    "block",
    "stmt",
    "expr",
    "pat",
    "ty",
    "lifetime",
    "literal",
    "path",
    "meta",
    "tt",
    "item",
    "vis",
    "expr_2021",
    "pat_param",
];

const CARGO_ENVIRONMENT_VARIABLES: [(&str, &str); 21] = [
    ("CARGO", "path to the Cargo executable"),
    (
        "CARGO_MANIFEST_DIR",
        "directory containing this package manifest",
    ),
    ("CARGO_MANIFEST_PATH", "path to this package manifest"),
    ("CARGO_PKG_VERSION", "complete package version"),
    ("CARGO_PKG_VERSION_MAJOR", "package major version"),
    ("CARGO_PKG_VERSION_MINOR", "package minor version"),
    ("CARGO_PKG_VERSION_PATCH", "package patch version"),
    ("CARGO_PKG_VERSION_PRE", "package pre-release version"),
    ("CARGO_PKG_AUTHORS", "package authors"),
    ("CARGO_PKG_NAME", "package name"),
    ("CARGO_PKG_DESCRIPTION", "package description"),
    ("CARGO_PKG_HOMEPAGE", "package homepage"),
    ("CARGO_PKG_REPOSITORY", "package repository"),
    ("CARGO_PKG_LICENSE", "package license expression"),
    ("CARGO_PKG_LICENSE_FILE", "package license-file path"),
    (
        "CARGO_PKG_RUST_VERSION",
        "package minimum supported Rust version",
    ),
    ("CARGO_CRATE_NAME", "crate name being compiled"),
    ("CARGO_BIN_NAME", "binary target name being compiled"),
    (
        "CARGO_PRIMARY_PACKAGE",
        "marker for a selected primary package",
    ),
    (
        "CARGO_TARGET_TMPDIR",
        "temporary directory for integration tests and benches",
    ),
    ("OUT_DIR", "build-script output directory"),
];

const SUPPORTED_ABIS: [&str; 29] = [
    "Rust",
    "C",
    "C-unwind",
    "cdecl",
    "stdcall",
    "stdcall-unwind",
    "fastcall",
    "vectorcall",
    "thiscall",
    "thiscall-unwind",
    "aapcs",
    "win64",
    "sysv64",
    "ptx-kernel",
    "msp430-interrupt",
    "x86-interrupt",
    "efiapi",
    "avr-interrupt",
    "avr-non-blocking-interrupt",
    "riscv-interrupt-m",
    "riscv-interrupt-s",
    "C-cmse-nonsecure-call",
    "C-cmse-nonsecure-entry",
    "wasm",
    "system",
    "system-unwind",
    "rust-intrinsic",
    "rust-call",
    "unadjusted",
];
