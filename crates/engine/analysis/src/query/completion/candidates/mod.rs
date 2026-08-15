//! Completion candidate assembly from generic indexed views.
//!
//! Completion renderers need editor-specific policies, but they should not know which frozen
//! storage owns name, member, or type lookup. This adapter accepts completion-domain cursor sites
//! and projects generic view facts into completion-ready candidates.

mod associated;
mod auto_import;
mod member;
mod module;
mod scope;

use rg_ir_model::{
    BodyRef, EnumVariantFieldRef, FieldRef, FunctionRef, ModuleRef, Path, ScopeId,
    identity::DeclarationRef,
};
use rg_ir_view::{
    IndexedViewDb,
    lookup::name::{MacroKind, NameNamespace, NameOrigin},
    source::IndexedSignatureTypeScope,
};

use crate::model::{CompletionApplicability, CompletionKind, CompletionTarget};

/// Definition-shaped candidate after module, associated-item, or auto-import lookup.
///
/// Storage-specific views have already supplied semantic identity and documentation. Optional
/// macro/function/import facts remain attached so one renderer can choose invocation syntax,
/// details, and additional edits without repeating lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefinitionCompletionCandidate {
    label: String,
    namespace: NameNamespace,
    module_origin: Option<NameOrigin>,
    target: CompletionTarget,
    kind: CompletionKind,
    applicability: CompletionApplicability,
    documentation: Option<String>,
    function: Option<FunctionRef>,
    macro_kind: Option<MacroKind>,
    import_path: Option<Path>,
    import_path_len: Option<usize>,
}

impl DefinitionCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn module_origin(&self) -> Option<NameOrigin> {
        self.module_origin
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn applicability(&self) -> CompletionApplicability {
        self.applicability
    }

    pub(crate) fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }

    pub(crate) fn function_ref(&self) -> Option<FunctionRef> {
        self.function
    }

    pub(crate) fn is_invocation_macro(&self) -> bool {
        self.macro_kind == Some(MacroKind::Invocation)
    }

    pub(crate) fn macro_kind(&self) -> Option<MacroKind> {
        self.macro_kind
    }

    pub(crate) fn import_path(&self) -> Option<&Path> {
        self.import_path.as_ref()
    }

    pub(crate) fn import_path_len(&self) -> Option<usize> {
        self.import_path_len
    }
}

/// One name visible from an indexed body scope before shadowing and rendering.
///
/// `scope_distance` orders nearer lexical scopes first. `shadow_namespaces` records every
/// namespace occupied by the declaration's source shape: a local binding shadows only values,
/// while a tuple or unit struct can shadow both its type name and value constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LexicalCompletionCandidate {
    label: String,
    namespace: NameNamespace,
    scope_distance: usize,
    target: CompletionTarget,
    kind: CompletionKind,
    declaration: Option<DeclarationRef>,
    function: Option<FunctionRef>,
    shadow_namespaces: Vec<NameNamespace>,
}

impl LexicalCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn scope_distance(&self) -> usize {
        self.scope_distance
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn declaration_ref(&self) -> Option<DeclarationRef> {
        self.declaration
    }

    pub(crate) fn function_ref(&self) -> Option<FunctionRef> {
        self.function
    }

    pub(crate) fn shadow_namespaces(&self) -> &[NameNamespace] {
        &self.shadow_namespaces
    }
}

/// A named type/const parameter or impl `Self` projected out of a declaration's generic scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenericScopeCompletionCandidate {
    label: String,
    namespace: NameNamespace,
    target: CompletionTarget,
    kind: CompletionKind,
}

impl GenericScopeCompletionCandidate {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn target(&self) -> CompletionTarget {
        self.target
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }
}

/// Scope used to verify that a primitive-looking spelling still resolves to the builtin type.
///
/// Primitive candidates are not unconditional keywords: a declaration can shadow their spelling.
/// Body and signature paths use different resolution anchors, so the check keeps that distinction.
#[derive(Debug, Clone, Copy)]
enum PrimitiveTypePathScope {
    Body { body: BodyRef, scope: ScopeId },
    Signature(IndexedSignatureTypeScope),
    Module(ModuleRef),
}

/// Resolved dot-method identity plus the trait applicability used for ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotMethodCompletionCandidate {
    function: FunctionRef,
    kind: CompletionKind,
    applicability: CompletionApplicability,
}

/// Stable field identity selected for a record literal or pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordFieldCompletionCandidate {
    Type(FieldRef),
    EnumVariant(EnumVariantFieldRef),
}

impl DotMethodCompletionCandidate {
    pub(crate) fn function_ref(&self) -> FunctionRef {
        self.function
    }

    pub(crate) fn kind(&self) -> CompletionKind {
        self.kind
    }

    pub(crate) fn applicability(&self) -> CompletionApplicability {
        self.applicability
    }
}

/// Turns normalized cursor sites into completion candidates from the matching indexed views.
///
/// This is the boundary where a site's semantic scope chooses body-local, declaration-generic, or
/// module lookup. Rendering and sort policy stay outside so they can operate on one candidate
/// vocabulary regardless of which frozen store supplied it.
pub(crate) struct CompletionCandidateSource<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> CompletionCandidateSource<'a, 'db> {
    pub(crate) fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }
}
