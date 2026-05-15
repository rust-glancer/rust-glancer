//! Completion assembly for source positions.
//!
//! Examples use `$0` to mark the cursor. Member completion handles shapes like
//! `user.na$0`; path completion handles body paths such as
//! `let value = crate::api::bu$0` and imports such as `use crate::api::$0`;
//! unqualified completion handles lexical positions such as `let value = inp$0`
//! and import roots such as `use st$0`. The scanners identify the cursor site,
//! while the resolver turns that site into labels, detail text, documentation,
//! sort keys, and replacement edits.

mod context;
mod dot;
mod path;
mod unqualified;

use rg_def_map::TargetRef;
use rg_parse::FileId;

use crate::{
    Analysis,
    model::{CompletionApplicability, CompletionItem, CompletionKind, CompletionTarget},
};

use self::{
    context::CompletionContext, dot::DotCompletionResolver, path::PathCompletionResolver,
    unqualified::UnqualifiedCompletionResolver,
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
    /// `let value = crate::$0`, `let value = inp$0`, or `use st$0`.
    pub(crate) fn completions_at(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(context) = CompletionContext::at(self.0, target, file_id, offset)? else {
            return Ok(Vec::new());
        };

        match context {
            CompletionContext::DotCompletionSite(site) => {
                DotCompletionResolver::new(self.0).completions(site)
            }
            CompletionContext::BodyPathCompletionSite(site) => {
                PathCompletionResolver::new(self.0).body_completions(site)
            }
            CompletionContext::BodyUnqualifiedCompletionSite(site) => {
                UnqualifiedCompletionResolver::new(self.0).body_completions(site)
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

/// Context-sensitive policy for building LSP `sortText`.
///
/// The policy keeps resolver-specific filtering separate from the final sort
/// key shape, while still allowing contexts like type positions to prefer
/// concrete types over modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionSortPolicy {
    General,
    TypePosition,
}

impl CompletionSortPolicy {
    fn sort_text(
        self,
        label: &str,
        kind: CompletionKind,
        applicability: CompletionApplicability,
        target: CompletionTarget,
    ) -> String {
        match self {
            Self::General => format!(
                "{label}|{:02}|{:02}|{target:?}",
                kind.sort_text_rank(),
                applicability.sort_text_rank(),
            ),
            Self::TypePosition => format!(
                "{:02}|{label}|{:02}|{target:?}",
                kind.type_context_sort_text_rank(),
                applicability.sort_text_rank(),
            ),
        }
    }
}

fn def_completion_detail(kind: CompletionKind, label: &str) -> String {
    match kind {
        CompletionKind::Const => format!("const {label}"),
        CompletionKind::Enum => format!("enum {label}"),
        CompletionKind::Field => format!("field {label}"),
        CompletionKind::Function => format!("fn {label}"),
        CompletionKind::InherentMethod | CompletionKind::TraitMethod => format!("method {label}"),
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
