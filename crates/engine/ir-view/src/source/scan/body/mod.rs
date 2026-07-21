//! Source scanning over lowered function bodies.
//!
//! Body IR owns the structural body and its source facts. The indexed view owns the editor-facing
//! interpretation: which node is under a cursor, which spelling is a reference, and which source
//! shape can accept completion. Keeping the scanners here prevents those query concepts from
//! becoming part of Body IR's storage API.
//!
//! The scanners cover three different shapes of request over the same body:
//!
//! ```text
//! user.na$0                  point query: select `na`
//! User { na$0 }              completion query: select the record field slot
//! let user = input; use(user) whole-file query: retain every declaration and reference
//! ```

mod cursor;
mod dot_completion_site;
mod path_completion_site;
mod paths;
mod record_field_completion_site;
mod record_pat_shorthand;
mod sites;
mod source;
mod unqualified_completion_site;
mod walk;

use rg_ir_model::{
    BindingId, BodyRef, EnumVariantRef, ExprId, FieldKey, FieldRef, FunctionRef, LocalDefRef, Path,
    ScopeId, SemanticItemRef,
};
use rg_parse::{FileId, Span};

use crate::lookup::name::ValueOrTypeNamespace;

pub(crate) use self::{
    cursor::BodyCursorScanner, dot_completion_site::DotCompletionSiteScanner,
    path_completion_site::PathCompletionSiteScanner,
    record_field_completion_site::RecordFieldCompletionSiteScanner, source::BodySourceScanner,
    unqualified_completion_site::UnqualifiedCompletionSiteScanner,
};

/// Source site selected for a dot-completion query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DotCompletionSite {
    pub body: BodyRef,
    pub receiver: ExprId,
    /// Member-name prefix already typed after the dot.
    ///
    /// For a bare dot, this is an empty span at the completion offset.
    pub member_prefix_span: Span,
}

/// Source site selected for a qualified-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Path before the segment being completed.
    pub qualifier: Path,
    /// Segment prefix already typed after `::`.
    pub member_prefix_span: Span,
    pub namespace: ValueOrTypeNamespace,
}

/// Source site selected for an unqualified completion query inside a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnqualifiedCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Name prefix already typed at the cursor.
    pub member_prefix_span: Span,
    pub member_prefix: String,
    pub context: BodyUnqualifiedNameContext,
    /// Number of body-wide bindings visible before this source site.
    ///
    /// Bindings are allocated in source order, so this boundary prevents later
    /// `let` declarations from completing before they are in scope.
    pub visible_bindings: usize,
}

/// Type/value interpretation selected by body syntax for an unqualified name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyUnqualifiedNameContext {
    /// A name in an annotation, such as `Us$0` in `let value: Us$0`.
    Type(super::TypeNamePosition),
    /// A name in an expression, such as `inp$0` in `let value = inp$0`.
    Value,
}

/// Source site selected for a record-field completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordFieldCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Struct-like path before the record field list.
    pub owner: Path,
    /// Field-name prefix already typed inside the record field list.
    pub member_prefix_span: Span,
    /// Named fields already written in this literal or pattern.
    pub existing_fields: Vec<FieldKey>,
}

/// Source spelling for a local binding declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindingSurface {
    /// Ordinary binding syntax, e.g. `user` in `let user = input;`.
    Plain,
    /// Binding introduced by record-pattern shorthand, e.g. `name` in `let User { name } = user;`.
    ///
    /// The field and pattern spans let rename expand the field while preserving modifiers and
    /// subpatterns:
    ///
    /// - rename field: `User { ref name }` -> `User { title: ref name }`
    /// - rename binding: `User { ref name }` -> `User { name: ref title }`
    RecordPatShorthand {
        key: FieldKey,
        field_span: Span,
        pat_span: Span,
        binding_name_span: Span,
    },
}

/// Source spelling for a record field key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordFieldKeySurface {
    /// Explicit key syntax, e.g. `name` in `User { name: value }`.
    Explicit,
    /// Expression shorthand key syntax, e.g. `name` in `User { name }`.
    ///
    /// Renaming the field key expands the value expression: `User { name }` becomes
    /// `User { title: name }`.
    RecordExprShorthand { field_span: Span },
    /// Pattern shorthand key syntax, e.g. `name` in `User { ref name }`.
    ///
    /// Renaming the field key must preserve the whole pattern: `User { ref name }` becomes
    /// `User { title: ref name }`.
    RecordPatShorthand { field_span: Span, pat_span: Span },
}

/// Lowered source backing a value-namespace reference candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValueReferenceSource {
    /// Expression-backed reference, e.g. `foo` in `foo()` or `name` in `User { name }`.
    Expr(ExprId),
    /// Path-segment reference without a dedicated expression id, e.g. `Some` in a pattern path.
    Path(Path),
}

/// Source spelling for a value-namespace reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValueReferenceSurface {
    /// Ordinary value reference syntax, e.g. `user` in `user.name`.
    Plain,
    /// Value implied by record-expression shorthand, e.g. `name` in `User { name }`.
    ///
    /// The field key and whole field span let rename expand the token to `field_key: new_value`
    /// while preserving that the occurrence still resolves like a normal value reference.
    RecordExprShorthand { key: FieldKey, field_span: Span },
}

/// One body source node that can become an indexed occurrence.
///
/// This is an internal transport shape between structural body scanning and the normalized source
/// facade. It keeps source distinctions such as record shorthand long enough for rename and
/// references to interpret them correctly; analysis code does not consume this enum directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BodySourceCandidate {
    /// Fallback source for a function body when no narrower body-local node owns the cursor.
    Body { body: BodyRef, span: Span },
    /// Local binding introduced by a parameter or pattern, e.g. `user` in `let user = input;`.
    Binding {
        body: BodyRef,
        binding: BindingId,
        span: Span,
        surface: BindingSurface,
    },
    /// Lowered expression node with its useful source spelling, e.g. `id` in `user.id()`.
    Expr {
        body: BodyRef,
        expr: ExprId,
        span: Span,
    },
    /// Macro invocation path written inside a body, e.g. `format` in `format!("{}", value)`.
    MacroCall {
        definition: LocalDefRef,
        file_id: FileId,
        span: Span,
    },
    /// Body-local type-namespace item, e.g. `User` in `fn f() { struct User; }`.
    ///
    /// This also covers local `enum`, `union`, `type`, and `trait` declarations.
    LocalItem { item: SemanticItemRef, span: Span },
    /// Body-local value-namespace item, e.g. `DEFAULT` in `fn f() { const DEFAULT: u8 = 0; }`.
    ///
    /// This covers local `const` and `static` declarations, not `let` bindings.
    LocalValueItem { item: SemanticItemRef, span: Span },
    /// Field declared on a body-local struct or union,
    /// e.g. `id` in `fn f() { struct User { id: Id } }`.
    LocalField { field: FieldRef, span: Span },
    /// Variant declared on a body-local enum, e.g. `Start` in `enum Action { Start }`.
    LocalEnumVariant { variant: EnumVariantRef, span: Span },
    /// Body-local function-like item, e.g. `helper` in `fn f() { fn helper() {} }`.
    LocalFunction { function: FunctionRef, span: Span },
    /// Record field key, e.g. `name` in either `User { name: value }` or `User { name }`.
    RecordFieldKey {
        body: BodyRef,
        scope: ScopeId,
        owner: Path,
        key: FieldKey,
        file_id: FileId,
        span: Span,
        surface: RecordFieldKeySurface,
    },
    /// Value-namespace reference, e.g. `user`, `Status::Ready`, or `name` in `User { name }`.
    ValueReference {
        body: BodyRef,
        scope: ScopeId,
        source: ValueReferenceSource,
        file_id: FileId,
        span: Span,
        surface: ValueReferenceSurface,
    },
    /// Type-namespace path inside a body, e.g. `User` in `let user: User;`.
    TypePath {
        body: BodyRef,
        scope: ScopeId,
        path: Path,
        file_id: FileId,
        span: Span,
    },
}

impl BodySourceCandidate {
    pub(crate) fn span(&self) -> Span {
        match self {
            Self::Body { span, .. }
            | Self::Binding { span, .. }
            | Self::Expr { span, .. }
            | Self::MacroCall { span, .. }
            | Self::LocalItem { span, .. }
            | Self::LocalValueItem { span, .. }
            | Self::LocalField { span, .. }
            | Self::LocalEnumVariant { span, .. }
            | Self::LocalFunction { span, .. }
            | Self::RecordFieldKey { span, .. }
            | Self::ValueReference { span, .. }
            | Self::TypePath { span, .. } => *span,
        }
    }
}
