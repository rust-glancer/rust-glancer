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
mod context;
mod dot;
mod field;
mod function;
mod keyword;
mod path;
mod record;
mod syntax;
mod unqualified;

use rg_body_ir::UnqualifiedCompletionNamespace;
use rg_def_map::TargetRef;
use rg_parse::FileId;

use crate::{
    Analysis,
    model::{CompletionItem, CompletionKind},
};

use self::{
    context::CompletionContext, dot::DotCompletionResolver, keyword::KeywordCompletionResolver,
    path::PathCompletionResolver, record::RecordFieldCompletionResolver,
    syntax::CompletionSyntaxContextCache, unqualified::UnqualifiedCompletionResolver,
};

/// Coordinates completion-site detection with semantic candidate rendering.
///
/// For `user.na$0`, Body IR identifies the receiver expression and typed
/// prefix; the resolver looks up the receiver type and renders member
/// candidates. For `crate::api::$0` or `inp$0`, scanners provide the relevant
/// source site and replacement span; the resolver renders the matching visible
/// definitions.
pub(crate) struct CompletionResolver<'a, 'db>(&'a Analysis<'db>);

impl<'a, 'db> CompletionResolver<'a, 'db> {
    pub(crate) fn new(analysis: &'a Analysis<'db>) -> Self {
        Self(analysis)
    }

    /// Collects completions for one source offset, e.g. `user.$0`,
    /// `let value = crate::$0`, `let value = inp$0`, `User { na$0 }`, or `use st$0`.
    pub(crate) fn completions_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let mut syntax_context = CompletionSyntaxContextCache::new(self.0, offset);

        // Keyword fragments can be useful even when the cursor does not lower
        // into a semantic completion site. For example, `f$0` at item level is
        // just incomplete text, not a Body IR or DefMap path.
        let Some(context) =
            CompletionContext::at(self.0, target, file_id, offset, syntax_context.get())?
        else {
            return KeywordCompletionResolver::new().completions(syntax_context.get());
        };

        match context {
            CompletionContext::DotCompletionSite(site) => {
                DotCompletionResolver::new(self.0).completions(site)
            }
            CompletionContext::BodyPathCompletionSite(site) => {
                PathCompletionResolver::new(self.0).body_completions(site)
            }
            CompletionContext::BodyUnqualifiedCompletionSite(site) => {
                // Plain body names come from lexical scope, but value positions
                // also accept expression keywords. Keep those as low-priority
                // overlay rows so semantic names remain the primary signal.
                let mut completions =
                    UnqualifiedCompletionResolver::new(self.0).body_completions(site)?;
                if matches!(site.namespace, UnqualifiedCompletionNamespace::Values) {
                    completions.extend(
                        KeywordCompletionResolver::new()
                            .overlay_completions(syntax_context.get())?,
                    );
                    completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
                }
                Ok(completions)
            }
            CompletionContext::RecordFieldCompletionSite(site) => {
                RecordFieldCompletionResolver::new(self.0).completions(site)
            }
            CompletionContext::UsePathCompletionSite(site) => {
                PathCompletionResolver::new(self.0).use_completions(site)
            }
            CompletionContext::UseUnqualifiedCompletionSite(site) => {
                UnqualifiedCompletionResolver::new(self.0).use_completions(site)
            }
        }
    }
}

struct CompletionMetadata {
    label: String,
    detail: Option<String>,
    documentation: Option<String>,
}

fn def_completion_detail(kind: CompletionKind, label: &str) -> String {
    match kind {
        CompletionKind::Const => format!("const {label}"),
        CompletionKind::Enum => format!("enum {label}"),
        CompletionKind::Field => format!("field {label}"),
        CompletionKind::Function => format!("fn {label}"),
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => format!("method {label}"),
        CompletionKind::Keyword => format!("keyword {label}"),
        CompletionKind::Macro => format!("macro {label}"),
        CompletionKind::Module => format!("mod {label}"),
        CompletionKind::Static => format!("static {label}"),
        CompletionKind::Struct => format!("struct {label}"),
        CompletionKind::Trait => format!("trait {label}"),
        CompletionKind::TypeAlias => format!("type {label}"),
        CompletionKind::Union => format!("union {label}"),
        CompletionKind::Variable => format!("let {label}"),
    }
}
