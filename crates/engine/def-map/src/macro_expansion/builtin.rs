//! Shared classification for compiler-provided macro names.
//!
//! Macro lookup should always prefer user definitions first. This classifier is only consulted
//! after normal macro resolution fails, so local `macro_rules! format_args` can still shadow the
//! compiler builtin.

use rg_ir_model::{BuiltinMacroExprKind, PathSegment};
use rg_ir_storage::ImportPath;
use rg_text::Name;

/// Known builtin macro call that should not be resolved as a user macro.
pub(crate) enum BuiltinMacroDisposition {
    /// The builtin cannot add module-scope definitions, so def-map can safely ignore it.
    IgnoredByDefMap,
    /// The builtin selects one item stream from cfg predicates.
    CfgSelect,
    /// The builtin splices another source file into the caller's item stream.
    Include,
    /// The builtin can affect item collection or requires dedicated compiler-like handling.
    Unsupported,
}

impl BuiltinMacroDisposition {
    /// Classifies builtin macros that are known even when no user macro binding resolves.
    ///
    /// We intentionally take a small shortcut here. Unqualified builtins and `std`/`core`-qualified
    /// builtin-shaped paths cover the realistic call sites, while a fully resolution-aware builtin
    /// prefix model would add a lot of complexity for rare local `std`/`core` shadowing cases.
    pub(crate) fn from_path(path: &ImportPath) -> Option<Self> {
        let name = macro_name_from_path(path)?;

        match name.as_str() {
            // Expression, diagnostic, or assembly builtins do not contribute named items to def-map.
            // Body lowering can synthesize values/types for the expression-like subset.
            "asm" | "cfg" | "column" | "compile_error" | "concat" | "env" | "file"
            | "format_args" | "format_args_nl" | "global_asm" | "include_bytes" | "include_str"
            | "line" | "llvm_asm" | "module_path" | "option_env" | "stringify" => {
                Some(Self::IgnoredByDefMap)
            }

            "cfg_select" => Some(Self::CfgSelect),
            "include" => Some(Self::Include),

            // `concat_idents!` has token-shaping behavior that is better handled by a dedicated
            // builtin implementation.
            "concat_idents" => Some(Self::Unsupported),
            _ => None,
        }
    }
}

/// Returns the Body IR expression shape for builtin macros that are useful inside bodies.
pub(crate) fn body_expr_kind_from_path(path: &ImportPath) -> Option<BuiltinMacroExprKind> {
    let name = macro_name_from_path(path)?;
    BuiltinMacroExprKind::from_macro_name(name.as_str())
}

// TODO: Shortcut/heuristic macro resolution for builtins: either direct path,
// or coming from `std`/`core`. It will not handle reexports, but that should be OK for now.
// (unless some popular crate does reexport built-ins and it breaks everything and then welp).
fn macro_name_from_path(path: &ImportPath) -> Option<&Name> {
    path.relative_single_name().or_else(|| {
        // Check if we have two segments and the first one is either `std` or `core`.
        let [PathSegment::Name(root), PathSegment::Name(name)] = path.segments.as_slice() else {
            return None;
        };
        matches!(root.as_str(), "std" | "core").then_some(name)
    })
}
