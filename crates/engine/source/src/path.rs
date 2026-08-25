//! Shared canonical paths for generation-backed source files.
//!
//! A source path appears in several indexes: the source inventory, the parse database's reverse
//! lookup, and the source descriptor used by cache snapshots. Keeping a `PathBuf` in each place
//! repeats long registry and sysroot paths. `SourcePath` gives those structures one immutable path
//! allocation to share without turning path creation into a global interning concern.

use std::{
    borrow::Borrow,
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_std::{MemoryRecorder, MemorySize, NativeOsString, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Shared canonical filesystem path for one generation-backed source file.
///
/// Construction stays inside `rg_source`: callers receive the path chosen by the inventory and
/// can clone the handle for their own path-indexed storage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourcePath(Arc<Path>);

impl SourcePath {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(Arc::from(path.into_boxed_path()))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Records the shared allocation at the descriptor that owns its memory attribution.
    ///
    /// Other `SourcePath` handles are lookup aliases. Counting from every map key would report the
    /// same Arc allocation several times.
    pub(crate) fn record_allocation(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

impl AsRef<Path> for SourcePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl Borrow<Path> for SourcePath {
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

// `SourceDescriptor` is the accounting owner for the shared allocation. Map keys and parsed-file
// aliases still contribute their inline handle bytes through their containers, but not the same
// path allocation again.
impl MemorySize for SourcePath {
    fn record_memory_children(&self, _recorder: &mut MemoryRecorder) {}
}

impl Shrink for SourcePath {
    fn shrink_to_fit(&mut self) {}
}

// The native encoding keeps non-UTF-8 Unix names and Windows path units reversible while Arc
// sharing remains a runtime-only detail of a restored project generation.
unsafe impl<C> SchemaWrite<C> for SourcePath
where
    C: wincode::config::Config,
{
    type Src = SourcePath;

    fn size_of(src: &Self::Src) -> wincode::WriteResult<usize> {
        let path = NativeOsString::from_os_str(src.as_path().as_os_str());
        <NativeOsString as SchemaWrite<C>>::size_of(&path)
    }

    fn write(writer: impl wincode::io::Writer, src: &Self::Src) -> wincode::WriteResult<()> {
        let path = NativeOsString::from_os_str(src.as_path().as_os_str());
        <NativeOsString as SchemaWrite<C>>::write(writer, &path)
    }
}

unsafe impl<'de, C> SchemaRead<'de, C> for SourcePath
where
    C: wincode::config::Config,
{
    type Dst = SourcePath;

    fn read(
        reader: impl wincode::io::Reader<'de>,
        dst: &mut std::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        let path = <NativeOsString as SchemaRead<'de, C>>::get(reader)?
            .into_os_string()
            .map(PathBuf::from)
            .ok_or(wincode::ReadError::InvalidValue(
                "cached source path has an invalid native encoding",
            ))?;
        dst.write(Self::new(path));
        Ok(())
    }
}
