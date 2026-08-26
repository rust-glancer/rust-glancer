use std::path::Path;

use rg_std::NormalizedPathBuf;
pub(crate) use test_fixture::synthetic_test_path;

mod utils;

mod engine_routing;

pub(crate) fn normalized_test_path(relative: impl AsRef<Path>) -> NormalizedPathBuf {
    NormalizedPathBuf::from_absolute(synthetic_test_path(relative))
        .expect("test path should normalize")
}
