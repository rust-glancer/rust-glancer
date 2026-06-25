//! Shared fallback classification for compiler-provided macro names.
//!
//! Macro lookup should always prefer user definitions first. Resolved definitions carry builtin
//! identity directly when item-tree saw `#[rustc_builtin_macro]`; the path classifier below remains
//! an item-position compatibility fallback for names that do not resolve through the sysroot yet.

use rg_ir_model::{PathSegment, items::BuiltinMacroKind};
use rg_ir_storage::ImportPath;
use rg_text::Name;

/// Classifies builtin macros that are known even when no user macro binding resolves.
///
/// We intentionally take a small shortcut here. Unqualified builtins and `std`/`core`-qualified
/// builtin-shaped paths cover the realistic call sites, while a fully resolution-aware builtin
/// prefix model would add a lot of complexity for rare local `std`/`core` shadowing cases.
pub(crate) fn kind_from_path(path: &ImportPath) -> Option<BuiltinMacroKind> {
    let name = macro_name_from_path(path)?;
    BuiltinMacroKind::from_known_name(name.as_str())
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
