//! Server-side ordering for the combined completion list.

use std::hash::{BuildHasher as _, BuildHasherDefault, DefaultHasher};

use rg_ir_view::lookup::name::NameOrigin;

use crate::model::CompletionItem;

/// Ranking hints gathered as each completion candidate is built.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRelevance {
    matches_expected_type: bool,
    priority: CompletionSortPriority,
    policy: CompletionSortPolicy,
}

impl CompletionRelevance {
    pub(crate) fn new(priority: CompletionSortPriority) -> Self {
        Self {
            priority,
            ..Self::default()
        }
    }

    /// Prioritize a completion when inference shows that it produces the expected type at the
    /// cursor.
    pub(crate) fn set_inference_priority(&mut self, prioritize: bool) {
        self.matches_expected_type = prioritize;
    }

    /// Put inferred type matches first, then compare priority groups. The sort policy orders
    /// candidates within each group.
    pub(crate) fn sort(items: &mut [CompletionItem]) {
        // Type positions and specialized rows group by kind before comparing labels.
        let kind_before_label = |item: &CompletionItem| match item.relevance.policy {
            CompletionSortPolicy::TypePosition => item.kind.type_context_rank(),
            CompletionSortPolicy::General
                if item.relevance.priority == CompletionSortPriority::Synthetic =>
            {
                item.kind.rank()
            }
            CompletionSortPolicy::General => 0,
        };
        let identities = BuildHasherDefault::<DefaultHasher>::default();
        items.sort_by(|left, right| {
            let left_relevance = left.relevance;
            let right_relevance = right.relevance;
            right_relevance
                .matches_expected_type
                .cmp(&left_relevance.matches_expected_type)
                .then_with(|| left_relevance.priority.cmp(&right_relevance.priority))
                .then_with(|| kind_before_label(left).cmp(&kind_before_label(right)))
                // Compare whole labels so a name like `alloc` sorts before `alloc_zeroed`.
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.kind.rank().cmp(&right.kind.rank()))
                .then_with(|| left.applicability.cmp(&right.applicability))
                .then_with(|| left.detail.cmp(&right.detail))
                // Two traits can offer the same method with the same signature. Only then
                // compare target identities so equivalent lookups give those rows a stable order.
                .then_with(|| {
                    identities
                        .hash_one(left.target)
                        .cmp(&identities.hash_one(right.target))
                })
        });
    }
}

/// Chooses how to compare labels and item kinds within a priority group.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionSortPolicy {
    /// Compare labels first for ordinary completions. Synthetic rows group by kind first.
    #[default]
    General,
    /// Prefer types and traits over module prefixes before comparing labels.
    TypePosition,
}

impl CompletionSortPolicy {
    pub(crate) fn relevance(self, priority: Option<CompletionSortPriority>) -> CompletionRelevance {
        CompletionRelevance {
            policy: self,
            ..CompletionRelevance::new(priority.unwrap_or_default())
        }
    }
}

/// Earlier groups sort first. Within a group, prefer nearer scopes and shorter import paths.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CompletionSortPriority {
    BodyScope {
        distance: usize,
    },
    /// An expected enum variant offered with a qualifier supplied by completion.
    ExpectedVariant,
    Keyword {
        rank: u8,
    },
    RequiredTraitMember,
    Synthetic,
    DefaultTraitMember,
    GenericScope,
    ItemScope,
    Primitive,
    Prelude,
    ExternRoot,
    Member,
    AutoImport {
        path_len: usize,
    },
    #[default]
    General,
    KeywordOverlay {
        rank: u8,
    },
}

impl CompletionSortPriority {
    pub(crate) fn body_scope(distance: usize) -> Self {
        Self::BodyScope { distance }
    }

    pub(crate) fn visible_scope(origin: NameOrigin) -> Self {
        match origin {
            NameOrigin::ItemScope => Self::ItemScope,
            NameOrigin::Prelude => Self::Prelude,
            NameOrigin::ExternRoot => Self::ExternRoot,
        }
    }
}
