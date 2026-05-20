//! Adapted from rust-analyzer's `span` crate.
//!
//! Coarse span and syntax-context types for macro expansion.
//!
//! These types intentionally model only the subset of rust-analyzer's `span`
//! module that `tt`, `mbe`, and the token-tree syntax bridge need.

use std::{fmt, mem::MaybeUninit};

pub use parser::Edition;
pub use text_size::{TextRange, TextSize};
use wincode::{
    ReadError, ReadResult, SchemaRead, SchemaWrite, TypeMeta, WriteResult,
    config::ConfigCore,
    io::{Reader, Writer},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct Span {
    #[wincode(with = "crate::wincode_adapters::TextRangeCodec")]
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
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
            _ => panic!("invalid edition encoded in file id"),
        }
    }
}

unsafe impl<C: ConfigCore> SchemaWrite<C> for EditionedFileId {
    type Src = EditionedFileId;

    const TYPE_META: TypeMeta = <u32 as SchemaWrite<C>>::TYPE_META;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <u32 as SchemaWrite<C>>::size_of(&src.0)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <u32 as SchemaWrite<C>>::write(writer, &src.0)
    }
}

unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for EditionedFileId {
    type Dst = EditionedFileId;

    const TYPE_META: TypeMeta = <u32 as SchemaRead<'de, C>>::TYPE_META;

    fn read(reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let raw = <u32 as SchemaRead<'de, C>>::get(reader)?;
        let edition = (raw >> EditionedFileId::EDITION_BITS_OFFSET) as u8;
        if crate::wincode_adapters::decode_edition_tag(edition).is_none() {
            return Err(ReadError::InvalidValue(
                "invalid edition encoded in file id",
            ));
        }

        dst.write(EditionedFileId(raw));
        Ok(())
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct ErasedFileAstId(pub u32);

impl fmt::Debug for ErasedFileAstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ErasedFileAstId").field(&self.0).finish()
    }
}

pub const ROOT_ERASED_FILE_AST_ID: ErasedFileAstId = ErasedFileAstId(0);
#[derive(Clone, Copy, PartialEq, Eq, Hash, wincode::SchemaRead, wincode::SchemaWrite)]
pub struct SyntaxContext {
    #[wincode(with = "crate::wincode_adapters::EditionCodec")]
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

#[cfg(test)]
mod tests {
    use super::{Edition, EditionedFileId, SyntaxContext};

    #[test]
    fn wincode_preserves_non_current_syntax_context_edition() {
        let ctx = SyntaxContext::root(Edition::Edition2015);

        let bytes = wincode::serialize(&ctx).expect("syntax context should serialize");
        let decoded: SyntaxContext =
            wincode::deserialize(&bytes).expect("syntax context should deserialize");

        assert_eq!(decoded.edition(), Edition::Edition2015);
    }

    #[test]
    fn wincode_rejects_invalid_syntax_context_edition() {
        let invalid_edition = wincode::serialize(&4_u8).expect("tag should serialize");
        let decoded: wincode::ReadResult<SyntaxContext> = wincode::deserialize(&invalid_edition);

        assert!(matches!(
            decoded,
            Err(wincode::ReadError::InvalidValue("invalid Rust edition tag"))
        ));
    }

    #[test]
    fn wincode_rejects_invalid_file_id_edition() {
        let invalid_file_id = wincode::serialize(&(4_u32 << EditionedFileId::EDITION_BITS_OFFSET))
            .expect("file id should serialize");
        let decoded: wincode::ReadResult<EditionedFileId> = wincode::deserialize(&invalid_file_id);

        assert!(matches!(
            decoded,
            Err(wincode::ReadError::InvalidValue(
                "invalid edition encoded in file id"
            ))
        ));
    }
}
