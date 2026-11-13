//! Adapted from rust-analyzer's `span` crate.
//!
//! Token provenance and syntax-context types for macro expansion.
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

/// Where a macro token came from, independent of where expanded syntax places it.
///
/// `origins` contains occurrence IDs in the table selected by `anchor`. These are not byte
/// offsets. The parser can give an expanded token a new position without changing its origin,
/// and a source reader can resolve that origin against the input's position table later.
///
/// This differs from the engine's physical `Span`, which is a range of file byte offsets.
/// Template tokens and copied arguments keep their own anchors when expansion mixes them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct Span {
    pub origins: TokenOrigins,
    pub anchor: SpanAnchor,
    pub ctx: SyntaxContext,
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Span")
            .field("origins", &self.origins)
            .field("anchor", &self.anchor)
            .field("ctx", &self.ctx)
            .finish()
    }
}

/// Identifies the source table containing a macro token. Body inputs are scoped to one
/// expansion request; declaration inputs are replayable through the file's owner table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub enum TokenOwner {
    /// The number is a declaration source owner. Token origin IDs select entries within it.
    Declaration(u32),
    /// Token origin IDs select entries in the macro invocation's transient input map.
    /// The caller must keep that map alongside any expansion that still refers to it.
    Body,
}

/// Endpoints in a source-occurrence table, never text offsets. A merged parser token can
/// originate in two input tokens. Keep both endpoints even when expansion reorders them.
///
/// For example, the parser may join two `:` punctuation tokens into `::`. The two IDs locate
/// their original ranges; covering those ranges gives the physical span of the joined token.
/// The endpoints need not be consecutive or numerically increasing, and they do not describe
/// all the entries between them.
///
/// A normal single token uses the same ID for both endpoints. A synthetic token keeps a first
/// ID for provenance but has no last endpoint, so readers cannot resolve it to editable input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct TokenOrigins {
    first: u32,
    last: Option<u32>,
}

impl TokenOrigins {
    pub fn single(source: u32) -> Self {
        Self {
            first: source,
            last: Some(source),
        }
    }

    pub fn synthetic(source: u32) -> Self {
        Self {
            first: source,
            last: None,
        }
    }

    pub fn first(self) -> u32 {
        self.first
    }

    pub fn last(self) -> Option<u32> {
        self.last
    }

    /// Keep the endpoints of a combined token or syntax range. The caller must first check that
    /// both origins belong to the same input table. A synthetic endpoint keeps the result
    /// synthetic rather than turning generated punctuation into an editable range.
    pub fn join(self, right: Self) -> Self {
        Self {
            first: self.first,
            last: self.last.and(right.last),
        }
    }

    // The compact token codec stores two integers as a starting ID and a wrapping delta.
    // These operations are private to storage; the delta is not a source length.
    pub(crate) fn encoded_width(self) -> u32 {
        self.last
            .map_or(0, |last| last.wrapping_add(1).wrapping_sub(self.first))
    }
    pub(crate) fn from_encoded(first: u32, width: u32) -> Self {
        Self {
            first,
            last: Some(first.wrapping_add(width).wrapping_sub(1)),
        }
    }
    pub(crate) fn shifted(self, offset: u32) -> Self {
        Self {
            first: self.first.wrapping_add(offset),
            last: self.last.map(|last| last.wrapping_add(offset)),
        }
    }
}

/// A token's canonical source file and the package that supplied it.
///
/// Expansion can mix tokens from several packages. Keep the package context even when two
/// packages share the same canonical file.
/// Together with `owner`, this selects the table in which [`TokenOrigins`] must be interpreted.
/// Matching numeric occurrence IDs without matching their anchors does not establish shared origin.
#[derive(Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
pub struct SpanAnchor {
    pub owner: TokenOwner,
    pub package: u32,
    pub file_id: EditionedFileId,
}

impl fmt::Debug for SpanAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpanAnchor")
            .field(&self.owner)
            .field(&self.package)
            .field(&self.file_id)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditionedFileId(u32);

impl EditionedFileId {
    const EDITION_BITS_OFFSET: u32 = 24;
    const FILE_ID_MASK: u32 = 0x00ff_ffff;

    pub const fn new(raw_file_id: u32, edition: Edition) -> Self {
        assert!(
            raw_file_id <= Self::FILE_ID_MASK,
            "file id exceeds macro span storage"
        );
        Self((raw_file_id & Self::FILE_ID_MASK) | ((edition as u32) << Self::EDITION_BITS_OFFSET))
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn raw_file_id(self) -> u32 {
        self.0 & Self::FILE_ID_MASK
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, SchemaRead, SchemaWrite)]
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
    fn wincode_preserves_syntax_context_edition() {
        let ctx = SyntaxContext::root(Edition::Edition2015);

        let bytes = wincode::serialize(&ctx).expect("syntax context should serialize");
        let decoded: SyntaxContext =
            wincode::deserialize(&bytes).expect("syntax context should deserialize");

        assert_eq!(decoded, ctx);
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
