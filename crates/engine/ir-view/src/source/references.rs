//! Select source files and bodies that could contain references to a declaration.
//!
//! A spelling can rule out an entire file before any semantic scan. Within a selected file,
//! declaration metadata and source tokens can rule out individual bodies before inference.

use std::collections::{HashMap, HashSet};

use rg_def_map::{DefMapSource as _, ModuleOrigin};
use rg_ir_model::{
    CrateRef, DefMapRef, FileId, ImplRef, ItemStoreId, ModuleRef, OuterBodyOwner, Span, TypeDefRef,
};
use rg_semantic_ir::{BodyOwnerData, ItemStoreQuery, SourceSelection};
use rg_syntax::{
    SyntaxKind,
    ast::{self, AstToken as _, FormatStringPartKind},
};

use crate::IndexedViewDb;

/// A spelling that can lead to a written reference. Ordinary names can be imported by another
/// crate. Relative module paths start in the crate interpreting the source, even when two targets
/// include the same file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferenceSearchLabel {
    spelling: String,
    only_in: Option<CrateRef>,
}

impl ReferenceSearchLabel {
    /// The text prefilter recognizes ASCII identifier boundaries. Other spellings require an
    /// unrestricted scan.
    pub fn identifier(spelling: &str) -> Option<Self> {
        let mut bytes = spelling.bytes();
        let first = bytes.next()?;
        if (first == b'_' || first.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            Some(Self {
                spelling: spelling.to_owned(),
                only_in: None,
            })
        } else {
            None
        }
    }

    /// `mod outer` can be referenced as `outer` in a dependent, as `self` inside `outer`, or as
    /// `super` in a child. Use the module's semantic crate for the keywords so an include or
    /// macro expansion needs no special routing. An unknown name disables spelling-based filtering.
    pub fn for_module(db: &IndexedViewDb<'_>, module_ref: ModuleRef) -> anyhow::Result<Vec<Self>> {
        let Some(module) = db.module_data(module_ref)? else {
            return Ok(Vec::new());
        };
        let krate = module_ref.origin.origin_crate();
        let name = match module.origin {
            ModuleOrigin::Root { .. } => db.def_map.crate_metadata(krate)?.map(|data| data.name()),
            ModuleOrigin::Inline { .. } | ModuleOrigin::OutOfLine { .. } => {
                module.name.as_ref().map(|name| name.as_str())
            }
            ModuleOrigin::Synthetic { .. } => None,
        };
        let Some(name) = name.and_then(Self::identifier) else {
            return Ok(Vec::new());
        };
        let mut labels = vec![name];
        if matches!(module.origin, ModuleOrigin::Root { .. }) {
            // The byte search also finds `crate` in `$crate`, which can refer back to the
            // defining crate of an exported macro expanded in a dependent.
            labels.push(Self {
                spelling: "crate".to_owned(),
                only_in: None,
            });
        }
        labels.extend(["self", "super"].map(|spelling| Self {
            spelling: spelling.to_owned(),
            only_in: Some(krate),
        }));
        Ok(labels)
    }

    pub fn as_str(&self) -> &str {
        &self.spelling
    }

    pub fn spelling_in(&self, krate: CrateRef) -> Option<&str> {
        self.only_in
            .is_none_or(|owner| owner == krate)
            .then_some(self.as_str())
    }
}

/// Hints from the declarations selected by a reference or rename search.
///
/// For references to `struct Widget`, labels can include both `Widget` and `Self`. A method on
/// `Widget` that only writes `Self` still needs scanning; an unrelated function containing neither
/// spelling can stay uncomputed. Module and type identities help distinguish uses of common
/// keywords without inferring every body containing `Self`, `self`, or `super`.
///
/// Keeping extra bodies is fine, but dropping a possible reference is not. When the written source
/// or enclosing scope cannot rule out a body, keep it for semantic matching. A selected body may
/// still contain no references to the searched declaration.
pub struct ReferenceSourceSelection {
    /// Possible source spellings, such as `["Widget", "Self"]`. Empty means no spelling filter.
    pub labels: Vec<ReferenceSearchLabel>,
    /// Searched modules, used to narrow `self`, `super`, and `crate` paths.
    pub modules: Vec<ModuleRef>,
    /// Searched nominal types, used to narrow `Self` in methods with a known impl receiver.
    pub types: Vec<TypeDefRef>,
}

impl ReferenceSourceSelection {
    /// Choose outer owners in one file; selecting one also includes its nested bodies.
    ///
    /// This reads declaration metadata and syntax, never inferred bodies.
    pub(crate) fn owners(
        &self,
        db: &IndexedViewDb<'_>,
        krate: CrateRef,
        file: FileId,
    ) -> anyhow::Result<HashSet<OuterBodyOwner>> {
        let owners = db.body_ir.owners(krate, SourceSelection::AllInFile(file))?;
        if owners.clone().next().is_none() {
            return Ok(HashSet::new());
        }
        let mut selected = HashSet::new();
        if self.labels.is_empty() {
            return Ok(owners.clone().map(|owner| owner.owner).collect());
        }
        let labels: Vec<_> = self
            .labels
            .iter()
            .filter_map(|label| label.spelling_in(krate))
            .collect();
        if labels.is_empty() {
            return Ok(selected);
        }
        // Keep generated owners even when the text lacks a label. `include!` also prevents this
        // shortcut: the included file may contain references absent from the invocation's text.
        let text = db.sources().text(file)?;
        // A capture such as `caf\u{e9}` need not contain its decoded name in the file text.
        // Escapes require the syntax pass below before we can rule out its references.
        if !labels.iter().any(|label| text.contains(*label))
            && !text.contains("include")
            && !text.contains('\\')
        {
            return Ok(owners
                .clone()
                .filter(|owner| owner.source.as_item_tree().is_none())
                .map(|owner| owner.owner)
                .collect());
        }
        let syntax = db.source_syntax(krate, file)?;
        let mut tokens: HashMap<String, Vec<Span>> = HashMap::new();
        let mut macro_arguments: HashMap<String, Vec<Span>> = HashMap::new();
        // Token equality ignores comments and ordinary string text while retaining identifiers
        // inside macro arguments. Their expansion may use the argument in several semantic roles.
        // Also record `impl`, `mod`, and `include`: they can introduce a new scope or more source,
        // so finding them below may prevent us from ruling out an owner by its enclosing scope.
        for token in syntax
            .syntax_node()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
        {
            rg_std::check_cancel!(db, "reference source selection");
            // A literal may become a format template after macro expansion. Recognize names with
            // the same parser as body lowering, including raw strings, escapes, width and precision.
            // This can keep extra owners; semantic scanning decides whether they are actual captures.
            if let Some(string) = ast::String::cast(token.clone()) {
                string.format_parts(|part| {
                    let FormatStringPartKind::Name(name) = part.kind else {
                        return;
                    };
                    if labels.contains(&name) {
                        let span = Span::from_text_range(part.range + token.text_range().start());
                        tokens.entry(name.to_owned()).or_default().push(span);
                        macro_arguments
                            .entry(name.to_owned())
                            .or_default()
                            .push(span);
                    }
                });
            }
            let text = token.text();
            // Raw identifiers such as `r#type` must match the label `type` too.
            let text = text.strip_prefix("r#").unwrap_or(text);
            if labels.contains(&text) || matches!(text, "impl" | "mod" | "include") {
                let span = Span {
                    start: token.text_range().start().into(),
                    end: token.text_range().end().into(),
                };
                tokens.entry(text.to_owned()).or_default().push(span);
                if token
                    .parent_ancestors()
                    .any(|node| node.kind() == SyntaxKind::TOKEN_TREE)
                {
                    macro_arguments
                        .entry(text.to_owned())
                        .or_default()
                        .push(span);
                }
            }
        }
        for owner in owners.clone() {
            rg_std::check_cancel!(db, "reference owner selection");
            let span = db.source_span(DefMapRef::Crate(krate), owner.source, owner.location)?;
            // Generated owners and included source cannot be ruled out from the invocation's
            // written tokens. Keep them even without a matching label in the owner's span.
            if owner.source.as_item_tree().is_none() || Self::contains(&tokens, "include", span) {
                selected.insert(owner.owner);
                continue;
            }
            for &label in &labels {
                if !Self::contains(&tokens, label, span) {
                    continue;
                }
                // A macro can move an argument into a different impl or module. Written owner
                // context proves nothing about the expansion's interpretation of that token.
                if Self::contains(&macro_arguments, label, span) {
                    selected.insert(owner.owner);
                    break;
                }
                // `Self` and relative module paths depend on where the token is written. A nested
                // impl or module may change that scope, so only use the outer scope without one.
                let plausible = match label {
                    "Self" if !Self::contains(&tokens, "impl", span) => {
                        self.accepts_self(db, &owner)?
                    }
                    "self" | "super" | "crate"
                        if !self.modules.is_empty() && !Self::contains(&tokens, "mod", span) =>
                    {
                        self.accepts_module(db, owner.module, label)?
                    }
                    _ => true,
                };
                if plausible {
                    selected.insert(owner.owner);
                    break;
                }
            }
        }
        Ok(selected)
    }

    /// Check whether the owner contains a token with this label. Spans follow source order, so
    /// checking the first token at or after the owner's start is enough. If it ends past the
    /// owner, no later token can fit inside either.
    fn contains(tokens: &HashMap<String, Vec<Span>>, label: &str, owner: Span) -> bool {
        tokens.get(label).is_some_and(|spans| {
            let first = spans.partition_point(|span| span.start < owner.start);
            spans.get(first).is_some_and(|span| span.end <= owner.end)
        })
    }

    /// Check whether a method's enclosing impl can give `Self` the searched nominal type.
    ///
    /// With no nested impl, `Self` in `impl Widget { fn new() -> Self { ... } }` may refer to
    /// `Widget`, but the same method on a known different type cannot. Non-method owners and
    /// unresolved receiver types stay eligible because this check cannot establish their meaning.
    fn accepts_self(&self, db: &IndexedViewDb<'_>, owner: &BodyOwnerData) -> anyhow::Result<bool> {
        if self.types.is_empty() {
            return Ok(true);
        }
        let OuterBodyOwner::Function {
            store: ItemStoreId::Impl(id),
            ..
        } = owner.owner
        else {
            return Ok(true);
        };
        let Some(data) =
            ItemStoreQuery::new(db).impl_data(ImplRef::new(owner.module.origin, id))?
        else {
            return Ok(true);
        };
        // Only a uniquely resolved nominal receiver can disprove the match. Missing or ambiguous
        // receiver information leaves the owner eligible for semantic scanning.
        Ok(data
            .resolved_self_ty
            .as_option()
            .is_none_or(|ty| self.types.contains(ty)))
    }

    /// Check which searched module a keyword could name from the owner's enclosing module.
    /// From `crate::outer::inner`, `self` starts at `inner`, `super` at `outer`, and `crate` at root.
    fn accepts_module(
        &self,
        db: &IndexedViewDb<'_>,
        mut module: ModuleRef,
        label: &str,
    ) -> anyhow::Result<bool> {
        match label {
            "super" => loop {
                let Some(data) = db.module_data(module)? else {
                    return Ok(true);
                };
                let Some(parent) = data.parent else {
                    return Ok(false);
                };
                module.module = parent;
                // `super::super` can name a more distant ancestor. The semantic path scanner
                // decides which segment refers to it; discovery retains every plausible owner.
                if self.modules.contains(&module) {
                    return Ok(true);
                }
            },
            "crate" => loop {
                let Some(data) = db.module_data(module)? else {
                    return Ok(true);
                };
                let Some(parent) = data.parent else {
                    break;
                };
                module.module = parent;
            },
            _ => {}
        }
        Ok(self.modules.contains(&module))
    }
}
