//! Adapted from rust-analyzer's `span` crate.
//!
//! Coarse span and syntax-context types for macro expansion.
//!
//! These types intentionally model only the subset of rust-analyzer's `span`
//! module that `tt` and `mbe` need for the first text-based expansion path.

use std::fmt;

pub use parser::Edition;
pub use text_size::{TextRange, TextSize};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub range: TextRange,
    pub anchor: SpanAnchor,
    pub ctx: SyntaxContext,
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Span")
            .field("range", &self.range)
            .field("anchor", &self.anchor)
            .field("ctx", &self.ctx)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanAnchor {
    pub file_id: EditionedFileId,
    pub ast_id: ErasedFileAstId,
}

impl fmt::Debug for SpanAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpanAnchor")
            .field(&self.file_id)
            .field(&self.ast_id)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditionedFileId(u32);

impl EditionedFileId {
    const EDITION_BITS_OFFSET: u32 = 24;
    const FILE_ID_MASK: u32 = 0x00ff_ffff;

    pub const fn new(raw_file_id: u32, edition: Edition) -> Self {
        Self((raw_file_id & Self::FILE_ID_MASK) | ((edition as u32) << Self::EDITION_BITS_OFFSET))
    }

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn edition(self) -> Edition {
        let edition = (self.0 >> Self::EDITION_BITS_OFFSET) as u8;
        match edition {
            0 => Edition::Edition2015,
            1 => Edition::Edition2018,
            2 => Edition::Edition2021,
            3 => Edition::Edition2024,
            _ => Edition::CURRENT,
        }
    }
}

impl fmt::Debug for EditionedFileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EditionedFileId")
            .field(&self.as_u32())
            .field(&self.edition())
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErasedFileAstId(pub u32);

impl fmt::Debug for ErasedFileAstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ErasedFileAstId").field(&self.0).finish()
    }
}

pub const ROOT_ERASED_FILE_AST_ID: ErasedFileAstId = ErasedFileAstId(0);
pub const FIXUP_ERASED_FILE_AST_ID_MARKER: ErasedFileAstId = ErasedFileAstId(u32::MAX);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyntaxContext {
    edition: Edition,
}

impl SyntaxContext {
    pub const fn root(edition: Edition) -> Self {
        Self { edition }
    }

    pub const fn edition(self) -> Edition {
        self.edition
    }
}

impl fmt::Debug for SyntaxContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SyntaxContext").field(&self.edition).finish()
    }
}
