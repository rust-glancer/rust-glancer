//! Replay the syntax of an item expansion from the packed tokens used to lower it.
//!
//! An item macro saves its packed output so body analysis and rename can reopen it on demand.
//! A nested invocation can receive two copies of the same written argument. Keep their distinct
//! parent copy IDs in the saved tokens and keep the table that maps them to the written input.

use std::sync::Arc;

use rg_ir_model::{FileId, PackageSlot};
use rg_item_tree::{ExpandedItemSyntax, file_declaration_sources};
use rg_macro_runtime::{ExpansionParseKind, ExpansionSyntax};
use rg_source::QueryContext;
use rg_std::{MemorySize, Shrink};
use rg_syntax::SyntaxNode;
use rg_tt::{Span as TtSpan, syntax_bridge::ExpansionSpanMap};
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    MacroCopyCoverage,
    macro_copies::{ForwardedMacroInputs, expansion_argument_origin},
};

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, MemorySize, Shrink)]
pub(crate) struct GeneratedItemExpansion {
    pub(crate) syntax: ExpandedItemSyntax,
    /// A generated invocation keeps its parent even when its forwarding table is empty.
    /// `None` identifies a root invocation for rename coverage.
    pub(crate) forwarding: Option<ForwardedMacroInputs>,
    #[memsize(skip)]
    #[shrink(skip)]
    pub(crate) transformed_inputs: Vec<TtSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub(super) enum SavedItemExpansion {
    File(ajika::Output<ExpandedItemSyntax>),
    Macro(ajika::Output<GeneratedItemExpansion>),
}

impl SavedItemExpansion {
    pub(super) fn load<'db>(
        &self,
        db: &'db QueryContext<'db>,
    ) -> ajika::Result<(&'db ExpandedItemSyntax, Option<&'db ForwardedMacroInputs>)> {
        match self {
            Self::File(output) => Ok((output.value_ref(db)?, None)),
            Self::Macro(output) => {
                let data = output.value_ref(db)?;
                Ok((&data.syntax, data.forwarding.as_ref()))
            }
        }
    }
}

pub(super) struct ParsedItemExpansion {
    pub(super) root: SyntaxNode,
    pub(super) spans: Arc<ExpansionSpanMap>,
}

/// Use the same item parse as declaration lowering. Body ranges and copy IDs both depend on
/// reproducing the original byte offsets, including the spaces inserted between output tokens.
#[ajika::query(transient, returns(ref))]
pub(super) fn parsed_item_expansion(
    db: &QueryContext<'_>,
    expansion: SavedItemExpansion,
) -> ajika::Result<ParsedItemExpansion> {
    let (syntax, _) = expansion.load(db)?;
    let (parse, spans) =
        ExpansionSyntax::parse_token_tree(&syntax.tokens, ExpansionParseKind::Items);
    Ok(ParsedItemExpansion {
        root: parse.syntax_node(),
        spans: Arc::new(spans),
    })
}

/// The route index uses the call site to select coverage without opening the packed tokens.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct ItemMacroCopySource {
    pub call_site: TtSpan,
    pub is_root: bool,
    pub(super) expansion: ajika::Output<GeneratedItemExpansion>,
}

impl ItemMacroCopySource {
    pub fn coverage<'db>(
        &self,
        db: &'db QueryContext<'db>,
    ) -> ajika::Result<&'db MacroCopyCoverage> {
        item_macro_copy_coverage(db, self.expansion.clone())
    }
}

/// Rebuild coverage from the whole output. A copied token still matters to rename when lowering
/// produced no semantic occurrence for it. Resolve its origin in the active snapshot so a
/// moved invocation uses its new written range.
#[ajika::query(transient, returns(ref))]
fn item_macro_copy_coverage(
    db: &QueryContext<'_>,
    expansion: ajika::Output<GeneratedItemExpansion>,
) -> ajika::Result<MacroCopyCoverage> {
    let data = expansion.value_ref(db)?;
    let call = data.syntax.call_site;
    let forwarding = data.forwarding.as_ref();
    let parsed = parsed_item_expansion(db, SavedItemExpansion::Macro(expansion))?;
    let file = FileId::from_raw(call.anchor.file_id.raw_file_id());
    let inputs = file_declaration_sources(db, PackageSlot(call.anchor.package as usize), file)?;
    let mut copies = Vec::new();
    let mut forwarded = Vec::new();
    for (range, token) in parsed.spans.token_spans(parsed.root.text_range()) {
        let Some(origin) = expansion_argument_origin(call, token, forwarding) else {
            continue;
        };
        // The raw token identifies the parent copy. The restored origin finds the written range.
        forwarded.extend(forwarding.and_then(|inputs| inputs.copy(token)));
        if let Some(span) = inputs.token_span(origin) {
            copies.push((range.start().into(), span));
        }
    }
    let transformed = data
        .transformed_inputs
        .iter()
        .filter_map(|&token| expansion_argument_origin(call, token, forwarding))
        .filter_map(|origin| inputs.token_span(origin))
        .collect();
    Ok(MacroCopyCoverage {
        file,
        copies,
        transformed,
        forwarded,
    })
}
