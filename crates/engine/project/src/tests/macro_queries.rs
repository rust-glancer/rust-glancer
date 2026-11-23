use rg_analysis::CompletionQuery;
use rg_ir_model::Span;

use crate::testonly::ProjectSourceFixture;

#[test]
fn structured_queries_use_written_ranges_of_macro_arguments() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:macro_queries
struct Widget;
struct Builder;
impl Widget {
    fn ready(&self) -> bool { true }
    fn builder(&self) -> Builder { Builder }
}
impl Builder { fn done(&self) {} }
fn parse(scale: u8, strict: bool) -> bool { strict }

macro_rules! inner { ($value:expr) => { $value }; }
macro_rules! forward { ($value:expr) => { inner!($value) }; }
macro_rules! repeat { ($value:expr) => { ($value, $value) }; }
macro_rules! repeat_in_blocks { ($value:expr) => { ({ $value }, { $value }) }; }
macro_rules! make_widget { () => { Widget }; }
macro_rules! bind { ($name:ident) => { let mut $name = 1_u8; }; }
macro_rules! generate { ($value:expr) => { fn nested() { let _ = $value; } }; }

fn test(widget: Widget) {
    forward!(widget./* keep this comment */$prefix$re$member$);
    let nested = ((Widget,),);
    forward!(nested.0.0.re$tuple_field$);
    forward!(make_widget!().re$macro_receiver$);
    repeat_in_blocks!(widget.re$distinct_scopes$);
    repeat!(parse(/* scale in bytes */ $scale$43, $strict$true));
    forward!(parse($macro_arg$inner!(44), $after_macro_arg$true));
    bind!(local$binding$);
    forward!(widget.builder()$chain$
        .done());
}

generate!(parse($generated_scale$44, $generated_strict$false));
$end$
"#,
    );
    let marker = |name| fixture.markers().position(name).offset;
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        for site in ["member", "tuple_field", "macro_receiver", "distinct_scopes"] {
            let offset = marker(site);
            let completion = analysis
                .completions_at(CompletionQuery::new(
                    context.crate_ref,
                    context.file,
                    offset,
                ))
                .expect("complete copied member");
            let ready = completion
                .items
                .iter()
                .find(|item| item.label == "ready")
                .unwrap_or_else(|| panic!("ready method at {site}"));
            let expected_start = if site == "member" {
                marker("prefix")
            } else {
                offset - 2
            };
            assert_eq!(
                ready.edit.expect("member replacement").replace,
                Span {
                    start: expected_start,
                    end: offset,
                },
                "{site}"
            );
        }

        for site in ["strict", "generated_strict", "after_macro_arg"] {
            let help = analysis
                .signature_help(context.crate_ref, context.file, marker(site))
                .expect("signature help")
                .expect("parse signature");
            assert_eq!(help.active_parameter, Some(1), "{site}");
            assert_eq!(&help.label[help.parameters[1].clone()], "strict: bool");
        }

        let hints = analysis
            .inlay_hints(
                context.crate_ref,
                context.file,
                Span {
                    start: 0,
                    end: marker("end"),
                },
                None,
            )
            .expect("collect hints");
        for (site, label) in [
            ("scale", "scale:"),
            ("macro_arg", "scale:"),
            ("generated_scale", "scale:"),
            ("binding", ": u8"),
            ("chain", "Builder"),
        ] {
            let matching: Vec<_> = hints
                .iter()
                .filter(|hint| hint.text_offset() == marker(site))
                .map(|hint| hint.label.as_str())
                .collect();
            assert_eq!(matching, [label], "{site}");
        }
    });
}

#[test]
fn structured_queries_do_not_pick_between_conflicting_macro_copies() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:macro_queries
mod left {
    pub fn parse(scale: u8) {}
    pub struct Widget;
    impl Widget { pub fn read(&self) {} }
}
mod right {
    pub fn parse(value: u8) {}
    pub struct Widget;
    impl Widget { pub fn reset(&self) {} }
}
macro_rules! both {
    ($value:expr) => {{
        { use crate::left::{parse, Widget}; $value; }
        { use crate::right::{parse, Widget}; $value; }
    }};
}
fn test() {
    both!(parse($arg$43));
    both!(Widget.re$member$);
}
"#,
    );
    let marker = |name| fixture.markers().position(name).offset;
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        let arg = marker("arg");
        let help = analysis
            .signature_help(context.crate_ref, context.file, arg)
            .expect("signature help");
        assert_eq!(help, None);
        let hints = analysis
            .inlay_hints(
                context.crate_ref,
                context.file,
                Span {
                    start: arg,
                    end: arg,
                },
                None,
            )
            .expect("collect parameter hints");
        assert!(hints.is_empty(), "{hints:?}");
        let completion = analysis
            .completions_at(CompletionQuery::new(
                context.crate_ref,
                context.file,
                marker("member"),
            ))
            .expect("complete member");
        assert!(
            !completion
                .items
                .iter()
                .any(|item| matches!(item.label.as_str(), "read" | "reset")),
            "{completion:?}",
        );
    });
}
