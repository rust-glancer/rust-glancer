//! Small adapters for rust-glancer's `wincode` schemas.

use std::marker::PhantomData;

use wincode::{SchemaRead, SchemaWrite};

/// Wincode adapter for recursive schema fields.
///
/// Wincode derives try to compute static metadata through every field. Recursive IR nodes must
/// stay dynamic so test builds do not evaluate chains like `TypeRef -> Box<TypeRef> -> TypeRef`
/// forever.
pub struct WincodeDynamic<T: ?Sized>(PhantomData<T>);

unsafe impl<C, T> SchemaWrite<C> for WincodeDynamic<T>
where
    C: wincode::config::ConfigCore,
    T: SchemaWrite<C> + ?Sized,
{
    type Src = T::Src;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn size_of(src: &Self::Src) -> wincode::WriteResult<usize> {
        <T as SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl wincode::io::Writer, src: &Self::Src) -> wincode::WriteResult<()> {
        <T as SchemaWrite<C>>::write(writer, src)
    }
}

unsafe impl<'de, C, T> SchemaRead<'de, C> for WincodeDynamic<T>
where
    C: wincode::config::ConfigCore,
    T: SchemaRead<'de, C> + ?Sized,
{
    type Dst = T::Dst;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut std::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        <T as SchemaRead<'de, C>>::read(reader, dst)
    }
}

/// Wincode adapter for values that must never be persisted.
///
/// This is useful for transient enum variants that still belong to a shared data type. The derive
/// can keep owning the real schema, while this adapter makes any attempt to encode or decode the
/// transient payload fail loudly.
pub struct WincodeUnsupported<T>(PhantomData<T>);

const WINCODE_UNSUPPORTED_ERROR: &str = "unsupported wincode field";

unsafe impl<C, T> SchemaWrite<C> for WincodeUnsupported<T>
where
    C: wincode::config::ConfigCore,
{
    type Src = T;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn size_of(_src: &Self::Src) -> wincode::WriteResult<usize> {
        Err(wincode::WriteError::Custom(WINCODE_UNSUPPORTED_ERROR))
    }

    fn write(_writer: impl wincode::io::Writer, _src: &Self::Src) -> wincode::WriteResult<()> {
        Err(wincode::WriteError::Custom(WINCODE_UNSUPPORTED_ERROR))
    }
}

unsafe impl<'de, C, T> SchemaRead<'de, C> for WincodeUnsupported<T>
where
    C: wincode::config::ConfigCore,
{
    type Dst = T;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn read(
        _reader: impl wincode::io::Reader<'de>,
        _dst: &mut std::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        Err(wincode::ReadError::Custom(WINCODE_UNSUPPORTED_ERROR))
    }
}
