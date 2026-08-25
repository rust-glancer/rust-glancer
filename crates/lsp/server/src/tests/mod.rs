use std::path::{Path, PathBuf};

use rg_std::NormalizedPathBuf;

mod utils;

mod engine_routing;

/// Returns a host-native absolute synthetic path below the test process's working directory.
///
/// The path looks like a real host-native absolute path but intentionally does not exist.
pub(crate) fn synthetic_test_path(relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();
    assert!(
        !relative.is_absolute() && !relative.has_root(),
        "test path should be relative: {}",
        relative.display()
    );
    std::fs::canonicalize(".")
        .expect("test process should have a working directory")
        .join("nonexistent-just-for-tests")
        .join(relative)
}

pub(crate) fn normalized_test_path(relative: impl AsRef<Path>) -> NormalizedPathBuf {
    NormalizedPathBuf::from_absolute(synthetic_test_path(relative))
        .expect("test path should normalize")
}
