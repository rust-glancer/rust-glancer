use rg_analysis::ReferenceQuery;

use crate::testonly::ProjectSourceFixture;

#[test]
fn rename_ignores_synthesized_punctuation_in_macro_arguments() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:macro_path_rename
fn target<T>(value: T) -> T { value }
const $decl$VALUE: u8 = 1;
macro_rules! make {
    ($path:path, $value:ident) => { fn read() -> u8 { $path($value) } };
}
make!(target<u8>, $module_arg$VALUE);
fn outer() {
    make!(target<u8>, $body_arg$VALUE);
}
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let expected =
            ["decl", "module_arg", "body_arg"].map(|name| fixture.markers().position(name).offset);
        let crates = [context.crate_ref];
        let query = ReferenceQuery::find_references(&crates, true);
        for offset in expected {
            let rename = snapshot
                .analysis()
                .rename(context.crate_ref, context.file, offset, "RENAMED", query)
                .expect("compute rename")
                .unwrap_or_else(|| panic!("rename should succeed at {offset}"));
            assert_eq!(
                rename
                    .edits
                    .iter()
                    .map(|edit| edit.span.start)
                    .collect::<Vec<_>>(),
                expected,
            );
        }
    });
}

#[test]
fn rename_updates_forwarded_and_repeated_macro_arguments() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:macro_rename
macro_rules! format_to_acc {
    ($buf:expr, $lit:literal $($arg:tt)*) => {{
        let _ = $buf.write_fmt(format_args!($lit $($arg)*));
        $buf
    }};
}
#[macro_export]
macro_rules! match_like {
    ($node:ident, $name:pat => $body:expr) => { $crate::match_like!(($node), $name => $body) };
    (($node:expr), $name:pat => $body:expr) => { if let Some($name) = $node { $body } };
}
macro_rules! generate {
    ($value:ident) => { fn one() -> u8 { crate::$value } fn two() -> u8 { crate::$value } };
}
macro_rules! forward_items {
    ($value:ident) => { mod emitted { generate!($value); } };
}
macro_rules! define {
    ($name:ident) => { struct $name; fn in_signature(_: $name) -> $name { $name } };
}
macro_rules! type_only { ($name:ident) => { struct $name; }; }
struct Methods;
macro_rules! generate_member { ($value:ident) => { fn value() -> u8 { crate::$value } }; }
macro_rules! forward_member { ($value:ident) => { impl Methods { generate_member!($value); } }; }
macro_rules! pair { ($left:expr, $right:expr) => { ($left, $right) }; }
macro_rules! forward_twice { ($name:ident) => { pair!($name, $name) }; }
macro_rules! optional_suffix {
    ($name:ident $(, $suffix:ident)?) => {{
        drop($name);
        $(let _ = ${concat($name, $suffix)};)?
    }};
}

fn write() {
    let mut $acc_decl$acc = String::new();
    let _ = format_to_acc!($acc_use$acc, "{}", 1);
    let _ = format_to_acc!($acc_use_again$acc, "{}", 2);
}

fn bind(node: Option<u8>) {
    match_like!(node, $bind_decl$item => drop($bind_use$item));
}

const $const_decl$VALUE: u8 = 1;
fn generated() { generate!($const_use$VALUE); }
generate!($module_use$VALUE);
forward_items!($forwarded_use$VALUE);
forward_member!($member_use$VALUE);
define!($generated_decl$Generated);
fn takes_generated(_: $generated_use$Generated) {}
type_only!($only_decl$Only);
fn forward() { let $pair_decl$item = 1; let _ = forward_twice!($pair_use$item); }
fn omitted_suffix() { let $optional_decl$item = 1; optional_suffix!($optional_use$item); }
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        let crates = [context.crate_ref];
        let query = ReferenceQuery::find_references(&crates, true);
        for markers in [
            &["acc_decl", "acc_use", "acc_use_again"][..],
            &["bind_decl", "bind_use"],
            &[
                "const_decl",
                "const_use",
                "module_use",
                "forwarded_use",
                "member_use",
            ],
            &["generated_decl", "generated_use"],
            &["only_decl"],
            &["pair_decl", "pair_use"],
            &["optional_decl", "optional_use"],
        ] {
            let expected = markers
                .iter()
                .map(|name| fixture.markers().position(name).offset)
                .collect::<Vec<_>>();
            for &offset in &expected {
                assert!(
                    analysis
                        .prepare_rename(context.crate_ref, context.file, offset)
                        .expect("prepare rename")
                        .is_some(),
                    "{markers:?} at {offset}",
                );
                let rename = analysis
                    .rename(context.crate_ref, context.file, offset, "renamed", query)
                    .expect("rename")
                    .unwrap_or_else(|| panic!("rename should update {markers:?} from {offset}"));
                assert_eq!(
                    rename
                        .edits
                        .iter()
                        .map(|edit| (edit.span.start, edit.new_text.as_str()))
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|&offset| (offset, "renamed"))
                        .collect::<Vec<_>>(),
                    "{markers:?} at {offset}",
                );
            }
        }
    });
}

#[test]
fn rename_rejects_unverified_copies_of_macro_identifiers() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:macro_rename
struct First;
struct Second;
impl First { fn $method_decl$run(&self) {} }
impl Second { fn run(&self) {} }
macro_rules! both { ($name:ident) => {{ First.$name(); Second.$name(); }}; }
macro_rules! data { ($name:ident) => {{ drop($name); stringify!($name); }}; }
macro_rules! unresolved { ($name:ident) => {{ drop($name); First.$name(); }}; }
macro_rules! transformed { ($name:ident) => {{ drop($name); let _ = ${concat(prefix_, $name)}; }}; }
fn methods() { both!($method_use$run); }
fn stringify() { let $data_decl$value = 1; data!($data_use$value); }
fn missing() { let $missing_decl$value = 1; unresolved!($missing_use$value); }
fn concatenation() { let $concat_decl$value = 1; transformed!($concat_use$value); }
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route source")
            .expect("source target");
        let analysis = snapshot.analysis();
        let crates = [context.crate_ref];
        let query = ReferenceQuery::find_references(&crates, true);
        for (decl, usage) in [
            ("method_decl", "method_use"),
            ("data_decl", "data_use"),
            ("missing_decl", "missing_use"),
            ("concat_decl", "concat_use"),
        ] {
            let [decl_offset, use_offset] =
                [decl, usage].map(|name| fixture.markers().position(name).offset);
            // The intended occurrence must be visible so this test exercises rename's proof.
            assert!(
                analysis
                    .references(context.crate_ref, context.file, decl_offset, query)
                    .expect("find copied reference")
                    .iter()
                    .any(|reference| reference.span.start == use_offset),
                "{usage}",
            );
            assert!(
                analysis
                    .prepare_rename(context.crate_ref, context.file, use_offset)
                    .expect("prepare rename")
                    .is_none(),
                "{usage}",
            );
            for offset in [decl_offset, use_offset] {
                assert!(
                    analysis
                        .rename(context.crate_ref, context.file, offset, "renamed", query)
                        .expect("rename")
                        .is_none(),
                    "{decl} at {offset}",
                );
            }
        }
    });
}

#[test]
fn rename_rejects_macro_input_shared_by_different_module_owners() {
    let fixture = ProjectSourceFixture::build_with_fake_sysroot(
        r#"
//- /src/lib.rs crate:shared_macro_rename
macro_rules! call { ($name:ident) => { $name() }; }
mod a {
    fn $declaration$run() {}
    include!("shared.rs");
}
mod b {
    fn run() {}
    include!("shared.rs");
}
//- /src/shared.rs
fn demo() { call!($argument$run); }
"#,
    );
    let project = fixture.build_project();
    project.inspect(|snapshot| {
        let context = snapshot
            .file_context_for_path(fixture.path("src/lib.rs"))
            .expect("route declaration")
            .expect("declaration target");
        let usage = snapshot
            .file_context_for_path(fixture.path("src/shared.rs"))
            .expect("route shared input")
            .expect("shared input target");
        let declaration = fixture.markers().position("declaration").offset;
        let argument = fixture.markers().position("argument").offset;
        let crates = [context.crate_ref];
        let query = ReferenceQuery::find_references(&crates, true);
        let analysis = snapshot.analysis();
        assert!(
            analysis
                .references(context.crate_ref, context.file, declaration, query)
                .expect("find copied use in a")
                .iter()
                .any(
                    |usage_ref| usage_ref.file_id == usage.file && usage_ref.span.start == argument
                )
        );
        assert!(
            analysis
                .rename(
                    context.crate_ref,
                    context.file,
                    declaration,
                    "renamed",
                    query
                )
                .expect("rename")
                .is_none(),
        );
    });
}
