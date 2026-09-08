//! Convert Rust file byte ranges into semantic tokens an editor can apply.
//! The output uses UTF-16 columns and positions relative to the preceding token, so source spans
//! must be ordered and split into lines before their positions can be encoded.

use ls_types::{Position, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens};
use rg_analysis::{Highlight, HighlightKind, SymbolKind};
use rg_lsp_proto::{SEMANTIC_TOKEN_MODIFIERS, SEMANTIC_TOKEN_TYPES};
use rg_parse::LineIndex;

use super::position;

/// Encode highlights using the same source text and line index that produced their spans.
/// The source text supplies token lengths and line breaks; the index locates token starts.
pub(crate) struct SemanticTokensEncoder<'a> {
    source: &'a str,
    line_index: &'a LineIndex,
    tokens: Vec<SemanticToken>,
    /// Start of the last emitted token, in LSP lines and UTF-16 columns, for relative encoding.
    previous: Position,
    /// Exclusive byte end of the last accepted source highlight, for rejecting overlaps.
    end: usize,
}

impl<'a> SemanticTokensEncoder<'a> {
    pub(crate) fn new(source: &'a str, line_index: &'a LineIndex) -> Self {
        Self {
            source,
            line_index,
            tokens: Vec::new(),
            previous: Position::default(),
            end: 0,
        }
    }

    /// Choose nonoverlapping highlights, then encode their positions and colors for the editor.
    ///
    /// Several crate interpretations can color the same source. Earlier starts take precedence,
    /// then shorter ranges at the same start; identical ranges retain their input order. Once
    /// a highlight is accepted, later highlights overlapping it are skipped in full.
    pub(crate) fn encode(mut self, mut highlights: Vec<Highlight>) -> SemanticTokens {
        highlights.sort_by_key(|highlight| (highlight.span.text.start, highlight.span.text.end));
        for highlight in highlights {
            let range = highlight.span.text.start as usize..highlight.span.text.end as usize;
            if range.start < self.end {
                continue;
            }
            let Some(text) = self.source.get(range.clone()) else {
                continue;
            };
            let (token_type, token_modifiers_bitset) = Self::classification(highlight);
            let mut start = range.start;
            for line in text.split_inclusive('\n') {
                // Emit one token per source line so clients do not need multiline-token support.
                // Exclude line endings, including both bytes of a CRLF, from the token text.
                let content = line.trim_end_matches(['\r', '\n']);
                if !content.is_empty() {
                    let current = position::position(self.line_index, start as u32);
                    // Same-line columns are relative to the previous token's start. After a
                    // line change, the column is measured from the beginning of the new line.
                    self.tokens.push(SemanticToken {
                        delta_line: current.line - self.previous.line,
                        delta_start: if current.line == self.previous.line {
                            current.character - self.previous.character
                        } else {
                            current.character
                        },
                        length: content.encode_utf16().count() as u32,
                        token_type,
                        token_modifiers_bitset,
                    });
                    self.previous = current;
                }
                start += line.len();
            }
            self.end = range.end;
        }
        // No result id: token results and their source maps live only for this request.
        SemanticTokens {
            result_id: None,
            data: self.tokens,
        }
    }

    /// Editors receive numbers rather than Rust symbol kinds. Use the advertised legend's order
    /// so both sides agree on the meaning of each type index and modifier bit.
    fn classification(highlight: Highlight) -> (u32, u32) {
        let token_type = match highlight.kind {
            HighlightKind::Symbol(kind) => match kind {
                SymbolKind::Module => SemanticTokenType::NAMESPACE,
                SymbolKind::Struct | SymbolKind::Union => SemanticTokenType::STRUCT,
                SymbolKind::Enum => SemanticTokenType::ENUM,
                SymbolKind::Trait => SemanticTokenType::INTERFACE,
                SymbolKind::TypeAlias | SymbolKind::Impl => SemanticTokenType::TYPE,
                SymbolKind::Function => SemanticTokenType::FUNCTION,
                SymbolKind::Method => SemanticTokenType::METHOD,
                SymbolKind::Macro => SemanticTokenType::MACRO,
                SymbolKind::Field => SemanticTokenType::PROPERTY,
                SymbolKind::EnumVariant => SemanticTokenType::ENUM_MEMBER,
                SymbolKind::Variable | SymbolKind::Const | SymbolKind::Static => {
                    SemanticTokenType::VARIABLE
                }
            },
            HighlightKind::Keyword => SemanticTokenType::KEYWORD,
            HighlightKind::String => SemanticTokenType::STRING,
            HighlightKind::Number => SemanticTokenType::NUMBER,
            HighlightKind::Operator => SemanticTokenType::OPERATOR,
            HighlightKind::Comment => SemanticTokenType::COMMENT,
            HighlightKind::Parameter => SemanticTokenType::PARAMETER,
            HighlightKind::TypeParameter => SemanticTokenType::TYPE_PARAMETER,
        };
        let token_type = SEMANTIC_TOKEN_TYPES
            .iter()
            .position(|candidate| *candidate == token_type)
            .expect("highlight type is in the shared legend") as u32;
        let mut modifiers = 0;
        for (index, modifier) in SEMANTIC_TOKEN_MODIFIERS.iter().enumerate() {
            if (*modifier == SemanticTokenModifier::DOCUMENTATION && highlight.documentation)
                || (*modifier == SemanticTokenModifier::READONLY
                    && matches!(highlight.kind, HighlightKind::Symbol(SymbolKind::Const)))
            {
                modifiers |= 1 << index;
            }
        }
        (token_type, modifiers)
    }
}
