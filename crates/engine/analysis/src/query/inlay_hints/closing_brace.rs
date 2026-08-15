use rg_ir_model::CrateRef;
use rg_ir_view::{
    SymbolKind,
    body::{BodyClosingBraceBlock, BodyClosingBraceBlockKind, BodyStructureView},
    display::syntax::SyntaxRenderer,
};
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    Analysis,
    model::{DocumentSymbol, InlayHint, InlayHintKind, InlayHintPosition},
};

use super::InlaySource;

pub(super) fn closing_brace_hints(
    analysis: &Analysis<'_>,
    crate_ref: CrateRef,
    file_id: FileId,
    range: Option<TextSpan>,
    source: &InlaySource<'_, '_>,
) -> anyhow::Result<Vec<InlayHint>> {
    const MIN_LINE_DELTA: u32 = 20;

    let mut hints = Vec::new();
    for candidate in ClosingBraceCandidate::collect(analysis, crate_ref, file_id, source)? {
        let open_line = source.line_for_offset(candidate.file_id, candidate.open_offset())?;
        let Some(open_line) = open_line else {
            continue;
        };
        let close_line = source.line_for_offset(candidate.file_id, candidate.close_offset())?;
        let Some(close_line) = close_line else {
            continue;
        };
        if close_line.saturating_sub(open_line) < MIN_LINE_DELTA {
            continue;
        }
        if range.is_some_and(|range| !range.touches(candidate.close_span.text.end)) {
            continue;
        }

        hints.push(InlayHint {
            file_id: candidate.file_id,
            span: candidate.close_span,
            position: InlayHintPosition::After,
            kind: InlayHintKind::Text,
            label: candidate.label,
            padding_left: Some(true),
            padding_right: None,
        });
    }

    Ok(hints)
}

struct ClosingBraceCandidate {
    file_id: FileId,
    block_span: Span,
    close_span: Span,
    label: String,
}

impl ClosingBraceCandidate {
    fn collect(
        analysis: &Analysis<'_>,
        crate_ref: CrateRef,
        file_id: FileId,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<Vec<Self>> {
        let mut candidates = Vec::new();
        match source.current() {
            Some(source) => {
                let edition = analysis.view_db().crate_edition(crate_ref)?;
                let Some(syntax) = source.parse(edition).map(|parse| parse.tree()) else {
                    return Ok(candidates);
                };
                for symbol in Analysis::document_symbols_from_syntax(&syntax) {
                    Self::collect_document_symbol(file_id, &symbol, &mut candidates);
                }
            }
            None => {
                let outline = analysis.document_symbols(crate_ref, file_id)?;
                for symbol in outline.symbols {
                    Self::collect_document_symbol(outline.file_id, &symbol, &mut candidates);
                }
            }
        }
        for block in
            BodyStructureView::new(analysis.view_db()).closing_brace_blocks(crate_ref, file_id)?
        {
            let label = Self::body_block_label(analysis, crate_ref, &block, source)?;
            if let Some(candidate) = Self::from_block_span(block.file_id(), block.span(), label) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    fn collect_document_symbol(
        file_id: FileId,
        symbol: &DocumentSymbol,
        candidates: &mut Vec<Self>,
    ) {
        if let Some(label) = Self::symbol_label(&symbol.name, symbol.kind)
            && let Some(candidate) = Self::from_block_span(file_id, symbol.span, label)
        {
            candidates.push(candidate);
        }

        for child in &symbol.children {
            Self::collect_document_symbol(file_id, child, candidates);
        }
    }

    fn symbol_label(name: &str, kind: SymbolKind) -> Option<String> {
        match kind {
            SymbolKind::Module => Some(format!("// mod {name}")),
            SymbolKind::Impl => Some(format!("// {name}")),
            SymbolKind::Const
            | SymbolKind::Enum
            | SymbolKind::EnumVariant
            | SymbolKind::Field
            | SymbolKind::Function
            | SymbolKind::Macro
            | SymbolKind::Method
            | SymbolKind::Static
            | SymbolKind::Struct
            | SymbolKind::Trait
            | SymbolKind::TypeAlias
            | SymbolKind::Union
            | SymbolKind::Variable => None,
        }
    }

    fn body_block_label(
        analysis: &Analysis<'_>,
        crate_ref: CrateRef,
        block: &BodyClosingBraceBlock,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<String> {
        let label = match block.kind() {
            BodyClosingBraceBlockKind::Function { name } => {
                let syntax = SyntaxRenderer::new(analysis.view_db().crate_edition(crate_ref)?);
                format!("// fn {}", syntax.identifier(name))
            }
            BodyClosingBraceBlockKind::Match { scrutinee } => {
                Self::control_flow_label(block.file_id(), "// match", *scrutinee, source)?
            }
            BodyClosingBraceBlockKind::Loop => "// loop".to_string(),
            BodyClosingBraceBlockKind::While { condition } => {
                Self::control_flow_label(block.file_id(), "// while", *condition, source)?
            }
            BodyClosingBraceBlockKind::For { pat, iterable } => {
                Self::for_label(block.file_id(), *pat, *iterable, source)?
            }
        };
        Ok(label)
    }

    fn control_flow_label(
        file_id: FileId,
        label: &str,
        detail_span: Option<Span>,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<String> {
        let Some(detail_span) = detail_span else {
            return Ok(label.to_string());
        };
        let detail = source.text_for_span(file_id, detail_span)?;
        let Some(detail) = detail.and_then(Self::compact_source_label) else {
            return Ok(label.to_string());
        };

        Ok(format!("{label} {detail}"))
    }

    fn for_label(
        file_id: FileId,
        pat: Option<Span>,
        iterable: Option<Span>,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<String> {
        let Some(pat) = Self::source_detail(file_id, pat, source)? else {
            return Ok("// for".to_string());
        };
        let Some(iterable) = Self::source_detail(file_id, iterable, source)? else {
            return Ok(format!("// for {pat}"));
        };

        Ok(format!("// for {pat} in {iterable}"))
    }

    fn source_detail(
        file_id: FileId,
        span: Option<Span>,
        source: &InlaySource<'_, '_>,
    ) -> anyhow::Result<Option<String>> {
        let Some(span) = span else {
            return Ok(None);
        };
        let text = source.text_for_span(file_id, span)?;
        Ok(text.and_then(Self::compact_source_label))
    }

    fn compact_source_label(text: String) -> Option<String> {
        const MAX_LABEL_CHARS: usize = 40;

        let label = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!label.is_empty() && label.chars().count() <= MAX_LABEL_CHARS).then_some(label)
    }

    fn from_block_span(file_id: FileId, block_span: Span, label: String) -> Option<Self> {
        if block_span.is_empty() {
            return None;
        }

        // These hints rely on semantic and Body IR spans preserving the block-like construct
        // extent. For the supported constructs, that extent ends immediately after `}`.
        let close_start = block_span.text.end.checked_sub(1)?;
        let close_span = Span {
            text: TextSpan {
                start: close_start,
                end: block_span.text.end,
            },
        };

        Some(Self {
            file_id,
            block_span,
            close_span,
            label,
        })
    }

    fn open_offset(&self) -> u32 {
        self.block_span.text.start
    }

    fn close_offset(&self) -> u32 {
        self.close_span.text.start
    }
}
