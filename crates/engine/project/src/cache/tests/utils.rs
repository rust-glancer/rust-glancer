use rg_body_ir::PackageBodies;
use rg_def_map::PackageSlot;
use rg_ir_model::CrateId;

use crate::cache::{
    PackageArtifactReader, PackageCacheHeader, PackageCacheUpdate, PackageCacheWriteInput,
};
use crate::{Project, testonly::ProjectFixture};

pub(super) fn package_cache_header(project: &Project, package: PackageSlot) -> PackageCacheHeader {
    let state = &project.state;
    state
        .cache_plan
        .artifact_header(package, &state.package_source_fingerprints)
        .expect("cache-planned fixture package should have an artifact header")
}

/// Write one resident fixture package through the same borrowed transaction path as production.
pub(super) fn write_resident_package_artifact(
    project: &Project,
    package: PackageSlot,
) -> PackageCacheHeader {
    let state = &project.state;
    let header = package_cache_header(project, package);
    let parse = state
        .parse
        .package(package.0)
        .expect("fixture package should have parse data")
        .parse_snapshot()
        .expect("fixture parse metadata should snapshot");
    let def_map = state
        .def_map
        .resident_package(package)
        .expect("fixture package should have def-map data");
    let semantic_ir = state
        .semantic_ir
        .resident_package(package)
        .expect("fixture package should have semantic IR data");
    let body_ir = state
        .body_ir
        .resident_package(package)
        .expect("fixture package should have body IR data");

    let update = state
        .cache_store
        .begin_artifact_update()
        .expect("fixture package cache update should start");
    update
        .write_input(PackageCacheWriteInput::new(
            &header,
            &parse,
            def_map,
            semantic_ir,
            body_ir,
        ))
        .expect("fixture resident package should write to cache");
    update
        .commit()
        .expect("fixture package cache update should commit");
    header
}

pub(super) fn package_cache_header_for(
    project: &Project,
    package_name: &str,
) -> PackageCacheHeader {
    let package = ProjectFixture::package_slot_by_name_in(project.state.parse_db(), package_name);
    package_cache_header(project, package)
}

/// Re-emit one cached package through the production lazy reader and borrowed writer.
pub(super) fn write_cached_package_artifact(
    update: &PackageCacheUpdate<'_>,
    reader: &PackageArtifactReader,
    header: &PackageCacheHeader,
) {
    let def_map = reader
        .read_def_map()
        .expect("cached fixture DefMap should read");
    let semantic_ir = reader
        .read_semantic_ir()
        .expect("cached fixture Semantic IR should read");
    let manifest = reader
        .read_body_ir_manifest()
        .expect("cached fixture Body IR manifest should read");
    let body_ir = PackageBodies::new(
        (0..manifest.crates().len())
            .map(|target| {
                reader
                    .read_body_crate(CrateId(target))
                    .expect("cached fixture Body IR target should read")
            })
            .collect(),
    );

    update
        .write_input(PackageCacheWriteInput::new(
            header,
            &reader.probe().parse,
            &def_map,
            &semantic_ir,
            &body_ir,
        ))
        .expect("cached fixture package should write")
}

pub(super) fn assert_reader_matches_resident_package(
    reader: &PackageArtifactReader,
    project: &Project,
    package: PackageSlot,
) {
    let state = &project.state;
    let expected_header = package_cache_header(project, package);
    let expected_parse = state
        .parse
        .package(package.0)
        .expect("fixture package should have parse data")
        .parse_snapshot()
        .expect("fixture parse metadata should snapshot");
    assert_eq!(reader.probe().header, expected_header);
    assert_eq!(reader.probe().parse, expected_parse);
    assert_eq!(
        reader
            .read_def_map()
            .expect("fixture cached DefMap should read"),
        *state
            .def_map
            .resident_package(package)
            .expect("fixture package should have resident DefMap"),
    );
    assert_eq!(
        reader
            .read_semantic_ir()
            .expect("fixture cached Semantic IR should read"),
        *state
            .semantic_ir
            .resident_package(package)
            .expect("fixture package should have resident Semantic IR"),
    );

    let expected_body_ir = state
        .body_ir
        .resident_package(package)
        .expect("fixture package should have resident Body IR");
    assert_eq!(
        reader
            .read_body_ir_manifest()
            .expect("fixture cached Body IR manifest should read"),
        expected_body_ir.manifest(),
    );
    for (target, expected) in expected_body_ir.crates().iter().enumerate() {
        assert_eq!(
            reader
                .read_body_crate(CrateId(target))
                .expect("fixture cached Body IR target should read"),
            *expected,
        );
    }
}
