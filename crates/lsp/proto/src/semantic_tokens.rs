//! The server advertises the same token vocabulary that the engine uses for encoding.

use gen_lsp_types::{SemanticTokenModifiers, SemanticTokenTypes, SemanticTokensLegend};

pub const SEMANTIC_TOKEN_TYPES: &[SemanticTokenTypes] = &[
    SemanticTokenTypes::Namespace,
    SemanticTokenTypes::Type,
    SemanticTokenTypes::Struct,
    SemanticTokenTypes::Enum,
    SemanticTokenTypes::Interface,
    SemanticTokenTypes::Function,
    SemanticTokenTypes::Method,
    SemanticTokenTypes::Macro,
    SemanticTokenTypes::Property,
    SemanticTokenTypes::EnumMember,
    SemanticTokenTypes::Variable,
    SemanticTokenTypes::Parameter,
    SemanticTokenTypes::TypeParameter,
    SemanticTokenTypes::Keyword,
    SemanticTokenTypes::String,
    SemanticTokenTypes::Number,
    SemanticTokenTypes::Operator,
    SemanticTokenTypes::Comment,
];

pub const SEMANTIC_TOKEN_MODIFIERS: &[SemanticTokenModifiers] = &[
    SemanticTokenModifiers::Documentation,
    SemanticTokenModifiers::Readonly,
];

pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: SEMANTIC_TOKEN_TYPES
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect(),
        token_modifiers: SEMANTIC_TOKEN_MODIFIERS
            .iter()
            .map(|modifier| modifier.as_str().to_owned())
            .collect(),
    }
}
