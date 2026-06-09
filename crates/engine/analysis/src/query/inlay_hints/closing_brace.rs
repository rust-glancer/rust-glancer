use rg_ir_model::TargetRef;
use rg_ir_view::{
    SymbolKind,
    body::{BodyClosingBraceBlock, BodyClosingBraceBlockKind, BodyStructureView},
};
use rg_parse::{FileId, Span, TextSpan};

use crate::{
    Analysis,
    model::{DocumentSymbol, InlayHint, InlayHintKind, InlayHintPosition},
};

pub(super) fn closing_brace_hints(
    analysis: &Analysis<'_>,
    target: TargetRef,
    file_id: FileId,
    range: Option<TextSpan>,
) -> anyhow::Result<Vec<InlayHint>> {
    const MIN_LINE_DELTA: u32 = 20;

    let mut hints = Vec::new();
    for candidate in ClosingBraceCandidate::collect(analysis, target, file_id)? {
        let Some(open_line) = analysis.source_line_for_offset(
            target.package,
            candidate.file_id,
            candidate.open_offset(),
        ) else {
            continue;
        };
        let Some(close_line) = analysis.source_line_for_offset(
            target.package,
            candidate.file_id,
            candidate.close_offset(),
        ) else {
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
        target: TargetRef,
        file_id: FileId,
    ) -> anyhow::Result<Vec<Self>> {
        let mut candidates = Vec::new();
        for symbol in analysis.document_symbols(target, file_id)? {
            Self::collect_document_symbol(&symbol, &mut candidates);
        }
        for block in
            BodyStructureView::new(analysis.view_db()).closing_brace_blocks(target, file_id)?
        {
            let label = Self::body_block_label(analysis, target, &block);
            if let Some(candidate) = Self::from_block_span(block.file_id(), block.span(), label) {
                candidates.push(candidate);
            }
        }

        Ok(candidates)
    }

    fn collect_document_symbol(symbol: &DocumentSymbol, candidates: &mut Vec<Self>) {
        if let Some(label) = Self::symbol_label(symbol)
            && let Some(candidate) =
                Self::from_block_span(symbol.file_id, symbol.span, label.to_string())
        {
            candidates.push(candidate);
        }

        for child in &symbol.children {
            Self::collect_document_symbol(child, candidates);
        }
    }

    fn symbol_label(symbol: &DocumentSymbol) -> Option<String> {
        match symbol.kind {
            SymbolKind::Module => Some(format!("// mod {}", symbol.name)),
            SymbolKind::Impl => Some(format!("// {}", symbol.name)),
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
        target: TargetRef,
        block: &BodyClosingBraceBlock,
    ) -> String {
        match block.kind() {
            BodyClosingBraceBlockKind::Function { name } => format!("// fn {name}"),
            BodyClosingBraceBlockKind::Match { scrutinee } => {
                Self::control_flow_label(analysis, target, block.file_id(), "// match", *scrutinee)
            }
            BodyClosingBraceBlockKind::Loop => "// loop".to_string(),
            BodyClosingBraceBlockKind::While { condition } => {
                Self::control_flow_label(analysis, target, block.file_id(), "// while", *condition)
            }
            BodyClosingBraceBlockKind::For { pat, iterable } => {
                Self::for_label(analysis, target, block.file_id(), *pat, *iterable)
            }
        }
    }

    fn control_flow_label(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        label: &str,
        detail_span: Option<Span>,
    ) -> String {
        let Some(detail_span) = detail_span else {
            return label.to_string();
        };
        let Some(detail) = analysis
            .source_text_for_span(target.package, file_id, detail_span)
            .and_then(Self::compact_source_label)
        else {
            return label.to_string();
        };

        format!("{label} {detail}")
    }

    fn for_label(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        pat: Option<Span>,
        iterable: Option<Span>,
    ) -> String {
        let Some(pat) = pat.and_then(|span| Self::source_detail(analysis, target, file_id, span))
        else {
            return "// for".to_string();
        };
        let Some(iterable) =
            iterable.and_then(|span| Self::source_detail(analysis, target, file_id, span))
        else {
            return format!("// for {pat}");
        };

        format!("// for {pat} in {iterable}")
    }

    fn source_detail(
        analysis: &Analysis<'_>,
        target: TargetRef,
        file_id: FileId,
        span: Span,
    ) -> Option<String> {
        analysis
            .source_text_for_span(target.package, file_id, span)
            .and_then(Self::compact_source_label)
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
