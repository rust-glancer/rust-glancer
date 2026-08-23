//! Construction-only handshake for files requested by generated module declarations.
//!
//! DefMap can recognize generated `mod child;`, but it does not own source capture or ItemTree
//! lowering. It therefore pauses with a batch of requests, the project loads each requested source,
//! and the same mutable DefMap session resumes. Requests and pending declarations are discarded
//! once the final immutable DefMap is built.

use std::{collections::HashMap, sync::Arc};

use rg_parse::{FileId, ModuleFileContext};

use crate::{DefMapDb, PackageSlot};

/// One coalesced generated-module lookup that the project must try to load.
///
/// Equivalent declarations can share this request and therefore share one filesystem lookup,
/// while their semantic continuations remain separate. `parent_context` describes the module that
/// contains `mod child;`; resolving the request produces a different context for `child` itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GeneratedModuleRequest {
    package: PackageSlot,
    parent_context: Arc<ModuleFileContext>,
    module_name: String,
    path_override: Option<String>,
}

impl GeneratedModuleRequest {
    pub(crate) fn new(
        package: PackageSlot,
        parent_context: Arc<ModuleFileContext>,
        module_name: String,
        path_override: Option<String>,
    ) -> Self {
        Self {
            package,
            parent_context,
            module_name,
            path_override,
        }
    }

    pub fn package(&self) -> PackageSlot {
        self.package
    }

    pub fn parent_context(&self) -> &ModuleFileContext {
        &self.parent_context
    }

    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    pub fn path_override(&self) -> Option<&str> {
        self.path_override.as_deref()
    }
}

/// Settled result for one generated-module request.
///
/// Absence from the surrounding map means that the project has not handled the request yet.
/// `Missing` means it completed the lookup without finding a source. A found source also carries
/// the child context that must be used while lowering and collecting that module's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GeneratedModuleResolution {
    Found {
        file_id: FileId,
        child_context: Arc<ModuleFileContext>,
    },
    Missing,
}

/// Resolutions retained by one mutable DefMap construction session.
pub(crate) type GeneratedModuleResolutions =
    HashMap<GeneratedModuleRequest, GeneratedModuleResolution>;

/// One step of resumable DefMap construction.
pub enum DefMapBuildProgress {
    /// Construction paused until the project loads or rejects these generated modules.
    NeedsGeneratedModules(Vec<GeneratedModuleRequest>),
    /// Every generated-module request has been resolved and the immutable snapshot is complete.
    Complete(DefMapDb),
}
