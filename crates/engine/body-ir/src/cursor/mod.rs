//! Cursor-oriented queries over lowered function bodies.
//!
//! Analysis owns the public query vocabulary, but Body IR owns body source layout: expression
//! spans, binding spans, body-local item names, let annotations, and dot-completion receiver
//! ranges. These queries are intentionally exposed only on read transactions.

mod scan;

use rg_ir_model::{
    BindingId, BodyRef, EnumVariantRef, ExprId, FieldRef, FunctionRef, ScopeId, SemanticItemRef,
    TargetRef,
};
use rg_ir_storage::Path;
use rg_item_tree::FieldKey;
use rg_package_store::PackageStoreError;
use rg_parse::{FileId, Span};

use crate::BodyIrReadTxn;

use self::scan::{
    BodyCursorScanner, BodySourceScanner, DotCompletionSiteScanner, PathCompletionSiteScanner,
    RecordFieldCompletionSiteScanner, UnqualifiedCompletionSiteScanner,
};

/// Source site selected for a dot-completion query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DotCompletionSite {
    pub body: BodyRef,
    pub receiver: ExprId,
    /// Member-name prefix already typed after the dot.
    ///
    /// For a bare dot, this is an empty span at the completion offset.
    pub member_prefix_span: Span,
}

/// Namespace expected by a path-completion site inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCompletionNamespace {
    Types,
    Values,
}

/// Namespace expected by an unqualified completion site inside a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnqualifiedCompletionNamespace {
    Types,
    Values,
}

/// Source site selected for a qualified-path completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Path before the segment being completed.
    pub qualifier: Path,
    /// Segment prefix already typed after `::`.
    pub member_prefix_span: Span,
    pub namespace: PathCompletionNamespace,
}

/// Source site selected for an unqualified completion query inside a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnqualifiedCompletionSite {
    pub body: BodyRef,
    pub scope: ScopeId,
    /// Name prefix already typed at the cursor.
    pub member_prefix_span: Span,
    pub member_prefix: String,
    pub namespace: UnqualifiedCompletionNamespace,
    /// Number of body-wide bindings visible before this source site.
    ///
    /// Bindings are allocated in source order, so this boundary prevents later
    /// `let` declarations from completing before they are in scope.
    pub visible_bindings: usize,
}

/// Source site selected for a record-field completion query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFieldCompletionSite {
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
pub enum BindingSurface {
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
pub enum RecordFieldKeySurface {
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
pub enum ValueReferenceSource {
    /// Expression-backed reference, e.g. `foo` in `foo()` or `name` in `User { name }`.
    Expr(ExprId),
    /// Path-segment reference without a dedicated expression id, e.g. `Some` in a pattern path.
    Path(Path),
}

/// Source spelling for a value-namespace reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueReferenceSurface {
    /// Ordinary value reference syntax, e.g. `user` in `user.name`.
    Plain,
    /// Value implied by record-expression shorthand, e.g. `name` in `User { name }`.
    ///
    /// The field key and whole field span let rename expand the token to `field_key: new_value`
    /// while preserving that the occurrence still resolves like a normal value reference.
    RecordExprShorthand { key: FieldKey, field_span: Span },
}

/// One body source node that can participate in cursor queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyCursorCandidate {
    /// Function body declaration, e.g. the name in `fn use_it() { ... }`.
    Body { body: BodyRef, span: Span },
    /// Local binding introduced by a parameter or pattern, e.g. `user` in `let user = input;`.
    Binding {
        body: BodyRef,
        binding: BindingId,
        span: Span,
        surface: BindingSurface,
    },
    /// Lowered expression node, e.g. the whole `user.id()` call expression.
    Expr {
        body: BodyRef,
        expr: ExprId,
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
    /// Explicit record field key, e.g. `name` in `User { name: value }`.
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

impl BodyCursorCandidate {
    pub fn span(&self) -> Span {
        match self {
            Self::Body { span, .. }
            | Self::Binding { span, .. }
            | Self::Expr { span, .. }
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

impl BodyIrReadTxn<'_> {
    /// Returns body-local cursor candidates at `offset`, including let-annotation type paths.
    pub fn cursor_candidates(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        BodyCursorScanner::new(self, target, file_id, offset).scan()
    }

    /// Returns body-local source candidates in one target.
    pub fn source_candidates(
        &self,
        target: TargetRef,
        file_id: Option<FileId>,
    ) -> Result<Vec<BodyCursorCandidate>, PackageStoreError> {
        BodySourceScanner::new(self, target, file_id).scan()
    }

    /// Returns the source site for a dot-completion query.
    pub fn dot_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<DotCompletionSite>, PackageStoreError> {
        DotCompletionSiteScanner::new(self, target, file_id, offset).site_at_dot()
    }

    /// Returns the source site for a qualified-path completion query inside a body.
    pub fn path_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<PathCompletionSite>, PackageStoreError> {
        PathCompletionSiteScanner::new(self, target, file_id, offset).site_at_path()
    }

    /// Returns the source site for an unqualified completion query inside a body.
    pub fn unqualified_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<UnqualifiedCompletionSite>, PackageStoreError> {
        UnqualifiedCompletionSiteScanner::new(self, target, file_id, offset).site_at_name()
    }

    /// Returns the source site for a record-field completion query inside a body.
    pub fn record_field_completion_site(
        &self,
        target: TargetRef,
        file_id: FileId,
        offset: u32,
    ) -> Result<Option<RecordFieldCompletionSite>, PackageStoreError> {
        RecordFieldCompletionSiteScanner::new(self, target, file_id, offset).site_at_record_field()
    }
}
