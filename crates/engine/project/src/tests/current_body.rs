use rg_analysis::{CompletionItem, CompletionQuery, CompletionSource, SavedSourceRelationship};
use rg_body_ir::{CurrentBodyBuildCheckpoint, CurrentBodySelection, CurrentBodyUnavailable};
use rg_std::CancellationToken;
use test_fixture::testonly::MarkedText;

use crate::{
    CurrentBodyBuildSummary, Project, SplitIndexingMode,
    testonly::{ProjectFixture, ProjectSourceFixture},
};

const SAVED_FIXTURE: &str = r#"
//- /Cargo.toml
[package]
name = "current_body_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Saved {
    pub field: u8,
}

pub trait Factory {
    fn create() -> Self;
}

impl Factory for Saved {
    fn create() -> Self {
        Saved { field: 0 }
    }
}

impl Saved {
    pub fn saved_method(&self) {}
}

pub fn inspect() {
    let saved_local = Saved { field: 0 };
    saved_local.saved_method();
}
"#;

#[test]
fn early_start_current_body_completion_builds_request_local_item_lookup() {
    let source = ProjectSourceFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "current_body_early_start_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Saved;

impl Saved {
    pub fn associated() -> Self {
        Self
    }

    pub fn method(&self) {}
}

pub fn inspect() {}
"#,
    );
    let project = Project::builder(source.workspace_metadata())
        .split_indexing_mode(SplitIndexingMode::EarlyStart)
        .build()
        .expect("early-start project should build");
    assert_eq!(project.stats().body_ir.missing_crate_count, 1);

    let snapshot = project.snapshot();
    let targets = snapshot
        .file_contexts_for_path(source.path("src/lib.rs"))
        .expect("fixture source should have project contexts")
        .into_iter()
        .flat_map(|context| {
            context
                .crates
                .into_iter()
                .map(move |crate_ref| (crate_ref, context.file))
        })
        .collect::<Vec<_>>();
    let current = MarkedText::parse(
        r#"
pub struct Saved;

impl Saved {
    pub fn associated() -> Self {
        Self
    }

    pub fn method(&self) {}
}

pub fn inspect() {
    let saved = Saved;
    saved.met$method$;
    Saved::ass$associated$;
}
"#,
    );

    // The saved Body IR is still missing here. Current-body analysis must build only the cheap
    // crate-wide lookup it needs instead of waiting for or materializing every saved body.
    for (marker, expected) in [("method", "method"), ("associated", "associated")] {
        let offset = current
            .offset(marker)
            .try_into()
            .expect("completion marker should fit into u32");
        let completion_source = CompletionSource::new(current.text(), offset)
            .expect("current source should produce completion syntax");
        let (analysis, summary) = snapshot
            .analysis_for_current_bodies_at_offset(
                &targets,
                current.text(),
                offset,
                CancellationToken::new(),
                |_| Ok(()),
            )
            .expect("early-start current body should build");
        let labels = targets
            .iter()
            .flat_map(|&(crate_ref, file)| {
                analysis
                    .completions_at(
                        CompletionQuery::new(crate_ref, file, offset)
                            .with_completion_source(&completion_source),
                    )
                    .expect("early-start completion should resolve")
            })
            .map(|item| item.label)
            .collect::<Vec<_>>();

        assert!(
            labels.iter().any(|label| label == expected),
            "completion at {marker} should contain {expected}, got {labels:?}",
        );
        assert!(
            summary.is_complete(),
            "early-start completion should build every selected current body: {summary:?}",
        );
    }

    assert_eq!(
        project.stats().body_ir.missing_crate_count,
        1,
        "request-local lookup construction must not materialize saved Body IR",
    );
}

#[test]
fn current_body_combines_current_locals_with_saved_global_semantics() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);
    let current = MarkedText::parse(
        r#"
pub struct UnsavedShift;

pub fn inspect() {
    use crate::Saved as Alias;
    fn body_helper() {}

    let outer = Saved { field: 1 };
    let closure = |outer: Saved| {
        let (current,) = (outer,);
        current.saved_$method$;
    };
    closure(outer);

    let imported = Alias { field: 2 };
    imported.fi$field$;
    Alias::cre$associated$;
    body_hel$local_item$;
}
"#,
    );

    for (marker, expected) in [
        ("method", "saved_method"),
        ("field", "field"),
        ("associated", "create"),
        ("local_item", "body_helper"),
    ] {
        let (labels, summary) = fixture.completion_labels(&current, marker);
        assert!(
            labels.iter().any(|label| label == expected),
            "completion at {marker} should contain {expected}, got {labels:?}",
        );
        assert!(
            summary.is_complete(),
            "completion at {marker} should have a complete current-body build: {summary:?}",
        );
    }
}

#[test]
fn current_body_builds_new_nested_bodies_and_parent_local_impls() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "current_nested_body_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn inspect() {}
"#,
    );
    let current = MarkedText::parse(
        r#"
pub fn inspect() {
    struct Local;

    impl Local {
        fn current_method(&self) {}
    }

    fn newly_typed() {
        let local = Local;
        local.current_$cursor$;
    }
}
"#,
    );

    let (labels, summary) = fixture.completion_labels(&current, "cursor");

    assert!(
        labels.iter().any(|label| label == "current_method"),
        "a new nested body should see the impl collected from its current parent: {labels:?}",
    );
    assert!(
        summary.is_complete(),
        "the associated root worklist should own the new nested body: {summary:?}",
    );
}

#[test]
fn current_body_keeps_an_unfinished_record_literal_inside_its_saved_owner() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);
    let current = MarkedText::parse(
        r#"pub fn inspect() {
    let _ = Saved { $cursor$"#,
    );

    let (labels, summary) = fixture.completion_labels(&current, "cursor");

    assert!(
        labels.iter().any(|label| label == "field"),
        "saved record fields should remain available at an unfinished current literal: {labels:?}",
    );
    assert!(
        summary.is_complete(),
        "the trailing incomplete literal should still belong to inspect: {summary:?}",
    );
}

#[test]
fn current_body_association_uses_the_full_saved_owner_path() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "current_body_owner_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod left {
    pub struct Item;

    impl Item {
        pub fn left_only(&self) {}
        pub fn inspect() {}
    }
}

pub mod right {
    pub struct Item;

    impl Item {
        pub fn right_only(&self) {}
        pub fn inspect() {}
    }
}
"#,
    );
    let current = MarkedText::parse(
        r#"
// Moving the body and changing everything inside it must not change its saved identity.
pub mod left {
    pub struct Item;

    impl Item {
        pub fn inspect() {
            let value = Item;
            value.le$owner$;
        }
    }
}
"#,
    );

    let (labels, summary) = fixture.completion_labels(&current, "owner");

    assert!(
        summary.is_complete(),
        "left::Item::inspect should associate"
    );
    assert!(labels.iter().any(|label| label == "left_only"));
    assert!(!labels.iter().any(|label| label == "right_only"));
}

#[test]
fn current_body_builds_request_local_function_roots_and_rejects_ambiguous_owners() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);
    let changed_signature = MarkedText::parse(
        r#"
pub fn inspect(value: Saved) {
    value.saved_$cursor$;
    insp$recursive$;
}
"#,
    );
    let (labels, summary) = fixture.completion_labels(&changed_signature, "cursor");
    assert!(
        summary.is_complete(),
        "a changed function signature should receive a request-local semantic root: {summary:?}",
    );
    assert!(
        labels.iter().any(|label| label == "saved_method"),
        "the changed parameter type should be available inside the current body: {labels:?}",
    );
    let (items, summary) = fixture.completion_items(&changed_signature, "recursive");
    assert!(summary.is_complete());
    let recursive = items
        .iter()
        .filter(|item| item.label == "inspect")
        .collect::<Vec<_>>();
    assert_eq!(
        recursive.len(),
        1,
        "the current free function should replace its saved declaration: {items:?}",
    );
    assert!(
        recursive[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("value: Saved")),
        "recursive lookup should use the current function signature: {recursive:?}",
    );

    let new_owner = MarkedText::parse(
        r#"
pub fn newly_added(value: Saved) {
    value.saved_$cursor$;
    newly_$recursive$;
}
"#,
    );
    let (labels, summary) = fixture.completion_labels(&new_owner, "cursor");
    assert!(
        summary.is_complete(),
        "a new function should receive a request-local semantic root: {summary:?}",
    );
    assert!(
        labels.iter().any(|label| label == "saved_method"),
        "the new function should combine its current parameter with saved item facts: {labels:?}",
    );
    let (items, summary) = fixture.completion_items(&new_owner, "recursive");
    assert!(summary.is_complete());
    let recursive = items
        .iter()
        .filter(|item| item.label == "newly_added")
        .collect::<Vec<_>>();
    assert_eq!(
        recursive.len(),
        1,
        "a new free function should be visible recursively only from its own body: {items:?}",
    );

    let ambiguous = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "ambiguous_current_body_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn inspect() {}
pub fn inspect() {}
"#,
    );
    let one_current_owner = MarkedText::parse(
        r#"
pub fn inspect() {
    let value = 1_u8;
    value.co$cursor$;
}
"#,
    );
    assert_eq!(
        ambiguous
            .build_summary(&one_current_owner, "cursor")
            .unavailable(),
        &[(
            ambiguous.crate_ref(),
            CurrentBodyUnavailable::AmbiguousSavedOwner,
        )],
    );
}

#[test]
fn current_body_builds_const_static_and_trait_owned_roots() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);
    let current = MarkedText::parse(
        r#"
const CURRENT_CONST: usize = {
    let value = Saved { field: 0 };
    value.saved_$const_member$;
    0
};

static CURRENT_STATIC: usize = {
    let value = Saved { field: 0 };
    value.saved_$static_member$;
    0
};

trait CurrentTrait<CurrentType> {
    fn current_default(value: CurrentType) {
        let _: CurrentT$trait_function_generic$ = value;
    }

    const CURRENT_ASSOCIATED: usize = {
        let _: CurrentT$trait_const_generic$ = todo!();
        0
    };
}
"#,
    );

    for (marker, expected) in [
        ("const_member", "saved_method"),
        ("static_member", "saved_method"),
        ("trait_function_generic", "CurrentType"),
        ("trait_const_generic", "CurrentType"),
    ] {
        let (labels, summary) = fixture.completion_labels(&current, marker);
        assert!(
            summary.is_complete(),
            "{marker} should have a request-local semantic root: {summary:?}",
        );
        assert!(
            labels.iter().any(|label| label == expected),
            "completion at {marker} should contain {expected}, got {labels:?}",
        );
    }
}

#[test]
fn current_declaration_headers_use_request_local_semantics() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);

    for (current, marker, expected) in [
        (
            MarkedText::parse("pub fn current<CurrentGeneric>(_: CurrentGen$cursor$) {}"),
            "cursor",
            "CurrentGeneric",
        ),
        (
            MarkedText::parse("const CURRENT: Sav$cursor$ = Saved { field: 0 };"),
            "cursor",
            "Saved",
        ),
        (
            MarkedText::parse("static CURRENT: Sav$cursor$ = Saved { field: 0 };"),
            "cursor",
            "Saved",
        ),
        (
            MarkedText::parse("impl<CurrentGeneric> Factory for CurrentGen$cursor$ {}"),
            "cursor",
            "CurrentGeneric",
        ),
        (
            MarkedText::parse("impl Factory for Sav$cursor$ {}"),
            "cursor",
            "Saved",
        ),
    ] {
        let (labels, _) = fixture.completion_labels(&current, marker);
        assert!(
            labels.iter().any(|label| label == expected),
            "current header completion should contain {expected}, got {labels:?}",
        );
    }

    let current = MarkedText::parse("impl Factory for Sa$cursor$ved {}");
    let offset = current
        .offset("cursor")
        .try_into()
        .expect("header marker should fit into u32");
    let snapshot = fixture.fixture.project().snapshot();
    let targets = fixture.targets();
    let (analysis, _) = snapshot
        .analysis_for_current_bodies_at_offset(
            &targets,
            current.text(),
            offset,
            CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("current impl header should build");
    for (crate_ref, file) in targets {
        let hover = analysis
            .hover(crate_ref, file, offset)
            .expect("current impl header hover should resolve")
            .expect("Saved should have hover information");
        assert!(
            hover.blocks.iter().any(|block| block
                .path
                .as_deref()
                .is_some_and(|path| path.ends_with("Saved"))),
            "hover should resolve the current path to Saved: {hover:?}",
        );
        assert!(
            analysis
                .type_at(crate_ref, file, offset)
                .expect("current impl header type should resolve")
                .is_some(),
            "type lookup should resolve the current path to Saved",
        );
        let targets = analysis
            .goto_definition(crate_ref, file, offset)
            .expect("current impl header navigation should resolve");
        assert!(
            targets.iter().any(|target| target.name == "Saved"),
            "navigation should resolve the current path to Saved: {targets:?}",
        );
    }
}

#[test]
fn request_local_method_root_uses_current_impl_without_publishing_current_items() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "current_method_root_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Service {
    pub value: u32,
}

impl Service {
    pub fn new() -> Self {
        Self { value: 0 }
    }

    pub fn existing_static() {}

    /// saved const
    pub const EXISTING_CONST: u32 = 0;

    /// saved type
    pub type ExistingType = u32;

    pub fn existing_member(&self) {}
}
"#,
    );

    for (root_name, current) in [
        (
            "existing_member",
            MarkedText::parse(
                r#"
pub struct Service {
    pub value: u32,
}

impl Service {
    pub fn new() -> Self {
        Self { value: 0 }
    }

    pub fn existing_static(current: bool) {}

    pub fn sibling_added(&self) {
        $sibling_body$
    }

    pub fn existing_member(&self, other: Service) {
        let _ = Self::$associated$;
        self.$member$;
        self.sibling_$current_sibling$;
        other.existing_$parameter$;
        fn nested() {
            let _ = Service::$nested_associated$;
        }
        nested();
    }
}
"#,
            ),
        ),
        (
            "added_member",
            MarkedText::parse(
                r#"
pub struct Service {
    pub value: u32,
}

impl Service {
    pub fn new() -> Self {
        Self { value: 0 }
    }

    pub fn existing_static(current: bool) {}

    pub fn existing_member(&self) {}

    pub fn sibling_added(&self) {
        $sibling_body$
    }

    pub fn added_member(&self, other: Service) {
        let _ = Self::$associated$;
        self.$member$;
        self.sibling_$current_sibling$;
        other.existing_$parameter$;
        fn nested() {
            let _ = Service::$nested_associated$;
        }
        nested();
    }
}
"#,
            ),
        ),
    ] {
        let (associated, summary) = fixture.completion_items(&current, "associated");
        assert!(
            summary.is_complete(),
            "{root_name} should receive exact request-local method semantics: {summary:?}",
        );
        let current_rows = associated
            .iter()
            .filter(|item| item.label == root_name)
            .collect::<Vec<_>>();
        assert_eq!(
            current_rows.len(),
            1,
            "the current method should replace, not accompany, its saved declaration: {associated:?}",
        );
        assert!(
            current_rows[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("other: Service")),
            "the request-local method should keep its current signature: {current_rows:?}",
        );
        assert!(
            associated.iter().any(|item| item.label == "new"),
            "the request-local root should retain saved associated item new: {associated:?}",
        );
        let current_sibling_rows = associated
            .iter()
            .filter(|item| item.label == "existing_static")
            .collect::<Vec<_>>();
        assert_eq!(
            current_sibling_rows.len(),
            1,
            "a current sibling should replace its saved declaration: {associated:?}",
        );
        assert!(
            current_sibling_rows[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("current: bool")),
            "the sibling candidate should use its current signature: {current_sibling_rows:?}",
        );

        let (members, summary) = fixture.completion_items(&current, "member");
        assert!(summary.is_complete());
        for member_name in [root_name, "value"] {
            assert!(
                members.iter().any(|item| item.label == member_name),
                "the current receiver should expose {member_name}: {members:?}",
            );
        }

        let (siblings, summary) = fixture.completion_items(&current, "current_sibling");
        assert!(summary.is_complete());
        assert!(
            siblings.iter().any(|item| item.label == "sibling_added"),
            "a request-local method should see a new sibling from its current impl: {siblings:?}",
        );
        let sibling_body = current
            .offset("sibling_body")
            .try_into()
            .expect("sibling body marker should fit into u32");
        assert!(
            summary
                .rebuilt_body_spans()
                .iter()
                .all(|(_, _, span)| !span.contains(sibling_body)),
            "the sibling signature should not add its body to the selected worklist: {summary:?}",
        );

        let (parameters, summary) = fixture.completion_items(&current, "parameter");
        assert!(summary.is_complete());
        assert!(
            parameters
                .iter()
                .any(|item| item.label == "existing_member"),
            "the current parameter type should drive method lookup: {parameters:?}",
        );

        let (nested, summary) = fixture.completion_items(&current, "nested_associated");
        assert!(summary.is_complete());
        let current_rows = nested
            .iter()
            .filter(|item| item.label == root_name)
            .collect::<Vec<_>>();
        assert_eq!(
            current_rows.len(),
            1,
            "a nested body should inherit the current root without reviving its saved declaration: {nested:?}",
        );
        assert!(
            current_rows[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("other: Service")),
            "nested lookup should still use the request-local method signature: {current_rows:?}",
        );
    }

    let current = MarkedText::parse(
        r#"
pub struct Service {
    pub value: u32,
}

pub struct DirtyType;

impl Service {
    pub fn new() -> Self {
        Self { value: 0 }
    }

    pub fn existing_static() {}

    /// current const
    pub const EXISTING_CONST: bool = true;

    /// current type
    pub type ExistingType = bool;

    pub fn added_member(&self) {
        $dirty_method_body$
    }

    pub const ADDED_CONST: u32 = {
        $dirty_const_body$
        1
    };

    pub type AddedType = u32;

    pub fn existing_member(&self) {
        self.add$dirty_method$;
        let _ = Self::ADDED_$dirty_const$;
        let _ = Self::EXISTING_$dirty_existing_const$;
        let _: Self::Added$dirty_type_alias$ = 0;
        let _: Self::Existing$dirty_existing_type_alias$ = false;
        let _: Dir$dirty_type$ = todo!();
    }
}

pub fn outside(service: Service) {
    service.add$outside_method$;
}
"#,
    );
    let (labels, summary) = fixture.completion_labels(&current, "dirty_method");
    assert!(summary.is_complete());
    assert!(
        labels.iter().any(|label| label == "added_member"),
        "a saved method should see a new sibling from its current impl: {labels:?}",
    );
    for sibling_body in ["dirty_method_body", "dirty_const_body"] {
        let sibling_body = current
            .offset(sibling_body)
            .try_into()
            .expect("sibling body marker should fit into u32");
        assert!(
            summary
                .rebuilt_body_spans()
                .iter()
                .all(|(_, _, span)| !span.contains(sibling_body)),
            "current impl siblings should contribute signatures without rebuilding bodies: {summary:?}",
        );
    }
    for (marker, expected) in [
        ("dirty_const", "ADDED_CONST"),
        ("dirty_type_alias", "AddedType"),
    ] {
        let (labels, summary) = fixture.completion_labels(&current, marker);
        assert!(summary.is_complete());
        assert!(
            labels.iter().any(|label| label == expected),
            "a saved method should see current impl member {expected}: {labels:?}",
        );
    }
    for (marker, expected, current_docs) in [
        ("dirty_existing_const", "EXISTING_CONST", "current const"),
        ("dirty_existing_type_alias", "ExistingType", "current type"),
    ] {
        let (items, summary) = fixture.completion_items(&current, marker);
        assert!(summary.is_complete());
        let rows = items
            .iter()
            .filter(|item| item.label == expected)
            .collect::<Vec<_>>();
        assert_eq!(
            rows.len(),
            1,
            "a current impl item should replace its saved same-name declaration: {items:?}",
        );
        assert_eq!(
            rows[0].documentation.as_deref(),
            Some(current_docs),
            "the remaining candidate should come from current source",
        );
    }
    let (labels, summary) = fixture.completion_labels(&current, "dirty_type");
    assert!(summary.is_complete());
    assert!(
        !labels.iter().any(|label| label == "DirtyType"),
        "a new type must not enter saved module lookup before save: {labels:?}",
    );
    let (labels, summary) = fixture.completion_labels(&current, "outside_method");
    assert!(summary.is_complete());
    assert!(
        !labels.iter().any(|label| label == "added_member"),
        "a current impl member must not enter lookup from an unrelated body: {labels:?}",
    );
}

#[test]
fn current_module_syntax_uses_saved_global_names_without_saved_offsets() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "current_module_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct RootSaved;

pub mod nested {
    pub struct NestedSaved;
}
"#,
    );
    let root = MarkedText::parse("impl RootS$cursor$");
    let nested = MarkedText::parse(
        r#"
pub mod nested {
    impl NestedS$cursor$
}
"#,
    );

    for (current, expected) in [(root, "RootSaved"), (nested, "NestedSaved")] {
        let (labels, summary) = fixture.completion_labels(&current, "cursor");
        assert_eq!(
            summary.unavailable(),
            &[(
                fixture.crate_ref(),
                CurrentBodyUnavailable::NoBodyAtPosition,
            )],
        );
        assert!(
            labels.iter().any(|label| label == expected),
            "module completion should contain {expected}, got {labels:?}",
        );
    }
}

#[test]
fn current_body_stops_at_each_reported_cancellation_boundary() {
    let fixture = CurrentBodyFixture::new(SAVED_FIXTURE);
    let current = MarkedText::parse(
        r#"
pub fn inspect() {
    let value = Saved { field: 1 };
    value.saved_$cursor$;
}
"#,
    );
    let snapshot = fixture.fixture.project().snapshot();
    let targets = fixture.targets();
    let offset = current
        .offset("cursor")
        .try_into()
        .expect("current-body marker should fit into u32");
    let checkpoints = [
        CurrentBodyBuildCheckpoint::SourceParsed,
        CurrentBodyBuildCheckpoint::OwnerAssociated,
        CurrentBodyBuildCheckpoint::BodyLowered,
        CurrentBodyBuildCheckpoint::BodyLocalItemsCollected,
        CurrentBodyBuildCheckpoint::ImplHeadersResolved,
        CurrentBodyBuildCheckpoint::PatternBindingsMaterialized,
        CurrentBodyBuildCheckpoint::BodyResolved,
    ];

    for (index, stop_at) in checkpoints.into_iter().enumerate() {
        let mut visited = Vec::new();
        let error = match snapshot.analysis_for_current_bodies_at_offset(
            &targets,
            current.text(),
            offset,
            CancellationToken::new(),
            |checkpoint| {
                visited.push(checkpoint);
                if checkpoint == stop_at {
                    anyhow::bail!("test cancellation")
                }
                Ok(())
            },
        ) {
            Ok(_) => panic!("cancellation at {stop_at} should stop current-body construction"),
            Err(error) => error,
        };

        assert!(format!("{error:#}").contains("test cancellation"));
        assert_eq!(
            visited,
            checkpoints[..=index],
            "cancellation at {stop_at} should skip every later stage",
        );
    }
}

#[test]
fn exact_current_source_preserves_unselected_saved_bodies() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "exact_current_body_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Saved;

pub fn selected() {
    let selected = 1_u8;
}

pub fn unselected() {
    let retained = Saved;
}
"#,
    );
    let snapshot = fixture.fixture.project().snapshot();
    let targets = fixture.targets();
    let &(first_crate, first_file) = targets
        .first()
        .expect("exact current-source fixture should have a target");
    let current = snapshot
        .file_source_text(first_crate.package, first_file)
        .expect("saved fixture source should load")
        .expect("saved fixture source should exist");
    let offset = current
        .find("let selected")
        .expect("selected body should exist in saved fixture")
        .try_into()
        .expect("selected body offset should fit into u32");
    let source = snapshot
        .prepare_current_source(&targets, &current)
        .expect("exact current source should prepare");
    for &(crate_ref, file) in &targets {
        assert_eq!(
            source.relationship(crate_ref.package, file),
            Some(SavedSourceRelationship::Exact),
        );
    }
    let (analysis, summary) = snapshot
        .analysis_for_current_bodies_from_source(
            &targets,
            source,
            CurrentBodySelection::AtOffset(offset),
            CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("selected exact body should build");

    assert!(summary.is_complete());
    for (crate_ref, file) in targets {
        let hints = analysis
            .inlay_hints(crate_ref, file, None)
            .expect("exact analysis should retain hints from unselected saved bodies");
        assert!(
            hints.iter().any(|hint| hint.label == ": Saved"),
            "the unselected saved body should remain visible: {hints:?}",
        );
    }
}

#[test]
fn current_body_range_analyzes_only_intersecting_owners() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "current_body_range_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Value;

pub fn make_value() -> Value {
    Value
}

pub fn first() {
    let saved = make_value();
}

pub fn second() {
    let saved = make_value();
}
"#,
    );
    let current = MarkedText::parse(
        r#"
pub struct Value;

pub fn make_value() -> Value {
    Value
}

pub fn first() {
    $range_start$let current = make_value();$range_end$
}

// This signature no longer matches the saved owner. It must not affect a range that only
// intersects `first`.
pub fn second(_changed: usize) {
    let unrelated = make_value();
}

// These declarations do not own bodies. They must not affect analysis of the body selected above.
pub trait CurrentOnly {
    fn required(&self);
}

fn unfinished
"#,
    );
    let range = rg_parse::TextSpan {
        start: current
            .offset("range_start")
            .try_into()
            .expect("range start should fit into u32"),
        end: current
            .offset("range_end")
            .try_into()
            .expect("range end should fit into u32"),
    };
    let snapshot = fixture.fixture.project().snapshot();
    let targets = fixture.targets();
    let source = snapshot
        .prepare_current_source(&targets, current.text())
        .expect("current range source should prepare");
    let (analysis, summary) = snapshot
        .analysis_for_current_bodies_from_source(
            &targets,
            source,
            CurrentBodySelection::IntersectingRange(range),
            CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("current bodies in the requested range should build");

    assert!(
        summary.is_complete(),
        "an unrelated changed owner should not make the requested range incomplete: {summary:?}",
    );
    assert_eq!(summary.rebuilt_body_spans().len(), 1);
    let [(crate_ref, file)] = targets.as_slice() else {
        panic!("fixture should have exactly one crate interpretation");
    };
    let hints = analysis
        .inlay_hints(*crate_ref, *file, Some(range))
        .expect("current body inlay hints should resolve");
    assert!(
        hints.iter().any(|hint| hint.label == ": Value"),
        "the selected current body should contribute its inferred local type: {hints:?}",
    );
}

#[test]
fn current_body_range_does_not_reuse_one_saved_identity_for_duplicate_roots() {
    let fixture = CurrentBodyFixture::new(
        r#"
//- /Cargo.toml
[package]
name = "duplicate_current_body_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn inspect() {}
"#,
    );
    let current = r#"
pub fn inspect() {
    let first = true;
}

pub fn inspect() {
    let second = true;
}
"#;
    let range = rg_parse::TextSpan {
        start: 0,
        end: current
            .len()
            .try_into()
            .expect("current source length should fit into u32"),
    };
    let snapshot = fixture.fixture.project().snapshot();
    let targets = fixture.targets();
    let source = snapshot
        .prepare_current_source(&targets, current)
        .expect("current range source should prepare");
    let (analysis, summary) = snapshot
        .analysis_for_current_bodies_from_source(
            &targets,
            source,
            CurrentBodySelection::IntersectingRange(range),
            CancellationToken::new(),
            |_| Ok(()),
        )
        .expect("ambiguous current roots should fail closed without breaking the request");

    assert!(summary.rebuilt_body_spans().is_empty());
    assert_eq!(summary.unavailable().len(), 2);
    assert!(
        summary
            .unavailable()
            .iter()
            .all(|(_, reason)| *reason == CurrentBodyUnavailable::AmbiguousSavedOwner),
    );
    drop(analysis);
}

struct CurrentBodyFixture {
    fixture: ProjectFixture,
}

impl CurrentBodyFixture {
    fn new(spec: &str) -> Self {
        Self {
            fixture: ProjectFixture::build(spec),
        }
    }

    fn completion_labels(
        &self,
        current: &MarkedText,
        marker: &str,
    ) -> (Vec<String>, CurrentBodyBuildSummary) {
        let (items, summary) = self.completion_items(current, marker);
        let mut labels = items.into_iter().map(|item| item.label).collect::<Vec<_>>();
        labels.sort();
        labels.dedup();
        (labels, summary)
    }

    fn completion_items(
        &self,
        current: &MarkedText,
        marker: &str,
    ) -> (Vec<CompletionItem>, CurrentBodyBuildSummary) {
        let snapshot = self.fixture.project().snapshot();
        let before = self.fixture.project().stats();
        let targets = self.targets();
        let offset = current
            .offset(marker)
            .try_into()
            .expect("current-body marker should fit into u32");
        let completion_source = CompletionSource::new(current.text(), offset)
            .expect("current body should produce completion syntax");
        let (analysis, summary) = snapshot
            .analysis_for_current_bodies_at_offset(
                &targets,
                current.text(),
                offset,
                CancellationToken::new(),
                |_| Ok(()),
            )
            .expect("current body should build against the saved fixture");
        let mut items = Vec::new();

        for (crate_ref, file) in targets {
            let query = CompletionQuery::new(crate_ref, file, offset)
                .with_completion_source(&completion_source);
            items.extend(
                analysis
                    .completions_at(query)
                    .expect("current-body completion should resolve"),
            );
        }
        drop(analysis);
        assert_eq!(
            self.fixture.project().stats(),
            before,
            "request-local current-body analysis must not change saved project stores",
        );
        (items, summary)
    }

    fn build_summary(&self, current: &MarkedText, marker: &str) -> CurrentBodyBuildSummary {
        let snapshot = self.fixture.project().snapshot();
        let offset = current
            .offset(marker)
            .try_into()
            .expect("current-body marker should fit into u32");
        let before = self.fixture.project().stats();
        let (analysis, summary) = snapshot
            .analysis_for_current_bodies_at_offset(
                &self.targets(),
                current.text(),
                offset,
                CancellationToken::new(),
                |_| Ok(()),
            )
            .expect("current-body build summary should be returned");
        drop(analysis);
        assert_eq!(
            self.fixture.project().stats(),
            before,
            "request-local current-body analysis must not change saved project stores",
        );
        summary
    }

    fn targets(&self) -> Vec<(rg_ir_model::CrateRef, rg_parse::FileId)> {
        let snapshot = self.fixture.project().snapshot();
        snapshot
            .file_contexts_for_path(self.fixture.path("src/lib.rs"))
            .expect("fixture source should resolve")
            .into_iter()
            .flat_map(|context| {
                context
                    .crates
                    .into_iter()
                    .map(move |crate_ref| (crate_ref, context.file))
            })
            .collect()
    }

    fn crate_ref(&self) -> rg_ir_model::CrateRef {
        let targets = self.targets();
        let [target] = targets.as_slice() else {
            panic!("fixture should have exactly one crate interpretation");
        };
        target.0
    }
}
