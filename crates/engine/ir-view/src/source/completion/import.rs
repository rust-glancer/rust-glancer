//! Qualified, unqualified, and explicitly empty import completion sites.

use anyhow::Context as _;
use rg_ir_model::{CrateRef, Path};
use rg_parse::{FileId, Span};

use super::{
    IndexedQualifiedPathScope, IndexedQualifiedPathSite, IndexedUnqualifiedNameScope,
    IndexedUnqualifiedNameSite, SourceCompletionView,
};
use crate::source::scan::{ImportPathCompletionSiteScanner, ModuleSourceSiteScanner};

impl<'a, 'db> SourceCompletionView<'a, 'db> {
    /// Resolve a qualified current import, such as `use std::sync::Ar$0`, in its saved module.
    pub fn import_syntax_qualified_path_site(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
        qualifier: Path,
        member_prefix_span: Span,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(ModuleSourceSiteScanner::module_for_inline_path(
            &self.db.def_map,
            crate_ref,
            file_id,
            inline_module_path,
        )
        .context("match current import syntax to saved module")?
        .map(|module| IndexedQualifiedPathSite {
            scope: IndexedQualifiedPathScope::Import { module },
            module_qualifier: Some(qualifier),
            associated_qualifier: None,
            member_prefix_span,
        }))
    }

    /// Resolve an unqualified current import, such as `use st$0`, in its saved module.
    pub fn import_syntax_unqualified_name_site(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        inline_module_path: &[String],
        member_prefix_span: Span,
        member_prefix: String,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(ModuleSourceSiteScanner::module_for_inline_path(
            &self.db.def_map,
            crate_ref,
            file_id,
            inline_module_path,
        )
        .context("match current import syntax to saved module")?
        .map(|module| IndexedUnqualifiedNameSite {
            scope: IndexedUnqualifiedNameScope::Import {
                module,
                member_prefix,
            },
            member_prefix_span,
        }))
    }

    /// Return an import qualified-path site at a cursor offset, e.g. `use model::Us$0;`.
    pub fn import_qualified_path_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedQualifiedPathSite>> {
        Ok(
            ImportPathCompletionSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .qualified_site()
                .context("scan qualified import completion site")?
                .map(|site| IndexedQualifiedPathSite {
                    scope: IndexedQualifiedPathScope::Import {
                        module: site.module,
                    },
                    module_qualifier: Some(site.qualifier),
                    associated_qualifier: None,
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return an import unqualified-name site at a cursor offset, e.g. `use cr$0;`.
    pub fn import_unqualified_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(
            ImportPathCompletionSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .unqualified_site()
                .context("scan unqualified import completion site")?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Import {
                        module: site.module,
                        member_prefix: site.member_prefix,
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }

    /// Return the containing module for an explicit empty import path.
    pub fn import_empty_name_site_at(
        &self,
        crate_ref: CrateRef,
        file_id: FileId,
        offset: u32,
    ) -> anyhow::Result<Option<IndexedUnqualifiedNameSite>> {
        Ok(
            ImportPathCompletionSiteScanner::new(&self.db.def_map, crate_ref, file_id, offset)
                .empty_unqualified_site()
                .context("scan empty import completion site")?
                .map(|site| IndexedUnqualifiedNameSite {
                    scope: IndexedUnqualifiedNameScope::Import {
                        module: site.module,
                        member_prefix: String::new(),
                    },
                    member_prefix_span: site.member_prefix_span,
                }),
        )
    }
}
