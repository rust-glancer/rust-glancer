//! Builds and rebuilds frozen def-map snapshots.
//!
//! Target collection intentionally stops before cross-target facts such as implicit roots,
//! preludes, and imports are fully known. Clean builds and package rebuilds now share one
//! finalization model:
//! - packages with fresh `TargetState`s are "dirty" and receive fixed-point import resolution;
//! - packages without fresh states are read from an optional frozen baseline;
//! - a clean build has no baseline and marks every package dirty;
//! - a package rebuild has an old baseline and marks only affected packages dirty.

mod clean;
mod finalize;
mod implicit_roots;
mod imports;
mod rebuild;

use rg_item_tree::ItemTreeDb;
use rg_text::PackageNameInterners;
use rg_workspace::WorkspaceMetadata;

use crate::{DefMapDb, DefMapReadTxn, PackageSlot};

/// Builder for a fresh def-map snapshot.
pub struct DefMapDbBuilder<'a, 'names> {
    workspace: &'a WorkspaceMetadata,
    parse: &'a rg_parse::ParseDb,
    item_tree: &'a ItemTreeDb,
    interners: NameInternerSource<'names>,
}

impl<'a> DefMapDbBuilder<'a, 'static> {
    pub(crate) fn new(
        workspace: &'a WorkspaceMetadata,
        parse: &'a rg_parse::ParseDb,
        item_tree: &'a ItemTreeDb,
    ) -> Self {
        DefMapDbBuilder {
            workspace,
            parse,
            item_tree,
            interners: NameInternerSource::Owned(PackageNameInterners::new(parse.package_count())),
        }
    }
}

impl<'a, 'names> DefMapDbBuilder<'a, 'names> {
    pub fn name_interners(
        self,
        interners: &'names mut PackageNameInterners,
    ) -> DefMapDbBuilder<'a, 'names> {
        DefMapDbBuilder {
            workspace: self.workspace,
            parse: self.parse,
            item_tree: self.item_tree,
            interners: NameInternerSource::Borrowed(interners),
        }
    }

    pub fn build(mut self) -> anyhow::Result<DefMapDb> {
        let mut db = clean::build_db(
            self.workspace,
            self.parse,
            self.item_tree,
            self.interners.as_mut(),
        )?;
        db.mutator().shrink_to_fit();
        Ok(db)
    }
}

enum NameInternerSource<'names> {
    Owned(PackageNameInterners),
    Borrowed(&'names mut PackageNameInterners),
}

impl NameInternerSource<'_> {
    fn as_mut(&mut self) -> &mut PackageNameInterners {
        match self {
            Self::Owned(interners) => interners,
            Self::Borrowed(interners) => interners,
        }
    }
}

/// Builder for a new def-map snapshot that reuses unchanged packages from an old snapshot.
pub struct DefMapDbPackageRebuilder<'a, 'db> {
    old: &'a DefMapDb,
    old_read: &'a DefMapReadTxn<'db>,
    workspace: &'a WorkspaceMetadata,
    parse: &'a rg_parse::ParseDb,
    item_tree: &'a ItemTreeDb,
    packages: &'a [PackageSlot],
    interners: &'a mut PackageNameInterners,
}

impl<'a, 'db> DefMapDbPackageRebuilder<'a, 'db> {
    pub(crate) fn new(
        old: &'a DefMapDb,
        old_read: &'a DefMapReadTxn<'db>,
        workspace: &'a WorkspaceMetadata,
        parse: &'a rg_parse::ParseDb,
        item_tree: &'a ItemTreeDb,
        packages: &'a [PackageSlot],
        interners: &'a mut PackageNameInterners,
    ) -> Self {
        DefMapDbPackageRebuilder {
            old,
            old_read,
            workspace,
            parse,
            item_tree,
            packages,
            interners,
        }
    }

    pub fn build(self) -> anyhow::Result<DefMapDb> {
        let mut db = rebuild::rebuild_packages(
            self.old,
            self.old_read,
            self.workspace,
            self.parse,
            self.item_tree,
            self.packages,
            self.interners,
        )?;
        db.mutator().shrink_packages(self.packages);
        Ok(db)
    }
}
