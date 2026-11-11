use rg_def_map::{CrateDataSource as _, CrateDependencies};
use rg_ir_model::{DefMapRef, FunctionRef, LocalDefRef, SemanticItemRef};
use rg_source::{CapturedSource, QueryContext, SourceInput};
use rg_workspace::testonly::WorkspaceFixture;

use crate::{CrateItemSource as _, ItemReader, ItemStoreQuery, TargetDeclarationReader};

#[ajika::query]
fn function_name(db: &QueryContext<'_>, function: FunctionRef) -> ajika::Result<String> {
    let declarations = TargetDeclarationReader::for_target(db, function.origin.origin_crate())?;
    let reader = ItemReader(&declarations);
    let data = ItemStoreQuery::new(&reader)
        .function_data(function)?
        .expect("fixture function exists");
    Ok(data.name.to_string())
}

#[ajika::query]
fn function_parameter_count(db: &QueryContext<'_>, function: FunctionRef) -> ajika::Result<usize> {
    let declarations = TargetDeclarationReader::for_target(db, function.origin.origin_crate())?;
    let reader = ItemReader(&declarations);
    let data = ItemStoreQuery::new(&reader)
        .function_data(function)?
        .expect("fixture function exists");
    Ok(data.signature.params().len())
}

#[ajika::query]
fn definition_name(
    db: &QueryContext<'_>,
    definition: LocalDefRef,
) -> ajika::Result<Option<String>> {
    let declarations = TargetDeclarationReader::for_target(db, definition.origin.origin_crate())?;
    let reader = ItemReader(&declarations);
    let items = ItemStoreQuery::new(&reader);
    let Some(item) = items.semantic_item_for_local_def(definition)? else {
        return Ok(None);
    };
    Ok(items
        .semantic_item_view(item)?
        .and_then(|item| item.name().map(ToString::to_string)))
}

#[test]
fn item_consumers_observe_saved_declaration_updates() {
    let workspace = WorkspaceFixture::build(
        r#"
//- /src/lib.rs crate:app
pub fn before() {}
macro_rules! Added { () => {} }
"#,
    );
    let mut database = rg_item_tree::testonly::fixture_database(workspace.metadata());
    let targets = database
        .update_inputs(|update| CrateDependencies::write_inputs(workspace.metadata(), update))
        .expect("fixture targets should install");
    let target = targets[0];
    let (function, definition) = database
        .run(|db| {
            let declarations = TargetDeclarationReader::for_target(db, target)?;
            let function = declarations
                .all_items(DefMapRef::Crate(target))?
                .into_iter()
                .find_map(|item| match item.item() {
                    SemanticItemRef::Function(function) => Some(function),
                    _ => None,
                })
                .expect("fixture function exists");
            let data = declarations
                .crate_data(target)?
                .expect("fixture target exists");
            let definition = data
                .def_map()
                .local_def_refs()
                .find(|definition| {
                    data.def_map()
                        .local_def(definition.local_def)
                        .is_some_and(|data| data.name.as_str() == "Added")
                })
                .expect("fixture macro definition exists");
            Ok((function, definition))
        })
        .expect("fixture identities should load");

    let path = workspace.metadata().packages()[0]
        .root_dir()
        .join("src/lib.rs");
    let interner = database.interner();

    // The macro has a local definition but no semantic item. Replacing it with a struct also
    // checks that an absent mapping is tracked, alongside two independent reads of the function.
    for (source, expected_name, expected_parameters, expected_item) in [
        (
            "pub fn before() {}\nmacro_rules! Added { () => {} }",
            "before",
            0,
            None,
        ),
        (
            "pub fn after(value: u8) {}\npub struct Added;",
            "after",
            1,
            Some("Added"),
        ),
    ] {
        let source = CapturedSource::new(&path, source).expect("fixture source should capture");
        database
            .update_inputs(|update| SourceInput::set_saved(update, &interner, &source))
            .expect("fixture source should update");
        let actual = database
            .run(|db| {
                Ok((
                    function_name(db, function)?.as_ref().clone(),
                    *function_parameter_count(db, function)?,
                    definition_name(db, definition)?.as_ref().clone(),
                ))
            })
            .expect("item consumers should run");
        assert_eq!(
            actual,
            (
                expected_name.to_owned(),
                expected_parameters,
                expected_item.map(str::to_owned)
            )
        );
    }
}
