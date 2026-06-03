mod fixture;
#[doc(hidden)]
pub mod testonly;

pub use self::fixture::{
    CrateFixture, FixtureMarker, FixtureMarkers, FixtureSpec, fixture_crate,
    fixture_crate_with_markers,
};
