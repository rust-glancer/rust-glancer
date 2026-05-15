//! LSP completion sort-text construction.
//!
//! Completion sorting is string-based: the final `sortText` is compared
//! lexicographically by the editor client. This module serializes contextual
//! sort components so resolver code can choose the ordering policy without
//! relying on the ordinary `Ord` of model types.

use std::fmt::Write as _;

use rg_def_map::VisibleScopeOrigin;

use crate::model::{CompletionApplicability, CompletionKind, CompletionTarget};

/// Context-sensitive policy for building LSP `sortText`.
///
/// The policy keeps resolver-specific filtering separate from the final sort
/// key shape, while still allowing contexts like type positions to prefer
/// concrete types over modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionSortPolicy {
    General,
    TypePosition,
}

/// Optional proximity bucket used before the ordinary completion sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionSortPriority {
    BodyScope { distance: usize },
    ModuleScope,
    Prelude,
    ExternRoot,
}

impl CompletionSortPriority {
    /// Returns the priority bucket for a body-local name.
    pub(super) fn body_scope(scope_distance: usize) -> Self {
        Self::BodyScope {
            distance: scope_distance,
        }
    }

    /// Returns the priority bucket for a visible module-scope name.
    pub(super) fn visible_scope(origin: VisibleScopeOrigin) -> Self {
        match origin {
            VisibleScopeOrigin::ModuleScope => Self::ModuleScope,
            VisibleScopeOrigin::Prelude => Self::Prelude,
            VisibleScopeOrigin::ExternRoot => Self::ExternRoot,
        }
    }
}

impl CompletionSortPolicy {
    /// Builds the lexicographic string consumed by LSP clients.
    pub(super) fn sort_text(
        self,
        priority: Option<CompletionSortPriority>,
        label: &str,
        kind: CompletionKind,
        applicability: CompletionApplicability,
        target: CompletionTarget,
    ) -> String {
        // Wrap raw inputs into sort components so each piece controls its own
        // lexicographic representation.
        let label = CompletionSortLabel(label);
        let kind = match self {
            Self::General => CompletionKindSort::General(kind),
            Self::TypePosition => CompletionKindSort::TypePosition(kind),
        };
        let mut sort_text = SortText::new();

        // Optional proximity comes first, so body-local and familiar names can
        // outrank otherwise alphabetically earlier global names.
        if let Some(priority) = priority {
            sort_text.append(&priority);
        }

        // The remaining components depend on the syntactic context. Type
        // positions sort by type-likelihood before label; general positions
        // keep label-first behavior.
        match self {
            Self::General => {
                sort_text.append(&label);
                sort_text.append(&kind);
                sort_text.append(&applicability);
                sort_text.append(&target);
            }
            Self::TypePosition => {
                sort_text.append(&kind);
                sort_text.append(&label);
                sort_text.append(&applicability);
                sort_text.append(&target);
            }
        }

        // Return the exact string that the LSP client compares lexicographically.
        sort_text.build()
    }
}

/// One component in the left-to-right LSP sort key.
trait SortTextComponent {
    /// Appends the component in a lexicographically sortable form.
    fn append_to(&self, out: &mut String);
}

/// Small builder that joins sort components with stable separators.
struct SortText {
    out: String,
    needs_separator: bool,
}

impl SortText {
    /// Starts an empty sort-text builder.
    fn new() -> Self {
        Self {
            out: String::new(),
            needs_separator: false,
        }
    }

    /// Appends one component after a separator when needed.
    fn append(&mut self, component: &dyn SortTextComponent) {
        if self.needs_separator {
            self.out.push('|');
        }
        component.append_to(&mut self.out);
        self.needs_separator = true;
    }

    /// Finishes the assembled sort text.
    fn build(self) -> String {
        self.out
    }
}

impl SortTextComponent for CompletionSortPriority {
    /// Serializes broad origin before narrower distance inside body scopes.
    fn append_to(&self, out: &mut String) {
        match self {
            Self::BodyScope { distance } => {
                let distance = (*distance).min(9_999);
                write!(out, "00-body:{distance:04}").expect("string writes should not fail");
            }
            Self::ModuleScope => out.push_str("01-module"),
            Self::Prelude => out.push_str("02-prelude"),
            Self::ExternRoot => out.push_str("03-extern"),
        }
    }
}

/// Completion label as a deliberate sort component.
struct CompletionSortLabel<'a>(&'a str);

impl SortTextComponent for CompletionSortLabel<'_> {
    /// Preserves normal label-first alphabetical ordering.
    fn append_to(&self, out: &mut String) {
        out.push_str(self.0);
    }
}

/// Completion-kind rank selected for the current syntactic context.
enum CompletionKindSort {
    General(CompletionKind),
    TypePosition(CompletionKind),
}

impl SortTextComponent for CompletionKindSort {
    /// Serializes the kind bucket as fixed-width digits.
    fn append_to(&self, out: &mut String) {
        let rank = match self {
            Self::General(kind) => kind.sort_text_rank(),
            Self::TypePosition(kind) => kind.type_context_sort_text_rank(),
        };
        write!(out, "{rank:02}").expect("string writes should not fail");
    }
}

impl SortTextComponent for CompletionApplicability {
    /// Places known candidates before less certain candidates.
    fn append_to(&self, out: &mut String) {
        write!(out, "{:02}", self.sort_text_rank()).expect("string writes should not fail");
    }
}

impl SortTextComponent for CompletionTarget {
    /// Adds a stable tie-breaker for otherwise identical completion rows.
    fn append_to(&self, out: &mut String) {
        write!(out, "{self:?}").expect("string writes should not fail");
    }
}
