use std::{
    io,
    path::{Path, PathBuf},
};

pub(crate) fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "while attempting to canonicalize {}: {error}",
                path.display()
            ),
        )
    })
}
