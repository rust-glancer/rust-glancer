//! Record source occurrences while lowering syntax, then read them separately from declarations.
//!
//! A signature such as `fn load() -> User` needs to remember the type `User` even when an edit
//! moves the function. It keeps a [`SourceId`] for the type occurrence. Its byte range lives in a
//! separate table, so readers of declaration meaning do not also depend on those changing bytes.
//!
//! The flow has three parts:
//!
//! 1. The caller starts an owner and chooses how syntax ranges map to source data.
//! 2. Lowering records each relevant range and keeps only the returned IDs in its output.
//! 3. A source reader selects the matching table and resolves those IDs when positions are needed.
//!
//! File tables store byte ranges. Generated tables store token provenance that can be resolved
//! against file tables. Bodies can store already-resolved locations for their own source snapshot.
//! Bounded initializer previews also live on this side: changing `2 + 2` to `4` changes display
//! text and body meaning, but leaves the const's declaration and its occurrence IDs unchanged.

use std::collections::HashMap;

use rg_ir_model::{SourceId, SourceLocation, Span};
use rg_std::{MemorySize, Shrink};
use rg_syntax::{AstNode as _, TextRange, ast};
use rg_tt::{Span as TtSpan, TokenOwner};
use wincode::{SchemaRead, SchemaWrite};

/// Source data indexed by the owner/local pair in [`SourceId`].
///
/// Each owner has its own entry list. For `impl User { fn first() {} fn second() {} }`, lowering
/// reserves the impl's owner before visiting its methods. Recording more occurrences inside
/// `first` does not shift the impl's local indices. Adding a new owner can still shift later owners.
///
/// The payload determines what is known at this stage: a file uses [`Span`], generated declarations
/// use token origins, and a lowered body uses [`SourceLocation`]. The table itself only allocates
/// and stores entries; the mapper supplied to [`Self::start_owner`] interprets their ranges.
#[derive(Debug, Clone, PartialEq, Eq, MemorySize, Shrink, SchemaRead, SchemaWrite)]
pub struct DeclarationSourceStore<T> {
    owners: Vec<Vec<T>>,
    // Preview text changes independently of the declaration's name, type, and occurrence IDs.
    // Keep it sparse: most source occurrences are not initializer expressions.
    initializer_previews: HashMap<SourceId, String>,
}

impl<T> Default for DeclarationSourceStore<T> {
    fn default() -> Self {
        Self {
            owners: Vec::new(),
            initializer_previews: HashMap::new(),
        }
    }
}

impl<T> DeclarationSourceStore<T> {
    /// Reserve an owner before descending into its children. Nested declarations allocate their
    /// own owners, so their number of occurrences cannot change this declaration's local IDs.
    ///
    /// The builder does not borrow the store: lowering can start and finish child owners while
    /// keeping the parent's builder. Finish each builder in the same store that created it.
    pub fn start_owner<'a>(
        &mut self,
        map: &'a mut dyn FnMut(TextRange, SourceRole) -> T,
    ) -> DeclarationSourceBuilder<'a, T> {
        let owner = u32::try_from(self.owners.len()).expect("source owners fit u32");
        self.owners.push(Vec::new());
        DeclarationSourceBuilder {
            owner,
            entries: Vec::new(),
            initializer_previews: Vec::new(),
            map,
        }
    }

    /// Install the recorded entries in the reserved owner slot. Until this step, the builder's
    /// IDs have no entries to read from the store. Dropping a builder does not finish it.
    pub fn finish_owner(&mut self, builder: DeclarationSourceBuilder<'_, T>) {
        self.owners[builder.owner as usize] = builder.entries;
        self.initializer_previews
            .extend(builder.initializer_previews);
    }

    pub fn entries(&self) -> impl Iterator<Item = &T> {
        self.owners.iter().flatten()
    }

    pub fn get(&self, source: SourceId) -> Option<&T> {
        self.owners
            .get(source.owner as usize)?
            .get(source.local as usize)
    }

    /// Short, verbatim initializer text, when lowering could retain it within the display limit.
    /// This also works for generated declarations whose bodies cannot yet be analyzed.
    pub fn initializer_preview(&self, source: SourceId) -> Option<&str> {
        self.initializer_previews.get(&source).map(String::as_str)
    }
}

impl DeclarationSourceStore<Span> {
    /// Macro origins address the same occurrence entries as declarations. Resolve both
    /// endpoints because the parser can combine two punctuation tokens into one syntax token.
    /// The caller must select this token's package/file table first; the owner index alone does
    /// not identify that table. Synthetic tokens have no complete origin and return `None`.
    pub fn token_span(&self, token: TtSpan) -> Option<Span> {
        let TokenOwner::Declaration(owner) = token.anchor.owner else {
            return None;
        };
        let first = *self.get(SourceId {
            owner,
            local: token.origins.first(),
        })?;
        let last = *self.get(SourceId {
            owner,
            local: token.origins.last()?,
        })?;
        Some(first.cover(last))
    }
}

impl SourceLocations for DeclarationSourceStore<Span> {
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation> {
        self.get(occurrence).map(|span| SourceLocation {
            span: *span,
            editable: true,
        })
    }
}

/// The source-writing interface for lowering declarations that have source occurrences.
///
/// A range is always in the syntax being lowered. In a macro expansion, that means temporary
/// expansion syntax, not the invocation's file offsets. The recorder's mapper handles that
/// distinction before returning an ID; declaration lowering does not remap the result afterward.
///
/// Unlike [`TypeSource`], recording here always returns an ID. Use [`Self::record`] for ordinary
/// syntax and [`Self::record_as`] when the mapper needs to distinguish a declaration container
/// or an unnamed field from the syntax inside it.
pub trait RecordSource: TypeSource {
    fn record_as(&mut self, range: TextRange, role: SourceRole) -> SourceId;

    /// Record the initializer as one occurrence, regardless of its expression structure. The
    /// declaration keeps this ID; the source store keeps its optional bounded display text.
    fn record_initializer(&mut self, initializer: &ast::Expr) -> SourceId;

    fn record(&mut self, range: TextRange) -> SourceId {
        self.record_as(range, SourceRole::Syntax)
    }
}

/// Generated syntax can have a useful navigation destination without introducing an editable
/// occurrence there. In particular, an unnamed tuple field must not hide its type argument.
///
/// For a macro producing `struct Wrapper($ty)` from `wrap!(User)`, the copied `User` is selectable
/// type syntax. The tuple field and the whole struct instead navigate to the invocation. Keeping
/// these roles separate prevents the field from claiming the same occurrence as `User`.
#[derive(Debug, Clone, Copy)]
pub enum SourceRole {
    /// Names, paths, and other syntax that may map to copied input in an expansion.
    Syntax,
    /// The whole item. Generated items use the invocation as their navigation destination.
    Declaration,
    /// A tuple field has no name token of its own, even if its type was copied from input.
    UnnamedField,
}

/// Records one owner's occurrences before handing them back to its [`DeclarationSourceStore`].
///
/// Each call appends a new entry, even if the same range was recorded before. For example, a
/// tuple field and its type may cover identical bytes but have different [`SourceRole`] values.
/// IDs describe those recorded occurrences, not a deduplicated set of text ranges.
///
/// The mapper is fixed for the lifetime of this builder. It can retain provenance or resolve a
/// range immediately; that choice does not change the declaration-lowering interface.
pub struct DeclarationSourceBuilder<'a, T> {
    owner: u32,
    entries: Vec<T>,
    initializer_previews: Vec<(SourceId, String)>,
    map: &'a mut dyn FnMut(TextRange, SourceRole) -> T,
}

impl<T> RecordSource for DeclarationSourceBuilder<'_, T> {
    fn record_as(&mut self, range: TextRange, role: SourceRole) -> SourceId {
        let local = u32::try_from(self.entries.len()).expect("source occurrences fit u32");
        self.entries.push((self.map)(range, role));
        SourceId {
            owner: self.owner,
            local,
        }
    }

    fn record_initializer(&mut self, initializer: &ast::Expr) -> SourceId {
        let id = self.record(initializer.syntax().text_range());
        if let Some(preview) = initializer_preview(initializer) {
            self.initializer_previews.push((id, preview));
        }
        id
    }
}

impl<T> TypeSource for DeclarationSourceBuilder<'_, T> {
    fn record_type(&mut self, range: TextRange) -> Option<SourceId> {
        Some(self.record(range))
    }
}

/// Reads occurrences after the caller has selected their source table and snapshot.
///
/// A file, an expansion, and a body can all contain `SourceId { owner: 0, local: 0 }`. Implementors
/// provide the surrounding source context; the ID alone cannot choose it. Opening a source view
/// handles query reads before this interface is used to walk its occurrences.
///
/// Use [`Self::span`] for navigation, including fallbacks. Use [`Self::editable_span`] when a
/// scanner needs a real source occurrence, such as an identifier to include in references.
pub trait SourceLocations {
    /// `None` means there is no mapping for this occurrence in the selected source. Callers that
    /// require a declaration location should report that absence rather than invent a range.
    fn location(&self, occurrence: SourceId) -> Option<SourceLocation>;

    /// Includes navigation fallbacks. A returned range does not establish that it can be edited.
    fn span(&self, occurrence: SourceId) -> Option<Span> {
        self.location(occurrence).map(|location| location.span)
    }

    /// Also accepts source-free type syntax (`None`), and excludes navigation-only mappings.
    fn editable_span(&self, occurrence: Option<SourceId>) -> Option<Span> {
        self.location(occurrence?)
            .filter(|location| location.editable)
            .map(|location| location.span)
    }
}

/// Type syntax can also be parsed just for lookup, such as a rustdoc link. Such a type has
/// semantic structure but no declaration occurrence that an editor can select.
///
/// Sharing this interface lets the same type lowering handle a written `Vec<User>` and a parsed
/// documentation link to `Vec<User>`. The first records IDs through a declaration builder; the
/// second uses [`NoSource`] and leaves its optional occurrence IDs absent.
pub trait TypeSource {
    fn record_type(&mut self, range: TextRange) -> Option<SourceId>;
}

/// Lower type meaning without creating source occurrences or placeholder IDs.
pub struct NoSource;

impl TypeSource for NoSource {
    fn record_type(&mut self, _range: TextRange) -> Option<SourceId> {
        None
    }
}

/// Capture only short initializers while syntax is available. Keep the text verbatim:
/// collapsing whitespace would change string literals and the extent of line comments.
/// Declaration and body lowering share the same limit, including for current editor text.
pub fn initializer_preview(body: &ast::Expr) -> Option<String> {
    const MAX_BYTES: u32 = 160;
    const MAX_LINES: usize = 3;
    if u32::from(body.syntax().text_range().len()) > MAX_BYTES {
        return None;
    }
    let text = body.syntax().text().to_string();
    (text.lines().count() <= MAX_LINES).then_some(text)
}
