//! Qualified, unqualified, and explicitly empty import completion sites.

use anyhow::Context as _;
use rg_ir_model::CrateRef;
use rg_parse::FileId;

use super::{
    IndexedQualifiedPathScope, IndexedQualifiedPathSite, IndexedUnqualifiedNameScope,
    IndexedUnqualifiedNameSite, SourceCompletionView,
};
use crate::source::scan::ImportPathCompletionSiteScanner;

impl<'a, 'db> SourceCompletionView<'a, 'db> {
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
