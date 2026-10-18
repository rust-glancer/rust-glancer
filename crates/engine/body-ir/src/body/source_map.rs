use rg_ir_model::Span;
use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Provenance of source tokens copied into a body's macro expansions.
///
/// Lowering puts original argument offsets directly in the IR's name and field spans. This map
/// distinguishes those precise locations from generated nodes anchored to the whole invocation.
/// For `duplicate!(value)` expanding to `(value, value)`, both paths keep the span of the single
/// written `value`. This map records that span once; the paths retain their separate IR identities.
/// Keeping the provenance separate means the expanded syntax trees can be dropped after lowering.
#[derive(Debug, Default, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub struct BodySourceMap {
    macro_arguments: Vec<Span>,
}

impl BodySourceMap {
    pub(crate) fn extend_macro_arguments(&mut self, spans: impl Iterator<Item = Span>) {
        self.macro_arguments.extend(spans);
    }

    pub(crate) fn finish(&mut self) {
        self.macro_arguments
            .sort_unstable_by_key(|span| (span.start, span.end));
        self.macro_arguments.dedup();
        self.macro_arguments.shrink_to_fit();
    }

    pub fn is_macro_argument(&self, span: Span) -> bool {
        self.macro_arguments
            .binary_search_by_key(&(span.start, span.end), |span| (span.start, span.end))
            .is_ok()
    }
}
