//! Request-local Body IR built from the text currently shown in the editor.
//!
//! These values sit on top of a saved Body IR read transaction for one request. They let local
//! analysis see newly typed expressions and bindings without creating another project generation.

use rg_ir_model::{BodyRef, Span};

use crate::{BodyData, BodyFacts, BodyView};

use crate::BodyLocalItems;

/// One function, const, or static body rebuilt from the current editor text.
///
/// An unchanged declaration reuses its saved identity. A new or changed declaration and a newly
/// typed nested body receive request-only identities instead. All of them can still refer to saved
/// types, traits, and impls, while their expressions, locals, and body-local items come from the
/// editor. The value exists only for the request that built it and is never written back into saved
/// Body IR.
#[derive(Debug)]
pub(crate) struct CurrentBody {
    body_ref: BodyRef,
    data: BodyData,
    facts: BodyFacts,
    local_items: BodyLocalItems,
}

impl CurrentBody {
    pub(crate) fn new(
        body_ref: BodyRef,
        data: BodyData,
        facts: BodyFacts,
        local_items: BodyLocalItems,
    ) -> Self {
        Self {
            body_ref,
            data,
            facts,
            local_items,
        }
    }

    pub(crate) fn body_ref(&self) -> BodyRef {
        self.body_ref
    }

    pub(crate) fn view(&self) -> BodyView<'_> {
        BodyView::new(&self.data, &self.facts)
    }

    pub(crate) fn local_items(&self) -> &BodyLocalItems {
        &self.local_items
    }

    pub(crate) fn source_span(&self) -> Span {
        self.data.source().span
    }
}
