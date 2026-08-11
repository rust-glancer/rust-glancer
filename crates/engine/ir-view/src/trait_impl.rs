//! Missing-member projections for resolved trait implementations.
//!
//! Semantic IR owns associated declarations and the type engine owns instantiated signatures.
//! This view joins those facts so editor features can present implementation scaffolds without
//! reparsing display text or approximating trait substitutions in the analysis layer.
//!
//! ```text
//! trait Service<T> {
//!     type Output;
//!     const LIMIT: usize;
//!     fn required(&self, value: T) -> Self::Output;
//!     fn defaulted(&self) {}
//! }
//!
//! struct Worker;
//!
//! impl Service<u8> for Worker {
//!     fn re$0
//! }
//! ```
//!
//! The missing-member projection retains all four declaration identities, renders `required`
//! with `T` substituted to `u8`, and records which members have no trait default. The
//! editor-facing layer uses the written `fn` to keep function members, then replaces the
//! incomplete `fn re` declaration with the selected snippet body.

use anyhow::Context as _;
use rg_ir_model::{
    AssocItemId, ConstRef, DefMapRef, FunctionRef, GenericDefRef, ImplRef, TraitDefRef,
    TypeAliasRef,
};
use rg_item_tree::Documentation;
use rg_semantic_ir::{GenericsQuery, ItemStoreQuery};
use rg_ty::{SemanticSignatureQuery, Substitution};

use crate::{
    IndexedViewDb,
    display::{signature::SignatureRenderer, ty_label::TypeRenderer},
};

/// Stable trait declaration represented by one missing implementation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingTraitMemberRef {
    Function(FunctionRef),
    TypeAlias(TypeAliasRef),
    Const(ConstRef),
}

/// Syntax-shaped scaffold text before the caller chooses placeholders and bodies.
///
/// Function and const signatures stop before their body or value. Associated types keep the
/// declaration prefix and suggested value separate, for example `type Output` plus `()`, so an
/// editor can place a tab stop around only the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingTraitMemberScaffold {
    Function {
        signature: String,
    },
    TypeAlias {
        signature_prefix: String,
        suggested_value: String,
    },
    Const {
        signature: String,
    },
}

/// One direct trait member that has no same-kind, same-name item in the selected impl.
///
/// Members with trait defaults are included because an impl may still override them. `required`
/// distinguishes declarations with no function body, associated type default, or const value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingTraitMember {
    member: MissingTraitMemberRef,
    label: String,
    required: bool,
    scaffold: MissingTraitMemberScaffold,
    documentation: Option<String>,
}

impl MissingTraitMember {
    pub fn member(&self) -> MissingTraitMemberRef {
        self.member
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    pub fn scaffold(&self) -> &MissingTraitMemberScaffold {
        &self.scaffold
    }

    pub fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssociatedMemberKey {
    Function(String),
    TypeAlias(String),
    Const(String),
}

/// Projects canonical trait signatures into one concrete implementation context.
pub struct TraitImplView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> TraitImplView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return direct trait members not yet implemented by the selected impl.
    ///
    /// Matching uses `(member kind, name)` so an associated const cannot accidentally satisfy a
    /// same-named function. Signatures and default values are rendered after applying the concrete
    /// trait substitution from the impl header.
    pub fn missing_members(
        &self,
        impl_ref: ImplRef,
        trait_ref: TraitDefRef,
    ) -> anyhow::Result<Vec<MissingTraitMember>> {
        // First verify that the source site and semantic impl header identify the same trait.
        // Cursor recovery can hand this view stale or incomplete source during active editing.
        let items = ItemStoreQuery::new(self.db);
        let Some(impl_data) = items
            .impl_data(impl_ref)
            .context("read trait impl completion owner")?
        else {
            return Ok(Vec::new());
        };
        if !impl_data.resolved_trait_ref.is(&trait_ref) {
            return Ok(Vec::new());
        }
        let Some(trait_data) = items
            .trait_data(trait_ref)
            .context("read implemented trait members")?
        else {
            return Ok(Vec::new());
        };

        // Lowering the header produces the substitution that turns trait-owned `T` and `Self`
        // references into syntax appropriate for this concrete impl.
        let signatures = SemanticSignatureQuery::new(self.db, self.db);
        let Some(header) = signatures
            .impl_header(impl_ref)
            .context("lower trait impl completion header")?
        else {
            return Ok(Vec::new());
        };
        let Some(trait_lowering) = header.trait_ref else {
            return Ok(Vec::new());
        };
        let application = trait_lowering.application;
        if application.def != trait_ref {
            return Ok(Vec::new());
        }
        let generics = GenericsQuery::new(self.db)
            .generics(GenericDefRef::Trait(trait_ref))
            .context("read implemented trait generics")?;
        let substitution = Substitution::from_args(&generics, &application.args);

        // Record only semantic identity needed for absence checks. The impl's syntax may be
        // incomplete, but successfully lowered members still suppress duplicate scaffolds.
        let mut existing = Vec::new();
        for item in &impl_data.items {
            if let Some(key) = self
                .member_key(impl_ref.origin, *item)
                .context("read existing trait impl member")?
            {
                existing.push(key);
            }
        }

        // Project the remaining trait declarations through the impl substitution. The view keeps
        // scaffold text separate from snippet policy so non-LSP consumers can reuse the result.
        let edition = self
            .db
            .origin_edition(impl_ref.origin)
            .context("read trait impl completion edition")?;
        let signatures_renderer = SignatureRenderer::new(edition);
        let types = TypeRenderer::new(self.db, edition);
        let mut missing = Vec::new();
        for item in &trait_data.items {
            let Some(key) = self
                .member_key(trait_ref.origin, *item)
                .context("read trait member completion identity")?
            else {
                continue;
            };
            if existing.iter().any(|candidate| candidate == &key) {
                continue;
            }

            let candidate = match *item {
                AssocItemId::Function(id) => {
                    let function = FunctionRef {
                        origin: trait_ref.origin,
                        id,
                    };
                    let Some(data) = items
                        .function_data(function)
                        .context("read missing trait function")?
                    else {
                        continue;
                    };
                    let Some(semantic) = signatures
                        .function(function)
                        .context("lower missing trait function signature")?
                    else {
                        continue;
                    };
                    MissingTraitMember {
                        member: MissingTraitMemberRef::Function(function),
                        label: data.name.to_string(),
                        required: !data.signature.has_body(),
                        scaffold: MissingTraitMemberScaffold::Function {
                            signature: signatures_renderer
                                .trait_impl_function_signature(
                                    self.db,
                                    data,
                                    &semantic,
                                    &substitution,
                                    &application,
                                )
                                .context("render missing trait function signature")?,
                        },
                        documentation: data.docs.as_ref().map(Documentation::text),
                    }
                }
                AssocItemId::TypeAlias(id) => {
                    let alias = TypeAliasRef {
                        origin: trait_ref.origin,
                        id,
                    };
                    let Some(data) = items
                        .type_alias_data(alias)
                        .context("read missing associated type")?
                    else {
                        continue;
                    };
                    let required = data.signature.aliased_ty().is_none();
                    let suggested_value = if required {
                        "()".to_string()
                    } else {
                        let ty = signatures
                            .type_alias_ty(alias)
                            .context("lower default associated type")?
                            .map(|ty| substitution.apply(&ty));
                        ty.as_ref()
                            .map(|ty| types.render_trait_impl_ty(ty, &application))
                            .transpose()
                            .context("render default associated type")?
                            .flatten()
                            .unwrap_or_else(|| "()".to_string())
                    };
                    let signature_prefix = signatures_renderer.trait_impl_type_alias_prefix(data);
                    MissingTraitMember {
                        member: MissingTraitMemberRef::TypeAlias(alias),
                        label: data.name.to_string(),
                        required,
                        scaffold: MissingTraitMemberScaffold::TypeAlias {
                            signature_prefix,
                            suggested_value,
                        },
                        documentation: data.docs.as_ref().map(Documentation::text),
                    }
                }
                AssocItemId::Const(id) => {
                    let konst = ConstRef {
                        origin: trait_ref.origin,
                        id,
                    };
                    let Some(data) = items
                        .const_data(konst)
                        .context("read missing associated const")?
                    else {
                        continue;
                    };
                    let ty = signatures
                        .const_ty(konst)
                        .context("lower missing associated const type")?
                        .map(|ty| substitution.apply(&ty));
                    let ty = ty
                        .as_ref()
                        .map(|ty| types.render_trait_impl_ty(ty, &application))
                        .transpose()
                        .context("render missing associated const type")?
                        .flatten()
                        .unwrap_or_else(|| "_".to_string());
                    MissingTraitMember {
                        member: MissingTraitMemberRef::Const(konst),
                        label: data.name.to_string(),
                        required: !data.signature.has_value(),
                        scaffold: MissingTraitMemberScaffold::Const {
                            signature: signatures_renderer.trait_impl_const_signature(data, &ty),
                        },
                        documentation: data.docs.as_ref().map(Documentation::text),
                    }
                }
            };
            missing.push(candidate);
        }

        Ok(missing)
    }

    fn member_key(
        &self,
        origin: DefMapRef,
        item: AssocItemId,
    ) -> anyhow::Result<Option<AssociatedMemberKey>> {
        let items = ItemStoreQuery::new(self.db);
        Ok(match item {
            AssocItemId::Function(id) => items
                .function_data(FunctionRef { origin, id })
                .context("read associated function name")?
                .map(|data| AssociatedMemberKey::Function(data.name.to_string())),
            AssocItemId::TypeAlias(id) => items
                .type_alias_data(TypeAliasRef { origin, id })
                .context("read associated type name")?
                .map(|data| AssociatedMemberKey::TypeAlias(data.name.to_string())),
            AssocItemId::Const(id) => items
                .const_data(ConstRef { origin, id })
                .context("read associated const name")?
                .map(|data| AssociatedMemberKey::Const(data.name.to_string())),
        })
    }
}
