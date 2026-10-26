//! Source loading for real files discovered after macro expansion.
//!
//! Most expanded items can enter DefMap immediately because their complete syntax is in the macro
//! output. Two useful examples explain why that is not always enough:
//!
//! ```text
//! make_module!()   -> mod generated;
//! make_bindings!() -> include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
//! ```
//!
//! The first expansion names a child file but does not contain it. The second names a file left by
//! a Cargo build script. In both cases DefMap understands what the Rust construct means, but the
//! project layer owns path lookup, source capture, and ItemTree lowering. DefMap calls an injected
//! [`MacroSourceFileLoader`] with a complete request batch, then collects the loaded files before
//! continuing its macro queue. Semantic state stays on the construction call stack during loading.
//!
//! The shared loading boundary should not hide the semantic difference between the examples. A `mod`
//! answer creates a child module and supplies its new file-resolution context. An `include!`
//! answer splices items from a real file into the caller's existing module and keeps the caller's
//! module context. Requests and pending collection are construction details and do not survive in the
//! immutable DefMap.
//!
//! A “macro source file” is the real file at the far end of one of these edges. This keeps it
//! distinct from [`crate::source::GeneratedSourceData`], which is the in-memory syntax produced by
//! the macro itself.

use std::{collections::HashMap, sync::Arc};

use rg_ir_model::{FileId, PackageSlot};
use rg_item_tree::{IncludePathExpression, ItemTreeDb};
use rg_parse::{ModuleFileContext, ParseDb};
use rg_text::PackageNameInterners;

/// Captures and lowers a complete batch of real files needed by macro expansion.
///
/// Return one resolution per request, in the same order. A found file must already be captured in
/// Parse and lowered in ItemTree under the requested module context. This boundary owns source
/// access only; name resolution and collection remain inside DefMap construction.
pub trait MacroSourceFileLoader {
    fn load_files(
        &mut self,
        parse: &mut ParseDb,
        item_tree: &mut ItemTreeDb,
        interners: &mut PackageNameInterners,
        requests: &[MacroSourceFileRequest],
    ) -> anyhow::Result<Vec<MacroSourceFileResolution>>;
}

/// One coalesced project-layer lookup requested while DefMap is collecting expanded syntax.
///
/// Request identity contains everything needed for source lookup. Equal requests
/// can share one answer even when more than one expansion reached them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MacroSourceFileRequest {
    /// Resolves an out-of-line `mod name;` produced by a macro.
    ///
    /// `parent_context` is the directory interpretation at the macro call site. The project uses
    /// it with the generated name and optional `#[path]`, then returns both the file and the child
    /// context that nested `mod` declarations must inherit.
    Module {
        package: PackageSlot,
        parent_context: Arc<ModuleFileContext>,
        module_name: String,
        path_override: Option<String>,
    },
    /// Resolves an `include!(...)` call that only became visible after macro expansion.
    ///
    /// `origin_file` is where relative include paths start. `module_file_context` is deliberately
    /// separate: included items stay in the caller's semantic module, so a nested `mod child;`
    /// uses the caller's module directory rather than a directory derived from the included file.
    Include {
        package: PackageSlot,
        origin_file: FileId,
        module_file_context: Arc<ModuleFileContext>,
        path: IncludePathExpression,
    },
}

impl MacroSourceFileRequest {
    pub(crate) fn module(
        package: PackageSlot,
        parent_context: Arc<ModuleFileContext>,
        module_name: String,
        path_override: Option<String>,
    ) -> Self {
        Self::Module {
            package,
            parent_context,
            module_name,
            path_override,
        }
    }

    pub(crate) fn include(
        package: PackageSlot,
        origin_file: FileId,
        module_file_context: Arc<ModuleFileContext>,
        path: IncludePathExpression,
    ) -> Self {
        Self::Include {
            package,
            origin_file,
            module_file_context,
            path,
        }
    }

    pub fn package(&self) -> PackageSlot {
        match self {
            Self::Module { package, .. } | Self::Include { package, .. } => *package,
        }
    }

    pub(crate) fn description(&self) -> String {
        match self {
            Self::Module {
                package,
                module_name,
                ..
            } => format!(
                "macro-generated module {module_name} for package {}",
                package.0
            ),
            Self::Include {
                package,
                origin_file,
                ..
            } => format!(
                "macro-generated include from file {} for package {}",
                origin_file.0, package.0
            ),
        }
    }
}

/// Settled project-layer result for one macro source-file request.
///
/// Absence from the surrounding map means that the project has not handled the request yet.
/// [`MacroSourceFileResolution::Missing`] means the lookup did finish but no usable source exists.
/// Construction retains these results to coalesce repeated requests across expansion batches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroSourceFileResolution {
    /// A generated `mod` file plus the context for resolving children inside that module.
    Module {
        file_id: FileId,
        child_context: Arc<ModuleFileContext>,
    },
    /// A real file whose items should be inserted at the generated `include!` call site.
    Include { file_id: FileId },
    /// The project completed the lookup without producing a usable source.
    Missing,
}

/// Resolutions retained during one synchronous DefMap build.
pub(crate) type MacroSourceFileResolutions =
    HashMap<MacroSourceFileRequest, MacroSourceFileResolution>;
