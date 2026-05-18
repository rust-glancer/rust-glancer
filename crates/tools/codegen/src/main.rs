mod codegen;
mod util;

use std::{fmt, path::PathBuf};

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Parser)]
struct Args {
    #[arg(value_enum, default_value_t = CodegenCommand::All)]
    command: CodegenCommand,

    #[arg(long, global = true)]
    check: bool,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum CodegenCommand {
    #[default]
    All,
    Grammar,
    ParserTests,
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

fn main() -> Result<()> {
    let args = Args::parse();
    codegen::run(args.command, args.check);
    Ok(())
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("rg_codegen lives in crates/tools/codegen")
        .to_path_buf()
}
