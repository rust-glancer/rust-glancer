//! The filesystem-path presentation boundary shared by the LSP server and engines.
//!
//! Analysis keeps the host's canonical spelling for identity. Editors need conventional disk and
//! UNC paths instead, so Windows verbatim prefixes are removed only while building protocol
//! values. Inbound paths remain unnormalized; their owning server boundary decides the base and
//! constructs the normalized identity.

use std::path::{Path, PathBuf};

use gen_lsp_types::Uri;

#[derive(Debug, thiserror::Error)]
pub enum FileUriError {
    #[error("URI `{uri}` is not a file URI")]
    NotFileUri { uri: String },
    #[error("file URI `{uri}` does not contain a native absolute path")]
    InvalidFileUri { uri: String },
    #[error("path `{path}` uses an unsupported Windows namespace")]
    UnsupportedWindowsNamespace { path: PathBuf },
    #[error("path `{path}` cannot be represented as a file URI")]
    InvalidFilePath { path: PathBuf },
}

/// Converts an inbound file URI to a native path without assigning filesystem identity.
pub fn file_uri_to_path(uri: &Uri) -> Result<PathBuf, FileUriError> {
    if !uri.scheme().eq_ignore_ascii_case("file") {
        return Err(FileUriError::NotFileUri {
            uri: uri.as_str().to_string(),
        });
    }

    let path = uri
        .to_file_path()
        .ok()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| FileUriError::InvalidFileUri {
            uri: uri.as_str().to_string(),
        })?;
    Ok(path)
}

/// Converts an internal native path to the conventional spelling expected by editors.
pub fn path_for_editor(path: impl AsRef<Path>) -> Result<PathBuf, FileUriError> {
    let path = path.as_ref();
    platform_path_for_editor(path)
}

/// Converts an internal native path to an editor-facing file URI.
pub fn path_to_file_uri(path: impl AsRef<Path>) -> Result<Uri, FileUriError> {
    let path = path_for_editor(path)?;
    Uri::from_file_path(&path).map_err(|()| FileUriError::InvalidFilePath { path })
}

#[cfg(not(windows))]
fn platform_path_for_editor(path: &Path) -> Result<PathBuf, FileUriError> {
    Ok(path.to_path_buf())
}

#[cfg(windows)]
fn platform_path_for_editor(path: &Path) -> Result<PathBuf, FileUriError> {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt as _, OsStringExt as _},
        path::{Component, Prefix},
    };

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Ok(path.to_path_buf());
    };

    let mut conventional = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => {
            PathBuf::from(OsString::from_wide(&[u16::from(drive), u16::from(b':')]))
        }
        Prefix::VerbatimUNC(server, share) => {
            // Build `\\server\share` as native OS-string units. Component boundaries stay
            // explicit and no lossy display string participates in the conversion.
            let mut units = vec![u16::from(b'\\'), u16::from(b'\\')];
            units.extend(server.encode_wide());
            units.push(u16::from(b'\\'));
            units.extend(share.encode_wide());
            PathBuf::from(OsString::from_wide(&units))
        }
        Prefix::Disk(_) | Prefix::UNC(_, _) => return Ok(path.to_path_buf()),
        Prefix::DeviceNS(_) | Prefix::Verbatim(_) => {
            return Err(FileUriError::UnsupportedWindowsNamespace {
                path: path.to_path_buf(),
            });
        }
    };

    for component in components {
        // A verbatim UNC prefix already contains the root separator. Re-pushing RootDir is benign
        // for disk paths but would replace a UNC share on some standard-library versions.
        if matches!(prefix.kind(), Prefix::VerbatimUNC(_, _))
            && matches!(component, Component::RootDir)
        {
            continue;
        }
        conventional.push(component.as_os_str());
    }

    Ok(conventional)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::{FileUriError, file_uri_to_path, path_to_file_uri};
    use gen_lsp_types::Uri;

    #[test]
    fn host_native_file_path_round_trips() {
        let root = std::env::current_dir().expect("test process should have a current directory");
        for filename in ["missing source.rs", "percent%hash#.rs", "unicode-λ😀.rs"] {
            let path = root.join(filename);
            let uri = path_to_file_uri(&path).expect("absolute path should convert to URI");

            assert_eq!(
                file_uri_to_path(&uri).expect("file URI should convert to path"),
                path,
                "{filename}"
            );
        }
    }

    #[test]
    fn non_file_uri_is_rejected() {
        let uri = Uri::from_str("untitled:Scratch").expect("untitled URI should parse");

        assert!(matches!(
            file_uri_to_path(&uri),
            Err(FileUriError::NotFileUri { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_disk_path_is_presented_as_a_conventional_file_uri() {
        use std::path::Path;

        let uri = path_to_file_uri(Path::new(r"\\?\C:\workspace\src\lib.rs"))
            .expect("verbatim disk path should convert to URI");

        assert_eq!(uri.as_str(), "file:///C:/workspace/src/lib.rs");
        assert_eq!(
            file_uri_to_path(&uri).expect("disk URI should convert back to a path"),
            Path::new(r"C:\workspace\src\lib.rs"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_unc_path_uses_the_server_as_file_uri_authority() {
        use std::path::Path;

        let uri = path_to_file_uri(Path::new(r"\\?\UNC\server\share\src\lib.rs"))
            .expect("verbatim UNC path should convert to URI");

        assert_eq!(uri.as_str(), "file://server/share/src/lib.rs");
        assert_eq!(
            file_uri_to_path(&uri).expect("UNC URI should convert back to a path"),
            Path::new(r"\\server\share\src\lib.rs"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn standard_unc_file_uri_converts_to_a_native_unc_path() {
        use std::path::Path;

        let uri =
            Uri::from_str("file://server/share/src/lib.rs").expect("standard UNC URI should parse");

        assert_eq!(
            file_uri_to_path(&uri).expect("standard UNC URI should convert to a path"),
            Path::new(r"\\server\share\src\lib.rs"),
        );
    }
}
