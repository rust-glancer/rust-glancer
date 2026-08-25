//! Reversible host-native strings used by disposable cache schemas.

use std::ffi::{OsStr, OsString};

use wincode::{SchemaRead, SchemaWrite};

use crate::{MemoryRecorder, MemorySize};

/// An OS string encoded without a UTF-8 or display-text conversion.
///
/// The encoding is deliberately host-local because rust-glancer caches are not portable between
/// operating systems. Unix stores native bytes; Windows stores little-endian UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, SchemaRead, SchemaWrite)]
pub struct NativeOsString(Vec<u8>);

impl NativeOsString {
    pub fn from_os_str(value: &OsStr) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            Self(value.as_bytes().to_vec())
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            Self(value.encode_wide().flat_map(u16::to_le_bytes).collect())
        }
    }

    pub fn into_os_string(self) -> Option<OsString> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            Some(OsString::from_vec(self.0))
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt as _;
            let mut chunks = self.0.chunks_exact(2);
            let units = chunks
                .by_ref()
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect::<Vec<_>>();
            chunks
                .remainder()
                .is_empty()
                .then(|| OsString::from_wide(&units))
        }
    }

    pub fn as_encoded_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl MemorySize for NativeOsString {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::NativeOsString;

    #[test]
    fn round_trips_host_native_units() {
        #[cfg(unix)]
        let value = {
            use std::os::unix::ffi::OsStringExt as _;
            OsString::from_vec(b"workspace/src/non-utf8-\xff.rs".to_vec())
        };
        #[cfg(windows)]
        let value = {
            use std::os::windows::ffi::OsStringExt as _;
            OsString::from_wide(&[u16::from(b'C'), u16::from(b':'), u16::from(b'\\'), 0xd800])
        };

        let encoded = NativeOsString::from_os_str(&value);

        assert_eq!(encoded.into_os_string(), Some(value));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_an_incomplete_wide_code_unit() {
        assert_eq!(NativeOsString(vec![0]).into_os_string(), None);
    }
}
