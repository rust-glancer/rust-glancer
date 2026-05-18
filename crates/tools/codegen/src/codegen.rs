use std::{
    fmt, fs,
    io::Write,
    mem,
    path::Path,
    process::{Command, Stdio},
};

use crate::{CodegenCommand, project_root};

pub(crate) mod grammar;
pub(crate) mod parser_inline_tests;

pub(crate) fn run(command: CodegenCommand, check: bool) {
    match command {
        CodegenCommand::All => {
            grammar::generate(check);
            parser_inline_tests::generate(check);
        }
        CodegenCommand::Grammar => grammar::generate(check),
        CodegenCommand::ParserTests => parser_inline_tests::generate(check),
    }
}

#[derive(Clone)]
pub(crate) struct CommentBlock {
    pub(crate) id: String,
    pub(crate) line: usize,
    pub(crate) contents: Vec<String>,
    is_doc: bool,
}

impl CommentBlock {
    #[allow(dead_code)]
    fn extract(tag: &str, text: &str) -> Vec<CommentBlock> {
        assert!(tag.starts_with(char::is_uppercase));

        let tag = format!("{tag}:");
        let mut blocks = CommentBlock::extract_untagged(text);
        blocks.retain_mut(|block| {
            let first = block.contents.remove(0);
            let Some(id) = first.strip_prefix(&tag) else {
                return false;
            };

            if block.is_doc {
                panic!("Use plain (non-doc) comments with tags like {tag}:\n    {first}");
            }

            id.trim().clone_into(&mut block.id);
            true
        });
        blocks
    }

    fn extract_untagged(text: &str) -> Vec<CommentBlock> {
        let mut res = Vec::new();

        let lines = text.lines().map(str::trim_start);

        let dummy_block = CommentBlock {
            id: String::new(),
            line: 0,
            contents: Vec::new(),
            is_doc: false,
        };
        let mut block = dummy_block.clone();
        for (line_num, line) in lines.enumerate() {
            match line.strip_prefix("//") {
                Some(mut contents) if !contents.starts_with('/') => {
                    if let Some('/' | '!') = contents.chars().next() {
                        contents = &contents[1..];
                        block.is_doc = true;
                    }
                    if let Some(' ') = contents.chars().next() {
                        contents = &contents[1..];
                    }
                    block.contents.push(contents.to_owned());
                }
                _ => {
                    if !block.contents.is_empty() {
                        let block = mem::replace(&mut block, dummy_block.clone());
                        res.push(block);
                    }
                    block.line = line_num + 2;
                }
            }
        }
        if !block.contents.is_empty() {
            res.push(block);
        }
        res
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum CodegenKind {
    Grammar,
    ParserTests,
}

impl fmt::Display for CodegenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenKind::Grammar => f.write_str("grammar"),
            CodegenKind::ParserTests => f.write_str("parser-tests"),
        }
    }
}

fn reformat(text: String) -> String {
    let rustfmt_toml = project_root().join("crates/tools/codegen/rustfmt.codegen.toml");
    let rustfmt_toml = rustfmt_toml
        .to_str()
        .expect("codegen rustfmt config path is UTF-8");

    // Generated parser and syntax files are easiest to review when they keep
    // rust-analyzer's formatting policy. The dedicated config pins that policy
    // for codegen output only, so vendored generated files remain comparable to
    // upstream without changing how the rest of this workspace is formatted.
    let mut args = vec![
        "--config-path".to_owned(),
        rustfmt_toml.to_owned(),
        "--config".to_owned(),
        "fn_single_line=true".to_owned(),
    ];

    let mut stdout = match run_rustfmt(
        "rustup",
        ["run", "stable", "rustfmt"].into_iter(),
        &args,
        &text,
    ) {
        Some(stdout) => stdout,
        None => {
            args.insert(0, "rustfmt".to_owned());
            run_rustfmt(
                args[0].as_str(),
                std::iter::empty::<&str>(),
                &args[1..],
                &text,
            )
            .expect("failed to run rustfmt")
        }
    };
    if !stdout.ends_with('\n') {
        stdout.push('\n');
    }
    stdout
}

fn run_rustfmt<'a>(
    program: &str,
    prefix_args: impl Iterator<Item = &'a str>,
    args: &[String],
    text: &str,
) -> Option<String> {
    let mut child = Command::new(program)
        .args(prefix_args)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    child
        .stdin
        .as_mut()
        .expect("rustfmt stdin is piped")
        .write_all(text.as_bytes())
        .expect("failed to send generated Rust to rustfmt");

    let output = child
        .wait_with_output()
        .expect("failed to wait for rustfmt");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("rustfmt failed:\n{stderr}");
    }
    Some(String::from_utf8(output.stdout).expect("rustfmt output is UTF-8"))
}

fn add_preamble(cg: CodegenKind, mut text: String) -> String {
    let preamble =
        format!("//! Generated by `cargo run -p rg_codegen -- {cg}`, do not edit by hand.\n\n");
    text.insert_str(0, &preamble);
    text
}

/// Checks that the `file` has the specified `contents`.
///
/// In write mode this updates stale generated files. In check mode this fails
/// loudly so CI can catch generated-code drift without mutating the workspace.
#[allow(clippy::print_stderr)]
fn ensure_file_contents(cg: CodegenKind, file: &Path, contents: &str, check: bool) -> bool {
    let contents = normalize_newlines(contents);
    if let Ok(old_contents) = fs::read_to_string(file)
        && normalize_newlines(&old_contents) == contents
    {
        return false;
    }

    let display_path = file.strip_prefix(project_root()).unwrap_or(file);
    if check {
        panic!(
            "{} was not up-to-date{}",
            file.display(),
            if std::env::var("CI").is_ok() {
                format!(
                    "\n    NOTE: run `cargo run -p rg_codegen -- {cg}` locally and commit the updated files\n"
                )
            } else {
                String::new()
            }
        );
    } else {
        eprintln!(
            "\n\x1b[31;1merror\x1b[0m: {} was not up-to-date, updating\n",
            display_path.display()
        );

        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).expect("failed to create generated file parent directory");
        }
        fs::write(file, contents).expect("failed to write generated file");
        true
    }
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}
