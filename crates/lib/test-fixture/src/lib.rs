use std::path::{Path, PathBuf};

mod fake_sysroot;
mod fixture;
#[doc(hidden)]
pub mod testonly;

pub use self::fixture::{
    CrateFixture, FixtureMarker, FixtureMarkers, FixtureSpec, fixture_crate,
    fixture_crate_with_markers, fixture_path_for_snapshot,
};

/// Returns a host-native absolute path for tests that need a realistic document identity.
///
/// `relative` describes the readable fixture layout, while the returned path is rooted below the
/// test process's working directory. Nothing is created on disk.
pub fn synthetic_test_path(relative: impl AsRef<Path>) -> PathBuf {
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
