//! Completion sites and request-local recovery inside declaration signatures.

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_parse::{FileId, Span};

use super::{
    IndexedAssociatedTypeBindingScope, IndexedAssociatedTypeBindingSite, IndexedQualifiedPathScope,
    IndexedQualifiedPathSite, IndexedSignatureTypeSite, IndexedTypeNamePosition,
    IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope, IndexedUnqualifiedNameSite,
    SourceCompletionView,
};
use crate::source::scan::{SignatureCompletionSite, SignatureSourceScanner};

impl<'a, 'db> SourceCompletionView<'a, 'db> {
    /// Return a type-path completion site from the current declaration under the cursor.
    ///
    /// The declaration has already been lowered into request-local semantic storage. Its own
    /// generics and impl `Self` come from that storage, while ordinary paths still start lookup in
    /// the saved module around the edited declaration.
    ///
    /// ```text
    /// fn load<CurrentType>(_: CurrentT$0) {} -> offers request-local generic `CurrentType`
    /// impl Service for model::Wor$0 {}        -> resolves `model` in the saved module
    /// fn load(_: Iterator<It$0 = u8>) {}      -> explicit associated binding in current scope
    /// ```
    pub fn current_signature_type_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedSignatureTypeSite>> {
        for origin in self.db.current_signature_origins(crate_ref, file_id)? {
            if let Some(site) =
                SignatureSourceScanner::completion_site_at_origin(self.db, origin, file_id, offset)
                    .context("scan current signature type completion site")?
            {
                return Ok(Some(Self::indexed_signature_type_site(site)));
            }
        }
        Ok(None)
    }

    /// Return a type-path completion site from an item signature.
    ///
    /// This covers declaration-owned syntax such as function parameters, return types, fields,
    /// generic bounds, impl headers, and aliases. The result deliberately reuses the same
    /// qualified/unqualified vocabulary as body completion so candidate rendering has one path.
    ///
    /// ```text
    /// fn load(_: model::Us$0) -> qualified path, qualifier `model`, prefix `Us`
    /// fn load<T>(_: T$0)      -> unqualified name, generic owner `load`, prefix `T`
    /// fn load(_: Iterator<It$0 = u8>) -> explicit associated type binding
    /// ```
    pub fn signature_type_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedSignatureTypeSite>> {
        let site = SignatureSourceScanner::completion_site_at(self.db, crate_ref, file_id, offset)
            .context("scan signature type completion site")?;

        Ok(site.map(Self::indexed_signature_type_site))
    }

    /// Recover a qualified path whose final segment is missing from lowered semantics.
    ///
    /// In `fn load(_: model::Widget::$0) {}`, signature lowering has no final path segment to
    /// retain. The caller supplies `model::Widget` and the empty replacement range from syntax;
    /// this method supplies the request-local declaration's module, impl, and generic context.
    pub fn current_signature_syntax_rich_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        qualifier: &str,
        member_prefix_span: Span,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        let Some(site) = self
            .current_signature_syntax_name_site_at(
                crate_ref,
                file_id,
                offset,
                IndexedUnqualifiedNameContext::Type {
                    position: IndexedTypeNamePosition::Type,
                },
                member_prefix_span,
                String::new(),
            )
            .context("scan current signature qualifier scope")?
        else {
            return Ok(None);
        };
        let IndexedUnqualifiedNameScope::Signature { scope, .. } = site.scope else {
            return Ok(None);
        };
        let Some((module_qualifier, associated_qualifier)) =
            Self::syntax_path_qualifiers(qualifier)
        else {
            return Ok(None);
        };

        Ok(Some(IndexedQualifiedPathSite {
            scope: IndexedQualifiedPathScope::Signature { scope },
            module_qualifier,
            associated_qualifier: Some(associated_qualifier),
            member_prefix_span,
        }))
    }

    /// Pair a request-local qualifier with the declaration signature that owns the cursor.
    ///
    /// This is the signature counterpart of the body recovery path for
    /// `Widget::<u8>::$0`: syntax supplies the incomplete qualifier and this view supplies the
    /// module, impl-`Self`, and generic-owner context needed to resolve it.
    pub fn signature_syntax_rich_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        qualifier: &str,
        member_prefix_span: Span,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        let Some(site) = self
            .signature_syntax_name_site_at(
                crate_ref,
                file_id,
                offset,
                IndexedUnqualifiedNameContext::Type {
                    position: IndexedTypeNamePosition::Type,
                },
                member_prefix_span,
                String::new(),
            )
            .context("scan request-local signature qualifier scope")?
        else {
            return Ok(None);
        };
        let IndexedUnqualifiedNameScope::Signature { scope, .. } = site.scope else {
            return Ok(None);
        };
        let Some((module_qualifier, associated_qualifier)) =
            Self::syntax_path_qualifiers(qualifier)
        else {
            return Ok(None);
        };

        Ok(Some(IndexedQualifiedPathSite {
            scope: IndexedQualifiedPathScope::Signature { scope },
            module_qualifier,
            associated_qualifier: Some(associated_qualifier),
            member_prefix_span,
        }))
    }

    fn indexed_signature_type_site(site: SignatureCompletionSite) -> IndexedSignatureTypeSite {
        match site {
            SignatureCompletionSite::AssociatedTypeBinding {
                scope,
                trait_ref,
                member_prefix_span,
                existing_bindings,
            } => {
                IndexedSignatureTypeSite::AssociatedTypeBinding(IndexedAssociatedTypeBindingSite {
                    scope: IndexedAssociatedTypeBindingScope::Signature {
                        scope: Self::signature_scope(scope),
                    },
                    trait_ref,
                    member_prefix_span,
                    existing_bindings,
                })
            }
            SignatureCompletionSite::Qualified {
                scope,
                module_qualifier,
                associated_qualifier,
                member_prefix_span,
            } => IndexedSignatureTypeSite::Qualified(IndexedQualifiedPathSite {
                scope: IndexedQualifiedPathScope::Signature {
                    scope: Self::signature_scope(scope),
                },
                module_qualifier,
                associated_qualifier: Some(Self::associated_qualifier(associated_qualifier)),
                member_prefix_span,
            }),
            SignatureCompletionSite::Unqualified {
                scope,
                member_prefix_span,
                member_prefix,
                position,
            } => IndexedSignatureTypeSite::Unqualified(IndexedUnqualifiedNameSite {
                scope: IndexedUnqualifiedNameScope::Signature {
                    scope: Self::signature_scope(scope),
                    context: IndexedUnqualifiedNameContext::Type {
                        position: Self::type_name_position(position),
                    },
                    member_prefix,
                },
                member_prefix_span,
            }),
        }
    }

    /// Return current declaration scope when there is no type path to lower.
    ///
    /// In `fn load<CurrentType>(_: $0) {}`, the empty range comes from editor syntax. Selecting
    /// `load` here keeps `CurrentType` visible to the ordinary unqualified-name query.
    pub fn current_signature_empty_type_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        position: IndexedTypeNamePosition,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        let empty_span = Span {
            text: rg_parse::TextSpan {
                start: offset,
                end: offset,
            },
        };
        self.current_signature_syntax_name_site_at(
            crate_ref,
            file_id,
            offset,
            IndexedUnqualifiedNameContext::Type { position },
            empty_span,
            String::new(),
        )
    }

    /// Attach an incomplete spelling from editor syntax to its request-local declaration.
    ///
    /// For `fn load<CurrentType>(_: CurrentT$0) {}`, syntax owns `CurrentT` and its replacement
    /// range. The lowered declaration supplies the function's generic, impl, and module scope.
    pub fn current_signature_syntax_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        for origin in self.db.current_signature_origins(crate_ref, file_id)? {
            let scope = SignatureSourceScanner::empty_type_scope_at_origin(
                self.db, origin, file_id, offset,
            )
            .context("scan current signature completion scope")?;
            if let Some(scope) = scope {
                return Ok(Some(IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Signature {
                        scope: Self::signature_scope(scope),
                        context,
                        member_prefix,
                    },
                    member_prefix_span,
                }));
            }
        }
        Ok(None)
    }

    /// Return a declaration-signature scope for an explicit empty type path.
    ///
    /// In `fn load(_: $0)`, there is no lowered path to select. The empty span comes from syntax
    /// and the signature scanner supplies the function's module and generic-owner context.
    pub fn signature_empty_type_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        position: IndexedTypeNamePosition,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        let empty_span = Span {
            text: rg_parse::TextSpan {
                start: offset,
                end: offset,
            },
        };
        self.signature_syntax_name_site_at(
            crate_ref,
            file_id,
            offset,
            IndexedUnqualifiedNameContext::Type { position },
            empty_span,
            String::new(),
        )
    }

    /// Pair request-local name syntax with the containing declaration signature scope.
    ///
    /// This is the signature equivalent of syntax-to-body attachment: the caller owns the
    /// recovered spelling, while this view selects the narrowest item signature and supplies its
    /// type-path and generic scopes.
    pub fn signature_syntax_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        let scope =
            SignatureSourceScanner::empty_type_scope_at(self.db, crate_ref, file_id, offset)
                .context("scan empty signature type completion scope")?;
        Ok(scope.map(|scope| IndexedUnqualifiedNameSite {
            scope: IndexedUnqualifiedNameScope::Signature {
                scope: Self::signature_scope(scope),
                context,
                member_prefix,
            },
            member_prefix_span,
        }))
    }
}
