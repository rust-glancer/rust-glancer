//! Small adapters for rust-glancer's `wincode` schemas.

/// Wincode adapter for recursive schema fields.
///
/// Wincode derives try to compute static metadata through every field. Recursive IR nodes must
/// stay dynamic so test builds do not evaluate chains like `TypeRef -> Box<TypeRef> -> TypeRef`
/// forever.
pub struct WincodeDynamic<T: ?Sized>(std::marker::PhantomData<T>);

unsafe impl<C, T> wincode::SchemaWrite<C> for WincodeDynamic<T>
where
    C: wincode::config::ConfigCore,
    T: wincode::SchemaWrite<C> + ?Sized,
{
    type Src = T::Src;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn size_of(src: &Self::Src) -> wincode::WriteResult<usize> {
        <T as wincode::SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl wincode::io::Writer, src: &Self::Src) -> wincode::WriteResult<()> {
        <T as wincode::SchemaWrite<C>>::write(writer, src)
    }
}

unsafe impl<'de, C, T> wincode::SchemaRead<'de, C> for WincodeDynamic<T>
where
    C: wincode::config::ConfigCore,
    T: wincode::SchemaRead<'de, C> + ?Sized,
{
    type Dst = T::Dst;

    const TYPE_META: wincode::TypeMeta = wincode::TypeMeta::Dynamic;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut std::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        <T as wincode::SchemaRead<'de, C>>::read(reader, dst)
    }
}
