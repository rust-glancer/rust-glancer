//! Generates checked-in parser and syntax files for repository maintenance.

mod codegen;
mod util;

use std::{fmt, path::PathBuf};

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum CodegenCommand {
    #[default]
    All,
    Grammar,
    ParserTests,
}

impl CodegenCommand {
    pub fn run(self, check: bool) {
        match self {
            Self::All => {
                codegen::grammar::generate(check);
                codegen::parser_inline_tests::generate(check);
            }
            Self::Grammar => codegen::grammar::generate(check),
            Self::ParserTests => codegen::parser_inline_tests::generate(check),
        }
    }
}

impl fmt::Display for CodegenCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenCommand::All => f.write_str("all"),
            CodegenCommand::Grammar => f.write_str("grammar"),
            CodegenCommand::ParserTests => f.write_str("parser-tests"),
        }
    }
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("rg_codegen lives in crates/tools/codegen")
        .to_path_buf()
}
