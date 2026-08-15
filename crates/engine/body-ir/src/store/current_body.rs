//! Request-local Body IR built from the text currently shown in the editor.
//!
//! These values sit on top of a saved Body IR read transaction for one request. They let local
//! analysis see newly typed expressions and bindings without creating another project generation.

use std::collections::{HashMap, HashSet};

use rg_ir_model::{BodyRef, CrateRef};
use rg_parse::{FileId, Span};
use rg_semantic_ir::ItemLookupIndex;

use crate::{BodyData, BodyFacts, BodyView};

use super::BodyLocalItems;

/// One function, const, or static body rebuilt from the current editor text.
///
/// An unchanged declaration reuses its saved identity. A new or changed function and a newly typed
/// nested body receive request-only identities instead. All of them can still refer to saved types,
/// traits, and impls, while their expressions, locals, and body-local items come from the editor.
/// The value exists only for the request that built it and is never written back into saved Body IR.
#[derive(Debug)]
pub struct CurrentBody {
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

    pub fn source_span(&self) -> Span {
        self.data.source().span
    }
}

/// Request-local bodies layered over one saved Body IR transaction.
///
/// A rebuilt body replaces the saved body with the same identity. When editor text differs from
/// saved text, every other saved body in that file is hidden as well because its ranges may point
/// at unrelated text. Exact source does not need that whole-file mask, so unselected saved bodies
/// remain available.
///
/// Early-start indexing may also leave out the saved crate-wide item lookup index. In that case,
/// the request builds one replacement index per crate and shares it between all current bodies.
#[derive(Debug, Default)]
pub struct CurrentBodySet {
    masked_files: HashSet<(CrateRef, FileId)>,
    bodies: Vec<CurrentBody>,
    supplemental_item_lookup_indexes: HashMap<CrateRef, ItemLookupIndex>,
}

impl CurrentBodySet {
    pub fn new(
        masked_files: HashSet<(CrateRef, FileId)>,
        bodies: Vec<CurrentBody>,
        supplemental_item_lookup_indexes: HashMap<CrateRef, ItemLookupIndex>,
    ) -> anyhow::Result<Self> {
        let mut body_refs = HashSet::new();
        anyhow::ensure!(
            bodies.iter().all(|body| body_refs.insert(body.body_ref)),
            "current Body IR contains one body identity more than once",
        );

        Ok(Self {
            masked_files,
            bodies,
            supplemental_item_lookup_indexes,
        })
    }

    pub(crate) fn masks_file(&self, crate_ref: CrateRef, file: FileId) -> bool {
        self.masked_files.contains(&(crate_ref, file))
    }

    pub(crate) fn affects_crate(&self, crate_ref: CrateRef) -> bool {
        self.masked_files
            .iter()
            .any(|(masked_crate, _)| *masked_crate == crate_ref)
            || self
                .bodies
                .iter()
                .any(|body| body.body_ref.crate_ref == crate_ref)
    }

    pub(crate) fn bodies(&self) -> &[CurrentBody] {
        &self.bodies
    }

    pub(crate) fn contains_body(&self, body_ref: BodyRef) -> bool {
        self.bodies.iter().any(|body| body.body_ref == body_ref)
    }

    pub(crate) fn supplemental_item_lookup_index(
        &self,
        crate_ref: CrateRef,
    ) -> Option<&ItemLookupIndex> {
        self.supplemental_item_lookup_indexes.get(&crate_ref)
    }
}
