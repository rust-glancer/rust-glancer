use std::path::{Path, PathBuf};

use rg_lsp_proto::{CargoMetadataTarget, PackageResidencyPolicy};
use serde_json::json;

use super::ServerConfig;

#[test]
fn exact_override_merges_with_base_cargo_config() {
    let options = json!({
        "cache": {
            "packageResidency": "all-resident",
        },
        "cargo": {
            "target": "x86_64-unknown-linux-gnu",
            "allFeatures": true,
            "noDefaultFeatures": false,
            "features": ["base"],
            "overrides": [{
                "path": "project-a",
                "noDefaultFeatures": true,
                "features": [],
            }],
        },
    });

    let config =
        ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
            .expect("server config should parse");
    let project_config = config.engine_config_for_root(Path::new("/repo/project-a"));
    let default_config = config.engine_config_for_root(Path::new("/repo/project-b"));

    assert_eq!(
        project_config.analysis.package_residency_policy,
        PackageResidencyPolicy::AllResident,
        "non-cargo engine settings should remain inherited",
    );
    assert_eq!(
        project_config.analysis.cargo_metadata_config.target(),
        &CargoMetadataTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
    );
    assert!(
        project_config
            .analysis
            .cargo_metadata_config
            .all_features_enabled()
    );
    assert!(
        project_config
            .analysis
            .cargo_metadata_config
            .no_default_features_enabled()
    );
    assert!(
        project_config
            .analysis
            .cargo_metadata_config
            .features()
            .is_empty(),
        "explicit empty features should clear inherited custom features",
    );

    assert!(
        !default_config
            .analysis
            .cargo_metadata_config
            .no_default_features_enabled()
    );
    assert_eq!(
        default_config.analysis.cargo_metadata_config.features(),
        &["base".to_string()],
    );
}

#[test]
fn override_matches_exact_engine_root_only() {
    let options = json!({
        "cargo": {
            "features": ["base"],
            "overrides": [{
                "path": "project-a",
                "features": ["override"],
            }],
        },
    });
    let config =
        ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
            .expect("server config should parse");

    assert_eq!(
        config
            .engine_config_for_root(Path::new("/repo/project-a"))
            .analysis
            .cargo_metadata_config
            .features(),
        &["override".to_string()],
    );
    assert_eq!(
        config
            .engine_config_for_root(Path::new("/repo/project-a/nested"))
            .analysis
            .cargo_metadata_config
            .features(),
        &["base".to_string()],
        "child workspace roots should not inherit a parent override",
    );
    assert_eq!(
        config
            .engine_config_for_root(Path::new("/repo"))
            .analysis
            .cargo_metadata_config
            .features(),
        &["base".to_string()],
        "parent workspace roots should not inherit a child override",
    );
}

#[test]
fn latest_duplicate_override_wins_without_warning() {
    let options = json!({
        "cargo": {
            "overrides": [
                {
                    "path": "/repo/project",
                    "allFeatures": true,
                    "features": ["old"],
                },
                {
                    "path": "/repo/project",
                    "allFeatures": false,
                    "features": ["new"],
                },
            ],
        },
    });
    let config = ServerConfig::from_initialization_options(Some(&options), &[])
        .expect("server config should parse");
    let project_config = config.engine_config_for_root(Path::new("/repo/project"));

    assert!(
        !project_config
            .analysis
            .cargo_metadata_config
            .all_features_enabled()
    );
    assert_eq!(
        project_config.analysis.cargo_metadata_config.features(),
        &["new".to_string()],
    );
}

#[test]
fn override_target_null_resets_inherited_target() {
    let options = json!({
        "cargo": {
            "target": "x86_64-unknown-linux-gnu",
            "overrides": [{
                "path": "project-a",
                "target": null,
            }],
        },
    });
    let config =
        ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
            .expect("server config should parse");

    assert_eq!(
        config
            .engine_config_for_root(Path::new("/repo/project-a"))
            .analysis
            .cargo_metadata_config
            .target(),
        &CargoMetadataTarget::Auto,
    );
}

#[test]
fn rejects_malformed_override_entries() {
    let options = json!({
        "cargo": {
            "overrides": [{
                "path": "project-a",
                "features": [true],
            }],
        },
    });
    let error =
        ServerConfig::from_initialization_options(Some(&options), &[PathBuf::from("/repo")])
            .expect_err("malformed override feature entries should be rejected");

    assert!(
        error
            .to_string()
            .contains("rust-glancer cargo.overrides[0].features[0]"),
        "{error:?}",
    );
}
