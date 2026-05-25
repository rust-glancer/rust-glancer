//! Completion assembly for source positions.
//!
//! Examples use `$0` to mark the cursor. Member completion handles shapes like
//! `user.na$0`; path completion handles body paths such as
//! `let value = crate::api::bu$0` and imports such as `use crate::api::$0`;
//! unqualified completion handles lexical positions such as `let value = inp$0`;
//! record-field completion handles `User { na$0 }`; import roots use shapes like
//! `use st$0`. The scanners identify the cursor site, while the resolver turns
//! that site into labels, detail text, documentation, sort keys, and replacement
//! edits.

mod completion_sort;
mod dot;
mod field;
mod function;
mod keyword;
mod path;
mod primitive;
mod record;
mod syntax;
mod unqualified;

use crate::{
    Analysis,
    api::view::completion::{CompletionSite, CompletionSiteSyntax, CompletionView},
    model::{CompletionItem, CompletionKind},
};
use rg_body_ir::UnqualifiedCompletionNamespace;
use rg_def_map::TargetRef;
use rg_parse::FileId;

use self::{
    dot::DotCompletionResolver, keyword::KeywordCompletionResolver, path::PathCompletionResolver,
    record::RecordFieldCompletionResolver, syntax::CompletionSyntaxContextCache,
    unqualified::UnqualifiedCompletionResolver,
};

/// Editor capabilities that affect how completion items should be rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompletionClientCapabilities {
    pub snippet_support: bool,
}

impl CompletionClientCapabilities {
    pub fn with_snippet_support(mut self, snippet_support: bool) -> Self {
        self.snippet_support = snippet_support;
        self
    }
}

/// One source-position completion query, including request-local editor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionQuery<'a> {
    pub target: TargetRef,
    pub file_id: FileId,
    pub offset: u32,
    pub source_text: Option<&'a str>,
    pub client_capabilities: CompletionClientCapabilities,
}

impl<'a> CompletionQuery<'a> {
    pub fn new(target: TargetRef, file_id: FileId, offset: u32) -> Self {
        Self {
            target,
            file_id,
            offset,
            source_text: None,
            client_capabilities: CompletionClientCapabilities::default(),
        }
    }

    pub fn with_source_text(mut self, source_text: &'a str) -> Self {
        self.source_text = Some(source_text);
        self
    }

    pub fn with_client_capabilities(
        mut self,
        client_capabilities: CompletionClientCapabilities,
    ) -> Self {
        self.client_capabilities = client_capabilities;
        self
    }
}

/// Coordinates completion-site detection with semantic candidate rendering.
///
/// For `user.na$0`, Body IR identifies the receiver expression and typed
/// prefix; the resolver looks up the receiver type and renders member
/// candidates. For `crate::api::$0` or `inp$0`, scanners provide the relevant
/// source site and replacement span; the resolver renders the matching visible
/// definitions.
pub(crate) struct CompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> CompletionResolver<'a, 'db, 'source> {
    pub(crate) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects completions for one source offset, e.g. `user.$0`,
    /// `let value = crate::$0`, `let value = inp$0`, `User { na$0 }`, or `use st$0`.
    pub(crate) fn completions_at(&self) -> anyhow::Result<Vec<CompletionItem>> {
        let mut syntax_context =
            CompletionSyntaxContextCache::new(self.query.source_text, self.query.offset);

        // Keyword fragments can be useful even when the cursor does not lower
        // into a semantic completion site. For example, `f$0` at item level is
        // just incomplete text, not a Body IR or DefMap path.
        let syntax_hint = syntax_context.get().map(|syntax| {
            CompletionSiteSyntax::new(
                syntax.inside_use_item(),
                syntax.after_dot(),
                syntax.after_colon_colon(),
            )
        });
        let Some(site) = CompletionView::new(self.analysis).site_at(
            self.query.target,
            self.query.file_id,
            self.query.offset,
            syntax_hint,
        )?
        else {
            return KeywordCompletionResolver::new(self.query.client_capabilities)
                .completions(syntax_context.get());
        };

        match site {
            CompletionSite::Dot(site) => {
                DotCompletionResolver::new(self.analysis, self.query).completions(site)
            }
            CompletionSite::BodyPath(site) => {
                PathCompletionResolver::new(self.analysis, self.query).body_completions(site)
            }
            CompletionSite::BodyUnqualified(site) => {
                // Plain body names come from lexical scope, but value positions
                // also accept expression keywords. Keep those as low-priority
                // overlay rows so semantic names remain the primary signal.
                let namespace = site.namespace;
                let mut completions = UnqualifiedCompletionResolver::new(self.analysis, self.query)
                    .body_completions(site)?;
                if matches!(namespace, UnqualifiedCompletionNamespace::Values) {
                    completions.extend(
                        KeywordCompletionResolver::new(self.query.client_capabilities)
                            .overlay_completions(syntax_context.get())?,
                    );
                    completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
                }
                Ok(completions)
            }
            CompletionSite::RecordField(site) => {
                RecordFieldCompletionResolver::new(self.analysis).completions(site)
            }
            CompletionSite::UsePath(site) => {
                PathCompletionResolver::new(self.analysis, self.query).use_completions(site)
            }
            CompletionSite::UseUnqualified(site) => {
                UnqualifiedCompletionResolver::new(self.analysis, self.query).use_completions(site)
            }
        }
    }
}

fn def_completion_detail(kind: CompletionKind, label: &str) -> String {
    match kind {
        CompletionKind::Const => format!("const {label}"),
        CompletionKind::Enum => format!("enum {label}"),
        CompletionKind::EnumVariant => format!("variant {label}"),
        CompletionKind::Field => format!("field {label}"),
        CompletionKind::Function => format!("fn {label}"),
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => format!("method {label}"),
        CompletionKind::Keyword => format!("keyword {label}"),
        CompletionKind::Macro => format!("macro {label}"),
        CompletionKind::Module => format!("mod {label}"),
        CompletionKind::PrimitiveType => format!("primitive type {label}"),
        CompletionKind::Static => format!("static {label}"),
        CompletionKind::Struct => format!("struct {label}"),
        CompletionKind::Trait => format!("trait {label}"),
        CompletionKind::TypeAlias => format!("type {label}"),
        CompletionKind::Union => format!("union {label}"),
        CompletionKind::Variable => format!("let {label}"),
    }
}
