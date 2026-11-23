//! Server-side ordering for the combined completion list.

use std::hash::{BuildHasher as _, BuildHasherDefault, DefaultHasher};

use rg_ir_view::lookup::name::NameOrigin;

use crate::model::{CompletionApplicability, CompletionItem, CompletionKind};

/// Ranking hints gathered as each completion candidate is built.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRelevance {
    matches_expected_type: bool,
    matches_parameter_name: bool,
    constructor: ConstructorPriority,
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

    /// Prefer the active parameter's name among candidates that produce the expected type.
    pub(crate) fn set_parameter_name_priority(&mut self, prioritize: bool) {
        self.matches_parameter_name = prioritize;
    }

    /// Boost an associated function that constructs `Self`. A call with no arguments is easier
    /// to insert and gets a slightly larger boost.
    pub(crate) fn set_constructor_priority(&mut self, has_arguments: bool) {
        self.constructor = if has_arguments {
            ConstructorPriority::WithArguments
        } else {
            ConstructorPriority::WithoutArguments
        };
    }

    fn score(self, item: &CompletionItem) -> i16 {
        // A type match should outweigh member and import preferences. Matching the parameter
        // name only helps once the candidate is known to produce the right type.
        let mut score = 0;
        if self.matches_expected_type {
            score += 32;
            if self.matches_parameter_name {
                score += 2;
            }
        }
        score += match self.constructor {
            ConstructorPriority::None => 0,
            ConstructorPriority::WithArguments => 8,
            ConstructorPriority::WithoutArguments => 10,
        };
        if item.kind == CompletionKind::TraitMethod {
            if self.priority == CompletionSortPriority::Member {
                score -= 3;
                if item.applicability == CompletionApplicability::Maybe {
                    score -= 6;
                }
            }
            if item.import.is_some() {
                score -= 12;
            }
        }
        score
    }

    /// Compare relevance scores first. Priority groups and labels break ties.
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
                .score(right)
                .cmp(&left_relevance.score(left))
                .then_with(|| left_relevance.priority.cmp(&right_relevance.priority))
                .then_with(|| kind_before_label(left).cmp(&kind_before_label(right)))
                // Compare whole labels so a name like `alloc` sorts before `alloc_zeroed`.
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.kind.rank().cmp(&right.kind.rank()))
                .then_with(|| left.applicability.cmp(&right.applicability))
                .then_with(|| left.detail.cmp(&right.detail))
                .then_with(|| {
                    left.import
                        .as_ref()
                        .map(|import| &import.display_path)
                        .cmp(&right.import.as_ref().map(|import| &import.display_path))
                })
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ConstructorPriority {
    #[default]
    None,
    WithArguments,
    WithoutArguments,
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
