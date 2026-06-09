use super::utils::{RoutingFixture, RoutingStep, check_routing};

const MULTI_PROJECT_FIXTURE: &str = r#"
//- /workspace/Cargo.toml
[workspace]
members = ["project_a", "project_b"]
resolver = "3"

//- /workspace/project_a/Cargo.toml
[package]
name = "project_a"
version = "0.1.0"
edition = "2024"

//- /workspace/project_a/src/lib.rs
pub struct ProjectA;

//- /workspace/project_a/vendor/member/Cargo.toml
[package]
name = "member"
version = "0.1.0"
edition = "2024"

//- /workspace/project_a/vendor/member/src/lib.rs
pub struct NestedMember;

//- /workspace/project_b/Cargo.toml
[package]
name = "project_b"
version = "0.1.0"
edition = "2024"

//- /workspace/project_b/src/lib.rs
pub struct ProjectB;

//- /external/thin_vec/src/lib.rs
pub struct ExternalDependency;
"#;

#[test]
fn routes_documents_to_existing_spawned_or_active_engines() {
    check_routing(
        RoutingFixture::new(MULTI_PROJECT_FIXTURE).workspace_folders(["workspace"]),
        &[
            RoutingStep::cached_file_owner(
                "unopened workspace member is not cached",
                "workspace/project_a/src/lib.rs",
            ),
            RoutingStep::workspace_root("resolved workspace root spawns engine", "workspace"),
            RoutingStep::open_file(
                "didOpen caches exact file owner",
                "workspace/project_a/src/lib.rs",
            ),
            RoutingStep::cached_file_owner(
                "same open file reuses cached owner",
                "workspace/project_a/src/lib.rs",
            ),
            RoutingStep::cached_file_owner(
                "different unopened file is not cached",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::open_file(
                "didOpen caches second exact file",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::cached_file_owner(
                "second open file reuses cached owner",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::close_file(
                "didClose drops exact file owner",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::cached_file_owner(
                "closed file is no longer cached",
                "workspace/project_b/src/lib.rs",
            ),
            RoutingStep::workspace_action("external file keeps active engine"),
        ],
        r#"
unopened workspace member is not cached: unowned
resolved workspace root spawns engine: spawn e0 /workspace
didOpen caches exact file owner: active e0 /workspace
same open file reuses cached owner: existing e0 /workspace
different unopened file is not cached: unowned
didOpen caches second exact file: active e0 /workspace
second open file reuses cached owner: existing e0 /workspace
didClose drops exact file owner: closed
closed file is no longer cached: unowned
external file keeps active engine: active e0 /workspace
"#,
    );
}

#[test]
fn respects_workspace_folder_boundaries_during_discovery_and_spawn() {
    check_routing(
        RoutingFixture::new(MULTI_PROJECT_FIXTURE)
            .workspace_folders(["workspace/project_a", "workspace/project_a/vendor"]),
        &[
            RoutingStep::discovery_workspace(
                "project file discovers nearest configured folder",
                "workspace/project_a/src/lib.rs",
            ),
            RoutingStep::discovery_workspace(
                "nested file discovers most specific configured folder",
                "workspace/project_a/vendor/member/src/lib.rs",
            ),
            RoutingStep::discovery_workspace(
                "external file has no discovery folder",
                "external/thin_vec/src/lib.rs",
            ),
            RoutingStep::workspace_root(
                "resolved project root inside configured folder spawns engine",
                "workspace/project_a",
            ),
            RoutingStep::workspace_root(
                "resolved external root outside configured folders does not spawn",
                "external",
            ),
        ],
        r#"
project file discovers nearest configured folder: /workspace/project_a
nested file discovers most specific configured folder: /workspace/project_a/vendor
external file has no discovery folder: none
resolved project root inside configured folder spawns engine: spawn e0 /workspace/project_a
resolved external root outside configured folders does not spawn: none
"#,
    );
}
