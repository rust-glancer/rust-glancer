//! Completion sites backed by Body IR and request-local body-scope recovery.

use anyhow::Context as _;
use rg_ir_model::{
    BodyBindingRef, CrateRef, Path,
    identity::{ExprRef, LexicalScopeRef},
};
use rg_parse::{FileId, Span};
use rg_semantic_ir::TypePathResolution;

use super::{
    IndexedAssociatedTypeBindingScope, IndexedAssociatedTypeBindingSite, IndexedMemberAccessSite,
    IndexedQualifiedPathContext, IndexedQualifiedPathScope, IndexedQualifiedPathSite,
    IndexedRecordFieldListSite, IndexedRecordOwner, IndexedTypeNamePosition,
    IndexedUnqualifiedNameContext, IndexedUnqualifiedNameScope, IndexedUnqualifiedNameSite,
    SourceCompletionView,
};
use crate::{
    body::BodyResolutionView,
    source::scan::{
        BodyUnqualifiedNameContext, DotCompletionSiteScanner, LabelScopeScanner,
        PathCompletionSiteScanner, RecordFieldCompletionSiteScanner, TypeNamePosition,
        UnqualifiedCompletionSiteScanner,
    },
};

impl<'a, 'db> SourceCompletionView<'a, 'db> {
    /// Return labels declared by loop or block expressions enclosing the cursor.
    ///
    /// Names are ordered from the nearest jump target outward. If an inner target shadows an outer
    /// target with the same spelling, that spelling appears only once.
    pub fn enclosing_labels_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Vec<String>> {
        LabelScopeScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
            .labels()
            .context("scan enclosing loop labels")
    }

    /// Return the member-access site at a cursor offset, e.g. `items.pu$0`.
    pub fn member_access_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedMemberAccessSite>> {
        Ok(
            DotCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_dot()
                .context("scan member access completion site")?
                .map(|site| IndexedMemberAccessSite {
                    receiver: ExprRef::new(site.body, site.receiver),
                    receiver_span: site.receiver_span,
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a body qualified-path site at a cursor offset, e.g. `model::Us$0` in an expression.
    pub fn body_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(
            PathCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_path()
                .context("scan body qualified path completion site")?
                .map(|site| IndexedQualifiedPathSite {
                    scope: IndexedQualifiedPathScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                        context: Self::qualified_path_context(site.context),
                    },
                    module_qualifier: site.module_qualifier,
                    associated_qualifier: Some(Self::associated_qualifier(
                        site.associated_qualifier,
                    )),
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return a body-owned binding whose `=` is already present.
    ///
    /// `Iterator<It$0 = u8>` is unambiguously an associated type binding, so this result owns the
    /// completion position instead of being combined with ordinary type-argument candidates.
    /// Signature-owned explicit bindings travel through `signature_type_site_at` with the other
    /// signature path shapes.
    pub fn body_associated_type_binding_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedAssociatedTypeBindingSite>> {
        Ok(
            PathCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .associated_type_binding_site_at()
                .context("scan associated type binding completion site")?
                .map(|site| IndexedAssociatedTypeBindingSite {
                    scope: IndexedAssociatedTypeBindingScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                    },
                    trait_ref: site.trait_ref,
                    member_prefix_span: site.member_prefix_span,
                    existing_bindings: site.existing_bindings,
                }),
        )
    }

    /// Return a body unqualified-name site at a cursor offset, e.g. `let value = inp$0;`.
    pub fn body_unqualified_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(
            UnqualifiedCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_name()
                .context("scan unqualified body completion site")?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                        context: match site.context {
                            BodyUnqualifiedNameContext::Type(position) => {
                                IndexedUnqualifiedNameContext::Type {
                                    position: Self::type_name_position(position),
                                }
                            }
                            BodyUnqualifiedNameContext::Value => {
                                IndexedUnqualifiedNameContext::Value
                            }
                            BodyUnqualifiedNameContext::Const => {
                                IndexedUnqualifiedNameContext::Const
                            }
                            BodyUnqualifiedNameContext::Pattern(kind) => {
                                IndexedUnqualifiedNameContext::Pattern(Self::pattern_kind(kind))
                            }
                        },
                        member_prefix: site.member_prefix,
                        generic_owner: site.generic_owner,
                        expected_type_binding: site.expected_type_binding.map(|binding| {
                            BodyBindingRef {
                                body: site.body,
                                binding,
                            }
                        }),
                        visible_bindings: site.visible_bindings,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return an unqualified body site for a syntax-classified explicit empty path.
    pub fn body_empty_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        let empty_span = Span {
            text: rg_parse::TextSpan {
                start: offset,
                end: offset,
            },
        };
        self.body_syntax_name_site_at(
            crate_ref,
            file_id,
            offset,
            context,
            empty_span,
            String::new(),
        )
    }

    /// Pair request-local name syntax with the nearest indexed body scope.
    ///
    /// This is used when syntax recovery can see an identifier but Body IR has no ordinary path
    /// node for it, for example inside an incomplete const expression. The caller owns the text
    /// and replacement span; this view contributes lexical and generic scope facts.
    pub fn body_syntax_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        self.body_syntax_name_site(
            crate_ref,
            file_id,
            offset,
            context,
            member_prefix_span,
            member_prefix,
            None,
        )
    }

    /// Pair request-local syntax with a declaration body identified before recovery changed its
    /// end.
    ///
    /// An unfinished declaration can make the indexed body end before the cursor. In that case the
    /// caller supplies the owner's source start, which is stable enough to reconnect the recovered
    /// spelling to the intended body.
    #[allow(clippy::too_many_arguments)]
    pub fn body_syntax_name_site_in_owner_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
        body_owner_start: Option<u32>,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        self.body_syntax_name_site(
            crate_ref,
            file_id,
            offset,
            context,
            member_prefix_span,
            member_prefix,
            body_owner_start,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn body_syntax_name_site(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        context: IndexedUnqualifiedNameContext,
        member_prefix_span: Span,
        member_prefix: String,
        body_owner_start: Option<u32>,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        let body_context = match context {
            IndexedUnqualifiedNameContext::Type { position } => {
                BodyUnqualifiedNameContext::Type(match position {
                    IndexedTypeNamePosition::Type => TypeNamePosition::Type,
                    IndexedTypeNamePosition::BareGenericArgument => {
                        TypeNamePosition::BareGenericArgument
                    }
                })
            }
            IndexedUnqualifiedNameContext::Value => BodyUnqualifiedNameContext::Value,
            IndexedUnqualifiedNameContext::Const => BodyUnqualifiedNameContext::Const,
            IndexedUnqualifiedNameContext::Pattern(_) => return Ok(None),
        };
        Ok(
            UnqualifiedCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_syntax_name(
                    body_context,
                    member_prefix_span,
                    member_prefix,
                    body_owner_start,
                )
                .context("scan syntax-classified body completion site")?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Body {
                        scope: LexicalScopeRef::new(site.body, site.scope),
                        context,
                        member_prefix: site.member_prefix,
                        generic_owner: site.generic_owner,
                        expected_type_binding: None,
                        visible_bindings: site.visible_bindings,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Pair a syntax-owned qualified macro callee with the nearest indexed body scope.
    ///
    /// Macro syntax can retain `tools::em$0!()` even when the incomplete callee did not become an
    /// ordinary Body IR path. The caller supplies that path spelling; this method contributes only
    /// the body scope and value-path context.
    pub fn body_syntax_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        qualifier: Path,
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        let Some(site) = self
            .body_syntax_name_site_at(
                crate_ref,
                file_id,
                offset,
                IndexedUnqualifiedNameContext::Value,
                member_prefix_span,
                member_prefix,
            )
            .context("scan qualified macro callee body scope")?
        else {
            return Ok(None);
        };
        let IndexedUnqualifiedNameScope::Body { scope, .. } = site.scope else {
            return Ok(None);
        };
        Ok(Some(IndexedQualifiedPathSite {
            scope: IndexedQualifiedPathScope::Body {
                scope,
                context: IndexedQualifiedPathContext::Value,
            },
            module_qualifier: Some(qualifier),
            associated_qualifier: None,
            member_prefix_span,
        }))
    }

    /// Pair a request-local type-shaped qualifier with the nearest indexed body scope.
    ///
    /// This handles a trailing separator such as `Widget::<u8>::$0`, where the incomplete final
    /// segment may not exist in Body IR. The qualifier is parsed for this request so generic
    /// arguments and `<T as Trait>` anchors survive associated-item lookup.
    #[allow(clippy::too_many_arguments)]
    pub fn body_syntax_rich_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        qualifier: &str,
        context: IndexedQualifiedPathContext,
        member_prefix_span: Span,
        body_owner_start: Option<u32>,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        let Some(site) = self
            .body_syntax_name_site_in_owner_at(
                crate_ref,
                file_id,
                offset,
                IndexedUnqualifiedNameContext::Value,
                member_prefix_span,
                String::new(),
                body_owner_start,
            )
            .context("scan request-local qualified path body scope")?
        else {
            return Ok(None);
        };
        let IndexedUnqualifiedNameScope::Body { scope, .. } = site.scope else {
            return Ok(None);
        };
        let Some((module_qualifier, associated_qualifier)) =
            Self::syntax_path_qualifiers(qualifier)
        else {
            return Ok(None);
        };

        Ok(Some(IndexedQualifiedPathSite {
            scope: IndexedQualifiedPathScope::Body { scope, context },
            module_qualifier,
            associated_qualifier: Some(associated_qualifier),
            member_prefix_span,
        }))
    }

    /// Recover an empty record field list that has no indexed field node yet.
    ///
    /// For an actively typed `User { $0`, syntax supplies `User` and the empty replacement span.
    /// This view attaches the nearest body scope and resolves whether that owner is a type or an
    /// enum variant.
    pub fn record_syntax_field_list_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
        owner: Path,
        member_prefix_span: Span,
        body_owner_start: Option<u32>,
    ) -> anyhow::Result<Option<IndexedRecordFieldListSite>> {
        let site = self
            .body_syntax_name_site_in_owner_at(
                crate_ref,
                file_id,
                offset,
                IndexedUnqualifiedNameContext::Value,
                member_prefix_span,
                String::new(),
                body_owner_start,
            )
            .context("scan syntax-owned record body scope")?;
        let Some(site) = site else {
            return Ok(None);
        };
        let IndexedUnqualifiedNameScope::Body { scope, .. } = site.scope else {
            return Ok(None);
        };
        let Some(owner) = self
            .record_owner(scope, &owner)
            .context("resolve syntax-owned record owner")?
        else {
            return Ok(None);
        };

        Ok(Some(IndexedRecordFieldListSite {
            scope,
            owner,
            member_prefix_span,
            existing_fields: Vec::new(),
        }))
    }

    /// Return a record field-list site retained by Body IR, e.g. `User { name, na$0 }`.
    ///
    /// Unlike request-local empty-list recovery, this path also retains the fields already written
    /// in the literal or pattern.
    pub fn record_field_list_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedRecordFieldListSite>> {
        let Some(site) =
            RecordFieldCompletionSiteScanner::new(&self.db.body_ir, crate_ref, file_id, offset)
                .site_at_record_field()
                .context("scan record field completion site")?
        else {
            return Ok(None);
        };

        // Resolve the owner while the body scope and original path still travel together. This is
        // the point where a record-shaped path can be classified without guessing from its text.
        let scope = LexicalScopeRef::new(site.body, site.scope);
        let owner = self
            .record_owner(scope, &site.owner)
            .context("resolve indexed record owner")?;
        let Some(owner) = owner else {
            return Ok(None);
        };

        Ok(Some(IndexedRecordFieldListSite {
            scope,
            owner,
            member_prefix_span: site.member_prefix_span,
            existing_fields: site.existing_fields,
        }))
    }

    fn record_owner(
        &self,
        scope: LexicalScopeRef,
        owner: &Path,
    ) -> anyhow::Result<Option<IndexedRecordOwner>> {
        let resolution = BodyResolutionView::new(self.db)
            .type_path_resolution(scope.body_ir(), scope.scope_id(), owner)
            .context("resolve record owner type path")?;
        Ok(match resolution {
            Some(TypePathResolution::SelfType(owner) | TypePathResolution::TypeDef(owner)) => {
                Some(IndexedRecordOwner::Type(owner))
            }
            Some(
                TypePathResolution::TypeAlias(_)
                | TypePathResolution::Trait(_)
                | TypePathResolution::Unknown,
            )
            | None => BodyResolutionView::new(self.db)
                .type_path_enum_variant(scope.body_ir(), scope.scope_id(), owner)
                .context("resolve record owner enum variant")?
                .map(IndexedRecordOwner::EnumVariant),
        })
    }
}
