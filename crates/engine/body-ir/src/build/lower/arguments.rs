use rg_def_map::{ExpandedBodyMacro, MacroCopyCoverage, MacroCopyRef, MacroExpansionRef};
use rg_ir_model::{BodyKey, BodySource};
use rg_syntax::{SyntaxNode, TextRange};

/// Source outputs for expansions performed within this worklist. Their IDs address the
/// producer. A child can also forward a copy from an independently prepared item expansion.
pub(super) struct ArgumentCopies {
    owner: BodyKey,
    expansions: Vec<MacroCopyCoverage>,
}

impl ArgumentCopies {
    pub(super) fn new(owner: BodyKey) -> Self {
        Self {
            owner,
            expansions: Vec::new(),
        }
    }

    pub(super) fn expand(
        &mut self,
        expansion: &ExpandedBodyMacro<SyntaxNode>,
        source: BodySource,
        parent_copy: impl Fn(TextRange) -> Option<MacroCopyRef>,
    ) -> Option<MacroExpansionRef> {
        let mut coverage = MacroCopyCoverage {
            file: source.file_id,
            copies: Vec::new(),
            transformed: expansion.transformed_inputs(source).collect(),
            forwarded: Vec::new(),
        };
        for (range, input, parent) in expansion.argument_tokens(source) {
            coverage.copies.push((range.start().into(), input));
            // A successful forwarding substitutes the child's outputs for this exact parent
            // token. Unexpanded or transformed inputs leave their own proof obligations.
            coverage.forwarded.extend(parent.and_then(&parent_copy));
        }
        if coverage.copies.is_empty() && coverage.transformed.is_empty() {
            return None;
        }
        let index = u32::try_from(self.expansions.len()).expect("body expansions fit u32");
        self.expansions.push(coverage);
        Some(MacroExpansionRef::Body {
            owner: self.owner,
            expansion: index,
        })
    }

    pub(super) fn into_expansions(self) -> Vec<MacroCopyCoverage> {
        self.expansions
    }
}
