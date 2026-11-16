use std::fs;

use rg_analysis::{CompletionQuery, ReferenceQuery, SavedSourceRelationship, SymbolKind};
use rg_body_ir::{BodyReader, CurrentSourceSelection};
use rg_ir_model::{BodyId, BodyKey, DefMapRef, PrimitiveTy, SemanticItemRef, UnsignedIntTy};
use rg_semantic_ir::{ItemReader, complete_source_ownership};
use rg_source::CapturedSource;
use rg_std::CancellationToken;

use crate::{
    AnalysisChangeSummary, DocumentSourceView, Project, SavedFileChange,
    testonly::ProjectSourceFixture,
};

#[test]
fn file_local_requests_preserve_lazy_member_capture() {
    for request in ["completion", "outline"] {
        let fixture = ProjectSourceFixture::build_with_fake_sysroot(
            r#"
//- /src/lib.rs crate:lazy_file_queries
mod other;
trait Service { fn required(&self); }
struct Selected;
impl Service for Selected { $completion$ }
impl Selected { fn visible() {} }

//- /src/other.rs
pub struct Late;
impl Late { include!("methods.rs"); }
fn use_late() { let value = Late::wanted(); let _ = $result$value; }

//- /src/methods.rs
pub fn wanted() -> u8 { 1u8 }
"#,
        );
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route selected file")
                .expect("target");
            let analysis = snapshot.analysis();
            match request {
                "completion" => {
                    let text =
                        fs::read_to_string(fixture.path("src/lib.rs")).expect("read selected file");
                    let completions = analysis
                        .completions_at(
                            CompletionQuery::new(
                                context.crate_ref,
                                context.file,
                                fixture.markers().position("completion").offset,
                            )
                            .with_source_text(&text),
                        )
                        .expect("saved impl completion");
                    assert!(completions.iter().any(|item| item.label == "required"));
                }
                "outline" => {
                    let outline = analysis
                        .document_symbols(context.crate_ref, context.file)
                        .expect("selected file outline");
                    assert!(outline.symbols.iter().any(|item| {
                        item.name == "impl Selected"
                            && item.children.iter().any(|child| child.name == "visible")
                    }));
                }
                _ => unreachable!("request comes from the test vector"),
            }
        });

        // The request above needs no members from other.rs. Its include must still receive its
        // first observation when we ask about Late, using the contents written here.
        fs::write(
            fixture.path("src/methods.rs"),
            "pub fn wanted() -> u16 { 1u16 }\n",
        )
        .expect("write first include observation");
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/other.rs"))
                .expect("route unrelated file")
                .expect("target");
            assert_eq!(
                snapshot
                    .analysis()
                    .type_at(
                        context.crate_ref,
                        context.file,
                        fixture.markers().position("result").offset,
                    )
                    .expect("included signature")
                    .expect("binding type")
                    .primitive(),
                Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)),
                "{request} must leave unrelated members lazy"
            );
        });
    }
}

#[test]
fn selected_body_leaves_other_owners_lazy_and_include_survives_restart() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:lazy_include
pub struct Selected;
impl Selected {
    pub fn first() -> u8 { let value = 1u8; let _ = $selected$value; value }
}
pub struct Late;
impl Late { include!("methods.rs"); }
fn use_late() { let value = Late::$call$wanted(); let _ = $result$value; }

//- /src/methods.rs
pub fn wanted() -> u8 { 1u8 }
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("target");
        assert_eq!(
            snapshot
                .analysis()
                .type_at(
                    context.crate_ref,
                    context.file,
                    fixture.markers().position("selected").offset
                )
                .expect("selected body")
                .expect("binding type")
                .primitive(),
            Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8))
        );
    });

    // Neither startup nor a request inside Selected should have captured Late's include yet.
    fs::write(
        fixture.path("src/methods.rs"),
        "pub fn wanted() -> u16 { let value = 7u16; value }\n",
    )
    .expect("write first include observation");
    let check = |project: &Project| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("target");
            let analysis = snapshot.analysis();
            assert_eq!(
                analysis
                    .type_at(
                        context.crate_ref,
                        context.file,
                        fixture.markers().position("result").offset
                    )
                    .expect("included signature")
                    .expect("binding type")
                    .primitive(),
                Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16))
            );
            let definitions = analysis
                .goto_definition(
                    context.crate_ref,
                    context.file,
                    fixture.markers().position("call").offset,
                )
                .expect("included navigation");
            assert_eq!(definitions.len(), 1);
            assert_eq!(
                snapshot
                    .file_path(context.crate_ref.package, definitions[0].file_id)
                    .expect("source path query")
                    .expect("included path"),
                fixture
                    .path("src/methods.rs")
                    .canonicalize()
                    .expect("canonical include")
            );
            let span = definitions[0].span.expect("included name span");
            assert_eq!(
                snapshot
                    .file_text_for_span(context.crate_ref.package, definitions[0].file_id, span)
                    .expect("included source text")
                    .as_deref(),
                Some("wanted")
            );
            let position = snapshot
                .file_line_index(context.crate_ref.package, definitions[0].file_id)
                .expect("included line index")
                .expect("line index")
                .utf16_position(span.start);
            assert_eq!((position.line, position.column), (0, 7));
            let reader = ItemReader(snapshot.declarations());
            let items = reader
                .all_items(DefMapRef::Crate(context.crate_ref))
                .expect("declarations");
            let function = items
                .iter()
                .find_map(|item| match item.item() {
                    SemanticItemRef::Function(function)
                        if item.name().is_some_and(|name| name.as_str() == "wanted") =>
                    {
                        Some(function)
                    }
                    _ => None,
                })
                .expect("included function");
            let bodies = BodyReader::new(snapshot.query_context(), snapshot.declarations());
            let key = BodyKey {
                krate: context.crate_ref,
                owner: function.into(),
            };
            let body = bodies
                .body(key.body(BodyId(0)))
                .expect("included body query")
                .expect("included body");
            assert_eq!(body.source().file_id, definitions[0].file_id);
            function
        })
    };
    let identity = check(&project);
    drop(project);
    assert_eq!(check(&fixture.build_project()), identity);
}

#[test]
fn member_macro_scope_and_broad_names_survive_restart() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:lazy_scope
macro_rules! methods { () => { pub fn wanted_early() -> u8 { 1 } }; }
pub struct Early;
impl Early { methods!(); }
macro_rules! owner { () => { pub struct Generated; impl Generated { methods!(); } }; }
owner!();
macro_rules! methods { () => { pub fn wanted_late() -> u16 { 1 } }; }
pub struct Late;
impl Late { methods!(); }
#[macro_export]
macro_rules! number { () => { pub fn wanted_root() -> u32 { 1 } }; }
macro_rules! qualified { () => { $crate::number!(); }; }
mod child {
    macro_rules! number { () => { pub fn wanted_root() -> u64 { 1 } }; }
    pub struct Nested;
    impl Nested { qualified!(); }
}

fn calls() {
    let late = Late::wanted_late(); let _ = $late$late;
    let generated = Generated::wanted_early(); let _ = $generated$generated;
    let early = Early::wanted_early(); let _ = $early$early;
    let root = child::Nested::wanted_root(); let _ = $root$root;
}
"#,
    );
    for restart in [false, true] {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("target");
            let analysis = snapshot.analysis();
            // On restart, discover all macro names before selecting a particular owner.
            if restart {
                assert_eq!(
                    analysis
                        .workspace_symbols("wanted_")
                        .expect("warm names")
                        .len(),
                    4
                );
            }
            for (marker, expected) in [
                ("late", UnsignedIntTy::U16),
                ("generated", UnsignedIntTy::U8),
                ("early", UnsignedIntTy::U8),
                ("root", UnsignedIntTy::U32),
            ] {
                assert_eq!(
                    analysis
                        .type_at(
                            context.crate_ref,
                            context.file,
                            fixture.markers().position(marker).offset
                        )
                        .expect("member call")
                        .expect("binding type")
                        .primitive(),
                    Some(PrimitiveTy::UnsignedInt(expected)),
                    "{marker}"
                );
            }
            let symbols = analysis
                .workspace_symbols("wanted_")
                .expect("all member names");
            assert_eq!(symbols.len(), 4);
        });
    }
}

#[test]
fn workspace_search_resolves_written_and_generated_member_names() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:lazy_search_names
pub trait Service {
    fn needle_decl(&self);
    const NEEDLE_REQUIRED: u8;
    type NeedleOutput;
}
pub struct Ordinary;
impl Ordinary {
    pub fn needle_direct() {}
    pub const NEEDLE_CONST: u8 = 1;
    pub type NeedleType = u8;
}
macro_rules! methods {
    () => { pub fn before() {} pub fn needle_generated() {} };
}
pub struct Generated;
impl Generated {
    methods!();
    pub fn needle_after() {}
}
pub fn needle_free() {}
"#,
    );
    for _ in 0..2 {
        // A second project reuses persisted discovery. Repeat each search in the same process too.
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            for _ in 0..2 {
                let symbols = snapshot
                    .analysis()
                    .workspace_symbols("NeEdLe")
                    .expect("search member names");
                let mut names = symbols
                    .iter()
                    .map(|symbol| {
                        (
                            symbol.name.as_str(),
                            symbol.kind,
                            symbol.container_name.as_deref(),
                        )
                    })
                    .collect::<Vec<_>>();
                names.sort_unstable_by_key(|(name, _, _)| *name);
                assert_eq!(
                    names,
                    [
                        ("NEEDLE_CONST", SymbolKind::Const, Some("impl Ordinary")),
                        ("NEEDLE_REQUIRED", SymbolKind::Const, Some("trait Service")),
                        ("NeedleOutput", SymbolKind::TypeAlias, Some("trait Service")),
                        ("NeedleType", SymbolKind::TypeAlias, Some("impl Ordinary")),
                        ("needle_after", SymbolKind::Method, Some("impl Generated")),
                        ("needle_decl", SymbolKind::Method, Some("trait Service")),
                        ("needle_direct", SymbolKind::Method, Some("impl Ordinary")),
                        ("needle_free", SymbolKind::Function, None),
                        (
                            "needle_generated",
                            SymbolKind::Method,
                            Some("impl Generated")
                        ),
                    ]
                );
                for symbol in symbols {
                    let text = snapshot
                        .file_text_for_span(
                            symbol.crate_ref.package,
                            symbol.file_id,
                            symbol.span.expect("symbol navigation span"),
                        )
                        .expect("symbol navigation text")
                        .expect("symbol source");
                    // Template names navigate to the invocation. The written member after that
                    // expansion must retain its own identity and name span, not `before`'s ID.
                    let expected = if symbol.name == "needle_generated" {
                        "methods!();"
                    } else {
                        &symbol.name
                    };
                    assert_eq!(text, expected, "{} navigation", symbol.name);
                }
            }
        });
    }
}

#[test]
fn included_owners_keep_their_collected_macro_scope() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:repeated_owners
mod first {
    macro_rules! methods { () => { pub fn first_member() {} }; }
    include!("shared.rs");
}
mod second {
    macro_rules! methods { () => { pub fn second_member() {} }; }
    include!("shared.rs");
}

//- /src/shared.rs
pub struct Value;
impl Value { methods!(); }
"#,
    );
    for _ in 0..2 {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let mut names = snapshot
                .analysis()
                .workspace_symbols("_member")
                .expect("names from both collected owners")
                .into_iter()
                .map(|symbol| symbol.name)
                .collect::<Vec<_>>();
            names.sort();
            assert_eq!(names, ["first_member", "second_member"]);
        });
    }
}

#[test]
fn saved_lazy_include_publishes_without_startup_membership() {
    for captured in [false, true] {
        let fixture = ProjectSourceFixture::build_with_fake_sysroot(
            r#"
//- /src/lib.rs crate:lazy_saved_source
pub struct Value;
impl Value { include!("methods.rs"); }

//- /src/methods.rs
pub fn before_member() {}
"#,
        );
        let mut project = fixture.build_project();
        project.inspect(|snapshot| {
            assert_eq!(
                snapshot
                    .analysis()
                    .workspace_symbols("before_member")
                    .expect("initial include")
                    .len(),
                1
            );
        });
        let path = fixture.path("src/methods.rs");
        let text = "pub fn after_member() {}\n";
        fs::write(&path, text).expect("save include");
        let change = if captured {
            SavedFileChange::captured(CapturedSource::new(&path, text).expect("capture save"))
        } else {
            SavedFileChange::fs_path(&path)
        };
        let summary = project
            .apply_changes([change.clone()])
            .expect("publish lazy source");
        assert_eq!(summary.changed_files.len(), 1);
        assert_eq!(
            project
                .apply_changes([change])
                .expect("repeat notification"),
            AnalysisChangeSummary::default()
        );
        for restarted in [false, true] {
            if restarted {
                project = fixture.build_project();
            }
            project.inspect(|snapshot| {
                let names = snapshot
                    .analysis()
                    .workspace_symbols("_member")
                    .expect("updated include")
                    .into_iter()
                    .map(|symbol| symbol.name)
                    .collect::<Vec<_>>();
                assert_eq!(names, ["after_member"]);
            });
        }
    }
}

#[test]
fn opened_lazy_include_prepares_saved_and_current_source() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:opened_lazy_include
pub struct Value;
impl Value { include!("methods.rs"); }

//- /src/methods.rs
pub fn included(&self) { let local = 1u16; let _ = $inside$local; }
"#,
    );
    let saved = fs::read_to_string(fixture.path("src/methods.rs")).expect("read include");
    let edited = saved.replace("1u16", "1u32");
    for _ in 0..2 {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/methods.rs"))
                .expect("route opened include")
                .expect("include has a target");
            let target = (context.crate_ref, context.file);
            let offset = fixture.markers().position("inside").offset;
            let cancellation = CancellationToken::new();
            for (text, relationship, ty) in [
                (&saved, SavedSourceRelationship::Exact, UnsignedIntTy::U16),
                (
                    &edited,
                    SavedSourceRelationship::Different,
                    UnsignedIntTy::U32,
                ),
            ] {
                let document = snapshot
                    .prepare_document_source(target, text, &cancellation)
                    .expect("prepare opened include");
                assert_eq!(
                    matches!(&document, DocumentSourceView::SavedExact(_)),
                    relationship == SavedSourceRelationship::Exact,
                );
                // Exercise explicit current preparation for unchanged text too. Edited text
                // reaches it through document preparation, as it does for an open editor.
                let current = match document {
                    DocumentSourceView::SavedExact(_) => snapshot
                        .prepare_current_source(target, text, &cancellation)
                        .expect("prepare unchanged include explicitly"),
                    DocumentSourceView::Current(current) => current,
                };
                assert_eq!(
                    current.relationship(context.crate_ref.package, context.file),
                    Some(relationship),
                );
                let (analysis, summary) = snapshot
                    .analysis_for_current_source(
                        target,
                        current,
                        CurrentSourceSelection::AtOffset(offset),
                        |_| Ok(()),
                    )
                    .expect("prepare included method body");
                assert!(summary.is_complete(), "{summary:?}");
                assert_eq!(
                    analysis
                        .type_at(context.crate_ref, context.file, offset)
                        .expect("query opened include")
                        .expect("local type")
                        .primitive(),
                    Some(PrimitiveTy::UnsignedInt(ty)),
                );
            }
        });
    }
}

#[test]
fn queries_starting_in_shared_member_include_survive_restart() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:included_source_queries
pub struct First;
pub struct Second;
impl First { include!("methods.rs"); }
impl Second { include!("methods.rs"); }

//- /src/methods.rs
pub fn included() -> u16 { let local = 1u16; let _ = $inside$local; local }
"#,
    );
    for _ in 0..2 {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            // Route the include before any semantic request has opened either owner.
            let context = snapshot
                .file_context_for_path(fixture.path("src/methods.rs"))
                .expect("discover include owners")
                .expect("include has a crate context");
            let ownership = complete_source_ownership(snapshot.query_context(), context.crate_ref)
                .expect("complete source ownership");
            assert_eq!(ownership.owners(context.file).len(), 2);
            let analysis = snapshot.analysis();
            let offset = fixture.markers().position("inside").offset;
            assert_eq!(
                analysis
                    .type_at(context.crate_ref, context.file, offset)
                    .expect("included body")
                    .expect("local type")
                    .primitive(),
                Some(PrimitiveTy::UnsignedInt(UnsignedIntTy::U16))
            );
            let outline = analysis
                .document_symbols(context.crate_ref, context.file)
                .expect("include outline");
            assert!(
                outline
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == "included")
            );
            let rename = analysis
                .rename(
                    context.crate_ref,
                    context.file,
                    offset,
                    "renamed",
                    ReferenceQuery::file_scoped(context.crate_ref, context.file),
                )
                .expect("included local rename");
            assert!(rename.is_some());
            assert_eq!(
                analysis
                    .workspace_symbols("included")
                    .expect("both include interpretations")
                    .len(),
                2
            );
        });
    }
}

#[test]
fn reference_scan_includes_every_interpretation_of_a_module_file() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:shared_source_interpretations
pub const VALUE: u8 = 1;
mod helpers;
mod outer {
    pub const $target$VALUE: u16 = 2;
    mod inner {
        pub struct Thing;
        impl Thing { include!("helpers.rs"); }
    }
}

//- /src/helpers.rs
pub fn shared() { let _ = super::$reference$VALUE; }
"#,
    );
    for _ in 0..2 {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("root context")
                .expect("root target");
            let included = snapshot
                .file_context_for_path(fixture.path("src/helpers.rs"))
                .expect("shared file context")
                .expect("shared target");
            let crates = [context.crate_ref];
            let references = snapshot
                .analysis()
                .references(
                    context.crate_ref,
                    context.file,
                    fixture.markers().position("target").offset,
                    ReferenceQuery::find_references(&crates, true),
                )
                .expect("all source interpretations");
            assert!(
                references
                    .iter()
                    .any(|reference| reference.file_id == included.file
                        && reference.span.start == fixture.markers().position("reference").offset)
            );
        });
    }
}
