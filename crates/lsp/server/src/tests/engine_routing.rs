use crate::{
    engine_registry::routing::{EngineRouting, WorkspaceEngineRoute},
    tests::normalized_test_path,
};

#[test]
fn routes_documents_to_known_roots_open_file_owners_and_the_active_engine() {
    let workspace = normalized_test_path("workspace");
    let project_a = normalized_test_path("workspace/project_a/src/lib.rs");
    let project_b = normalized_test_path("workspace/project_b/src/lib.rs");
    let external = normalized_test_path("external/thin_vec/src/lib.rs");
    let mut routing = EngineRouting::default();
    routing.set_workspace_folders([workspace.clone()]);

    assert_eq!(routing.open_file_owner(&project_a), None);

    let route = routing
        .route_workspace_root(workspace.clone())
        .expect("configured workspace root should be routed");
    let WorkspaceEngineRoute::Spawn { new_id, root } = route else {
        panic!("first route should reserve a new engine");
    };
    assert_eq!(root, workspace);
    assert_eq!(new_id.index(), 0);
    routing.set_active_id(new_id);

    assert_eq!(
        routing.engine_id_for_known_root_path(&project_a),
        Some(new_id)
    );
    assert_eq!(routing.engine_id_for_known_root_path(&external), None);

    routing.set_open_file(project_a.clone(), new_id);
    assert_eq!(routing.open_file_owner(&project_a), Some(new_id));
    assert_eq!(routing.open_file_owner(&project_b), None);

    routing.set_open_file(project_b.clone(), new_id);
    assert_eq!(routing.open_file_owner(&project_b), Some(new_id));
    assert_eq!(
        routing.remove_open_file(&project_b, None),
        Some(new_id),
        "closing a document should return and remove its exact owner",
    );
    assert_eq!(routing.open_file_owner(&project_b), None);
    assert_eq!(routing.active_id(), Some(new_id));
    assert_eq!(routing.root_for_id(new_id), Some(&workspace));
}

#[test]
fn respects_workspace_folder_boundaries_during_discovery_and_spawn() {
    let project_folder = normalized_test_path("workspace/project_a");
    let vendor_folder = normalized_test_path("workspace/project_a/vendor");
    let project_file = normalized_test_path("workspace/project_a/src/lib.rs");
    let nested_file = normalized_test_path("workspace/project_a/vendor/member/src/lib.rs");
    let external_file = normalized_test_path("external/thin_vec/src/lib.rs");
    let external_root = normalized_test_path("external");
    let mut routing = EngineRouting::default();
    routing.set_workspace_folders([project_folder.clone(), vendor_folder.clone()]);

    assert_eq!(
        routing.discovery_workspace_for(&project_file),
        Some(&project_folder)
    );
    assert_eq!(
        routing.discovery_workspace_for(&nested_file),
        Some(&vendor_folder)
    );
    assert_eq!(routing.discovery_workspace_for(&external_file), None);

    let route = routing
        .route_workspace_root(project_folder.clone())
        .expect("root inside a configured workspace folder should be routed");
    let WorkspaceEngineRoute::Spawn { new_id, root } = route else {
        panic!("first route should reserve a new engine");
    };
    assert_eq!(new_id.index(), 0);
    assert_eq!(root, project_folder);
    assert_eq!(routing.route_workspace_root(external_root), None);
}
