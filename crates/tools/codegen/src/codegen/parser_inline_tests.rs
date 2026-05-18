//! This module greps parser's code for specially formatted comments and turns
//! them into tests.
#![allow(clippy::disallowed_types, clippy::print_stdout)]

use std::{
    collections::HashMap,
    fs, iter,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use itertools::Itertools as _;

use crate::{
    codegen::{CodegenKind, CommentBlock, ensure_file_contents, reformat},
    project_root,
    util::list_rust_files,
};

pub(crate) fn generate(check: bool) {
    let parser_crate_root = project_root().join("crates/engine/parser");
    let parser_test_data = parser_crate_root.join("test_data");
    let parser_test_data_inline = parser_test_data.join("parser/inline");

    let tests = tests_from_dir(&parser_crate_root.join("src/grammar"));

    let mut some_file_was_updated = false;
    some_file_was_updated |= install_tests(&tests.ok, parser_test_data_inline.join("ok"), check)
        .expect("failed to install ok parser inline tests");
    some_file_was_updated |= install_tests(&tests.err, parser_test_data_inline.join("err"), check)
        .expect("failed to install err parser inline tests");

    if some_file_was_updated {
        let _ = fs::File::open(parser_crate_root.join("src/tests.rs"))
            .expect("parser tests module exists")
            .set_modified(SystemTime::now());
    }

    let ok_tests = tests
        .ok
        .values()
        .sorted_by(|a, b| a.name.cmp(&b.name))
        .map(|test| {
            let test_name = quote::format_ident!("{}", test.name);
            let test_file = format!("test_data/parser/inline/ok/{test_name}.rs");
            let (test_func, args) = match &test.edition {
                Some(edition) => {
                    let edition = quote::format_ident!("Edition{edition}");
                    (
                        quote::format_ident!("run_and_expect_no_errors_with_edition"),
                        quote::quote! {#test_file, crate::Edition::#edition},
                    )
                }
                None => (
                    quote::format_ident!("run_and_expect_no_errors"),
                    quote::quote! {#test_file},
                ),
            };
            quote::quote! {
                #[test]
                fn #test_name() {
                    #test_func(#args);
                }
            }
        });
    let err_tests = tests
        .err
        .values()
        .sorted_by(|a, b| a.name.cmp(&b.name))
        .map(|test| {
            let test_name = quote::format_ident!("{}", test.name);
            let test_file = format!("test_data/parser/inline/err/{test_name}.rs");
            let (test_func, args) = match &test.edition {
                Some(edition) => {
                    let edition = quote::format_ident!("Edition{edition}");
                    (
                        quote::format_ident!("run_and_expect_errors_with_edition"),
                        quote::quote! {#test_file, crate::Edition::#edition},
                    )
                }
                None => (
                    quote::format_ident!("run_and_expect_errors"),
                    quote::quote! {#test_file},
                ),
            };
            quote::quote! {
                #[test]
                fn #test_name() {
                    #test_func(#args);
                }
            }
        });

    let output = quote::quote! {
        mod ok {
            use crate::tests::*;
            #(#ok_tests)*
        }
        mod err {
            use crate::tests::*;
            #(#err_tests)*
        }
    };

    let pretty = reformat(output.to_string());
    ensure_file_contents(
        CodegenKind::ParserTests,
        parser_test_data.join("generated/runner.rs").as_ref(),
        &pretty,
        check,
    );
}

fn install_tests(tests: &HashMap<String, Test>, tests_dir: PathBuf, check: bool) -> Result<bool> {
    if !tests_dir.is_dir() {
        fs::create_dir_all(&tests_dir)
            .context("while attempting to create parser inline test directory")?;
    }
    let existing = existing_tests(&tests_dir, TestKind::Ok)
        .context("while attempting to list existing parser inline tests")?;
    if let Some((t, (path, _))) = existing.iter().find(|&(t, _)| !tests.contains_key(t)) {
        panic!("Test `{t}` is deleted: {}", path.display());
    }

    let mut some_file_was_updated = false;

    for (name, test) in tests {
        let path = match existing.get(name) {
            Some((path, _test)) => path.clone(),
            None => tests_dir.join(name).with_extension("rs"),
        };
        if ensure_file_contents(CodegenKind::ParserTests, &path, &test.text, check) {
            some_file_was_updated = true;
        }
    }

    Ok(some_file_was_updated)
}

#[derive(Debug)]
struct Test {
    name: String,
    text: String,
    kind: TestKind,
    edition: Option<String>,
}

#[derive(Copy, Clone, Debug)]
enum TestKind {
    Ok,
    Err,
}

#[derive(Default, Debug)]
struct Tests {
    ok: HashMap<String, Test>,
    err: HashMap<String, Test>,
}

fn collect_tests(s: &str) -> Vec<Test> {
    let mut res = Vec::new();
    for comment_block in CommentBlock::extract_untagged(s) {
        let first_line = &comment_block.contents[0];
        let (name, kind) = if let Some(name) = first_line.strip_prefix("test ") {
            (name.to_owned(), TestKind::Ok)
        } else if let Some(name) = first_line.strip_prefix("test_err ") {
            (name.to_owned(), TestKind::Err)
        } else {
            continue;
        };
        let (name, edition) = match *name.split(' ').collect_vec().as_slice() {
            [name, edition] => {
                assert!(!edition.contains(' '));
                (name.to_owned(), Some(edition.to_owned()))
            }
            [name] => (name.to_owned(), None),
            _ => panic!("invalid test name: {name:?}"),
        };
        let text: String = edition
            .as_ref()
            .map(|edition| format!("// {edition}"))
            .into_iter()
            .chain(comment_block.contents[1..].iter().cloned())
            .chain(iter::once(String::new()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.trim().is_empty() && text.ends_with('\n'));
        res.push(Test {
            name,
            edition,
            text,
            kind,
        })
    }
    res
}

fn tests_from_dir(dir: &Path) -> Tests {
    let mut res = Tests::default();
    for entry in list_rust_files(dir) {
        process_file(&mut res, entry.as_path());
    }
    let grammar_rs = dir
        .parent()
        .expect("parser grammar directory has a parent")
        .join("grammar.rs");
    process_file(&mut res, &grammar_rs);
    return res;

    fn process_file(res: &mut Tests, path: &Path) {
        let text = fs::read_to_string(path).expect("failed to read parser grammar source");

        for test in collect_tests(&text) {
            if let TestKind::Ok = test.kind {
                if let Some(old_test) = res.ok.insert(test.name.clone(), test) {
                    panic!("Duplicate test: {}", old_test.name);
                }
            } else if let Some(old_test) = res.err.insert(test.name.clone(), test) {
                panic!("Duplicate test: {}", old_test.name);
            }
        }
    }
}

fn existing_tests(dir: &Path, ok: TestKind) -> Result<HashMap<String, (PathBuf, Test)>> {
    let mut res = HashMap::new();
    for file in
        fs::read_dir(dir).context("while attempting to read parser inline test directory")?
    {
        let path = file
            .context("while attempting to read parser inline test directory entry")?
            .path();
        let rust_file = path.extension().and_then(|ext| ext.to_str()) == Some("rs");

        if rust_file {
            let name = path
                .file_stem()
                .map(|x| x.to_string_lossy().to_string())
                .expect("parser inline test file has a stem");
            let text = fs::read_to_string(&path)
                .context("while attempting to read existing parser inline test")?;
            let edition = text
                .lines()
                .next()
                .and_then(|it| it.strip_prefix("// "))
                .map(ToOwned::to_owned);
            let test = Test {
                name: name.clone(),
                text,
                kind: ok,
                edition,
            };
            if let Some(old) = res.insert(name, (path, test)) {
                println!("Duplicate test: {old:?}");
            }
        }
    }
    Ok(res)
}
