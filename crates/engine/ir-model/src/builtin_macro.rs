use rg_std::{MemorySize, Shrink};
use wincode::{SchemaRead, SchemaWrite};

/// Compiler-provided expression macro recognized before Body IR exists.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    derive_more::Display,
    SchemaRead,
    SchemaWrite,
    MemorySize,
    Shrink,
)]
#[memsize(leaf)]
#[shrink(leaf)]
pub enum BuiltinMacroExprKind {
    #[display("cfg")]
    Cfg,
    #[display("column")]
    Column,
    #[display("concat")]
    Concat,
    #[display("env")]
    Env,
    #[display("file")]
    File,
    #[display("format_args")]
    FormatArgs,
    #[display("format_args_nl")]
    FormatArgsNl,
    #[display("include_bytes")]
    IncludeBytes,
    #[display("include_str")]
    IncludeStr,
    #[display("line")]
    Line,
    #[display("module_path")]
    ModulePath,
    #[display("option_env")]
    OptionEnv,
    #[display("stringify")]
    Stringify,
}

impl BuiltinMacroExprKind {
    pub fn from_macro_name(name: &str) -> Option<Self> {
        Some(match name {
            "cfg" => Self::Cfg,
            "column" => Self::Column,
            "concat" => Self::Concat,
            "env" => Self::Env,
            "file" => Self::File,
            "format_args" => Self::FormatArgs,
            "format_args_nl" => Self::FormatArgsNl,
            "include_bytes" => Self::IncludeBytes,
            "include_str" => Self::IncludeStr,
            "line" => Self::Line,
            "module_path" => Self::ModulePath,
            "option_env" => Self::OptionEnv,
            "stringify" => Self::Stringify,
            _ => return None,
        })
    }
}
