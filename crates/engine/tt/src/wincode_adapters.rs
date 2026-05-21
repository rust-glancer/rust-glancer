//! Wincode adapters for foreign syntax types used inside token-tree spans.

use std::mem::MaybeUninit;

use parser::Edition;
use text_size::{TextRange, TextSize};
use wincode::{
    ReadError, ReadResult, SchemaRead, SchemaWrite, TypeMeta, WriteResult, config::ConfigCore,
    io::Writer,
};

pub(crate) struct TextRangeCodec;

unsafe impl<C: ConfigCore> SchemaWrite<C> for TextRangeCodec {
    type Src = TextRange;

    const TYPE_META: TypeMeta = TypeMeta::Dynamic;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        let start = u32::from(src.start());
        let end = u32::from(src.end());
        Ok(<u32 as SchemaWrite<C>>::size_of(&start)? + <u32 as SchemaWrite<C>>::size_of(&end)?)
    }

    fn write(mut writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <u32 as SchemaWrite<C>>::write(writer.by_ref(), &u32::from(src.start()))?;
        <u32 as SchemaWrite<C>>::write(writer, &u32::from(src.end()))
    }
}

unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for TextRangeCodec {
    type Dst = TextRange;

    const TYPE_META: TypeMeta = TypeMeta::Dynamic;

    fn read(
        mut reader: impl wincode::io::Reader<'de>,
        dst: &mut MaybeUninit<Self::Dst>,
    ) -> ReadResult<()> {
        let start = <u32 as SchemaRead<'de, C>>::get(reader.by_ref())?;
        let end = <u32 as SchemaRead<'de, C>>::get(reader)?;
        dst.write(TextRange::new(TextSize::new(start), TextSize::new(end)));
        Ok(())
    }
}

pub(crate) struct EditionCodec;

unsafe impl<C: ConfigCore> SchemaWrite<C> for EditionCodec {
    type Src = Edition;

    const TYPE_META: TypeMeta = <u8 as SchemaWrite<C>>::TYPE_META;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <u8 as SchemaWrite<C>>::size_of(&edition_to_u8(*src))
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <u8 as SchemaWrite<C>>::write(writer, &edition_to_u8(*src))
    }
}

unsafe impl<'de, C: ConfigCore> SchemaRead<'de, C> for EditionCodec {
    type Dst = Edition;

    const TYPE_META: TypeMeta = <u8 as SchemaRead<'de, C>>::TYPE_META;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut MaybeUninit<Self::Dst>,
    ) -> ReadResult<()> {
        let edition = decode_edition_tag(<u8 as SchemaRead<'de, C>>::get(reader)?)
            .ok_or(ReadError::InvalidValue("invalid Rust edition tag"))?;
        dst.write(edition);
        Ok(())
    }
}

pub(crate) fn decode_edition_tag(tag: u8) -> Option<Edition> {
    match tag {
        0 => Some(Edition::Edition2015),
        1 => Some(Edition::Edition2018),
        2 => Some(Edition::Edition2021),
        3 => Some(Edition::Edition2024),
        _ => None,
    }
}

pub(crate) fn edition_to_u8(edition: Edition) -> u8 {
    match edition {
        Edition::Edition2015 => 0,
        Edition::Edition2018 => 1,
        Edition::Edition2021 => 2,
        Edition::Edition2024 => 3,
    }
}
