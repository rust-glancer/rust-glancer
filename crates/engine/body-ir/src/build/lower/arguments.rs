use std::collections::HashMap;

use rg_def_map::ExpandedBodyMacro;
use rg_ir_model::{BodySource, FileId, Span};
use rg_syntax::{SyntaxNode, TextRange};

pub(super) type ExpansionCopies = HashMap<TextRange, u32>;

/// Counts the uses of each written macro argument across one body worklist.
///
/// If `twice!(x)` expands to `(x, x)`, both output tokens point to the same written `x`.
/// Give the two copies separate IDs before lowering so a copy still counts if no source
/// occurrence comes out of it.
#[derive(Default)]
pub(super) struct ArgumentCopies {
    copies: Vec<Option<(FileId, Span)>>,
}

impl ArgumentCopies {
    pub(super) fn expand(
        &mut self,
        expansion: &ExpandedBodyMacro<SyntaxNode>,
        source: BodySource,
        parent: Option<&ExpansionCopies>,
    ) -> ExpansionCopies {
        let copies = expansion
            .argument_tokens(source)
            .map(|(range, span, parent_range)| {
                // If `outer!(x)` emits `inner!(x)` and inner forwards x, count the inner
                // output in place of the parent token. Keep the parent token if we cannot
                // follow it further, for example when inner cannot be expanded.
                if let Some(parent_range) = parent_range
                    && let Some(&parent_id) = parent.and_then(|parent| parent.get(&parent_range))
                {
                    self.copies[parent_id as usize] = None;
                }
                let id = u32::try_from(self.copies.len()).expect("macro copies fit u32");
                self.copies.push(Some((source.file_id, span)));
                (range, id)
            })
            .collect();

        // An input to `${concat(prefix_, $name)}` affects the output without supplying an
        // editable token. Count that use without giving any lowered occurrence its ID.
        self.copies.extend(
            expansion
                .transformed_inputs(source)
                .map(|span| Some((source.file_id, span))),
        );
        copies
    }

    pub(super) fn totals(self) -> impl Fn(u32) -> Option<u32> {
        let mut totals = HashMap::new();
        for &span in self.copies.iter().flatten() {
            *totals.entry(span).or_insert(0) += 1;
        }
        move |id| self.copies[id as usize].map(|span| totals[&span])
    }
}
