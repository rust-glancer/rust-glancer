use rg_def_map::{CrateDataSource as _, CrateDependencies};
use rg_ir_model::{DefMapRef, FunctionRef, ImplRef, ItemStoreId, LocalDefRef, SemanticItemRef};
use rg_item_tree::testonly::fixture_database;
use rg_source::{CapturedSource, QueryContext, SourceInput};
use rg_workspace::testonly::WorkspaceFixture;

use crate::{CrateItemSource as _, ItemReader, ItemStoreQuery, TargetDeclarationReader};

#[ajika::query]
fn function_name(db: &QueryContext<'_>, function: FunctionRef) -> ajika::Result<String> {
    let declarations =
        TargetDeclarationReader::for_target(db, function.store.origin.origin_crate())?;
    let reader = ItemReader(&declarations);
    let data = ItemStoreQuery::new(&reader)
        .function_data(function)?
        .expect("fixture function exists");
    Ok(data.name.to_string())
}

#[ajika::query]
fn function_parameter_count(db: &QueryContext<'_>, function: FunctionRef) -> ajika::Result<usize> {
    let declarations =
        TargetDeclarationReader::for_target(db, function.store.origin.origin_crate())?;
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
    let mut database = fixture_database(workspace.metadata());
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

#[test]
fn changing_one_member_arena_preserves_other_item_identities() {
    let workspace = WorkspaceFixture::build(
        r#"
//- /src/lib.rs crate:independent_members
struct A;
struct B;
impl A { fn first() {} }
impl B { fn first() {} }
fn free() {}
"#,
    );
    let mut database = fixture_database(workspace.metadata());
    let targets = database
        .update_inputs(|update| CrateDependencies::write_inputs(workspace.metadata(), update))
        .expect("fixture targets");
    let target = targets[0];
    let read = |db: &QueryContext<'_>| -> ajika::Result<_> {
        let declarations = TargetDeclarationReader::for_target(db, target)?;
        let reader = ItemReader(&declarations);
        let items = ItemStoreQuery::new(&reader);
        let mut retained = Vec::new();
        for item in reader.all_items(DefMapRef::Crate(target))? {
            let SemanticItemRef::Function(function) = item.item() else {
                continue;
            };
            let keep = match function.store.id {
                ItemStoreId::Scope => true,
                ItemStoreId::Impl(id) => items
                    .impl_data(ImplRef {
                        origin: function.store.origin,
                        id,
                    })?
                    .is_some_and(|header| header.self_ty.to_string() == "B"),
                ItemStoreId::Trait(_) => false,
            };
            if keep {
                retained.push(function);
            }
        }
        Ok(retained)
    };
    let before = database.run(read).expect("initial identities");
    assert_eq!(before.len(), 2);

    // A may grow independently. Neither B's first method nor the free function uses A's
    // allocation sequence, even though all three originally had local FunctionId(0).
    let path = workspace.metadata().packages()[0]
        .root_dir()
        .join("src/lib.rs");
    let source = CapturedSource::new(
        &path,
        r#"
struct A;
struct B;
impl A { fn added() {} fn first() {} }
impl B { fn first() {} }
fn free() {}
"#,
    )
    .expect("changed source");
    let interner = database.interner();
    database
        .update_inputs(|update| SourceInput::set_saved(update, &interner, &source))
        .expect("publish changed members");
    assert_eq!(database.run(read).expect("updated identities"), before);
}
