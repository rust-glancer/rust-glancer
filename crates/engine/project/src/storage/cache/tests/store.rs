use std::fs;

use rg_ir_model::{CrateId, EvaluatedConstValue, PackageSlot, UnsignedIntTy};

use super::utils::{
    assert_reader_matches_resident_package, package_cache_header, write_resident_package_artifact,
};
use crate::testonly::ProjectSourceFixture;

#[test]
fn stores_package_cache_artifacts_on_disk() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app
pub struct App;
pub const CAPACITY: usize = 2 * 8;
pub const LABEL: &str = "a  b";
pub static SLOTS: usize = 1 << 10;
pub static STATIC_LABEL: &str = "static  label";
"#,
    );
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = package_cache_header(&project, PackageSlot(0));
    let path = store.package_artifact_path(&header.package);

    store
        .clear_package_artifacts()
        .expect("fixture cache namespace should start empty for direct store I/O");
    assert!(
        store
            .open_artifact(&header)
            .expect("missing package cache artifact should not fail")
            .is_none(),
        "cache namespace should start empty",
    );

    let written_header = write_resident_package_artifact(&project, PackageSlot(0));
    assert_eq!(written_header, header);
    let loaded = store
        .open_artifact(&header)
        .expect("written package cache artifact should read from disk")
        .expect("written package cache artifact should exist");
    assert_reader_matches_resident_package(&loaded, &project, PackageSlot(0));
    let semantic_ir = loaded
        .read_semantic_ir()
        .expect("cached const declaration should decode");
    assert_eq!(
        semantic_ir
            .crate_items(CrateId(0))
            .and_then(|items| items.consts().iter().find_map(|data| data.value.as_deref())),
        Some(&EvaluatedConstValue::UnsignedInt {
            value: 16,
            ty: UnsignedIntTy::Usize
        }),
        "evaluated initializer value survives the cache roundtrip",
    );
    assert_eq!(
        semantic_ir
            .crate_items(CrateId(0))
            .and_then(|items| items
                .consts()
                .iter()
                .find(|data| data.name.as_str() == "CAPACITY"))
            .and_then(|data| data.initializer_preview.as_deref()),
        Some("2 * 8"),
        "evaluated const also retains its source initializer in the cache",
    );
    assert_eq!(
        semantic_ir
            .crate_items(CrateId(0))
            .and_then(|items| items
                .consts()
                .iter()
                .find(|data| data.name.as_str() == "LABEL"))
            .and_then(|data| data.initializer_preview.as_deref()),
        Some(r#""a  b""#),
        "initializer preview survives the cache roundtrip verbatim",
    );
    let statics = semantic_ir
        .crate_items(CrateId(0))
        .expect("cached crate should exist")
        .statics();
    let slots = statics
        .iter()
        .find(|data| data.name.as_str() == "SLOTS")
        .expect("cached static should exist");
    assert_eq!(
        slots.value.as_deref(),
        Some(&EvaluatedConstValue::UnsignedInt {
            value: 1024,
            ty: UnsignedIntTy::Usize
        }),
        "static initial value survives the cache roundtrip",
    );
    assert_eq!(slots.initializer_preview.as_deref(), Some("1 << 10"));
    assert_eq!(
        statics
            .iter()
            .find(|data| data.name.as_str() == "STATIC_LABEL")
            .and_then(|data| data.initializer_preview.as_deref()),
        Some(r#""static  label""#),
        "static initializer preview survives the cache roundtrip verbatim",
    );
    assert_eq!(loaded.probe().header.package.package.0, 0);
    assert_eq!(loaded.probe().header.package.name, "app");
    assert!(
        fs::metadata(&path)
            .expect("written package cache artifact should have file metadata")
            .len()
            > 0,
        "written artifact should contain bytes",
    );

    // Corruption is surfaced as a cache problem, not silently treated as a miss. The higher-level
    // invalidation layer will decide whether to wipe and rebuild.
    fs::write(&path, b"not a package cache artifact")
        .expect("test should overwrite package cache artifact with invalid bytes");
    let corrupt_error = store
        .open_artifact(&header)
        .expect_err("corrupted package cache artifact should fail to decode");
    assert!(
        format!("{corrupt_error:#}").contains("failed to decode artifact"),
        "corrupt reads should retain typed decode context: {corrupt_error:#}",
    );

    store
        .clear_package_artifacts()
        .expect("package cache artifacts should be removable");
    assert!(
        store
            .open_artifact(&header)
            .expect("missing package cache artifact should not fail after invalidation")
            .is_none(),
        "artifact should be absent after invalidation",
    );
}

#[test]
fn removes_stale_package_cache_generations() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:app
pub struct App;
"#,
    );
    let project = fixture.build_project();
    let store = project.state.cache_store.clone();
    let header = write_resident_package_artifact(&project, PackageSlot(0));
    let current_artifact = store.package_artifact_path(&header.package);
    assert!(
        current_artifact.exists(),
        "current artifact should exist before stale-generation cleanup",
    );

    let packages_dir = store.root().join("packages");
    let stale_generation = packages_dir.join("graph-stale");
    fs::create_dir_all(&stale_generation).expect("stale generation dir should be creatable");
    fs::write(stale_generation.join("old.rgpkg"), b"old artifact")
        .expect("stale generation artifact should be writable");

    store
        .cleanup_stale_generations()
        .expect("stale generation cleanup should succeed");

    assert!(
        !stale_generation.exists(),
        "stale cache generation should be removed",
    );
    assert!(
        current_artifact.exists(),
        "current cache artifact should survive stale-generation cleanup",
    );
}
