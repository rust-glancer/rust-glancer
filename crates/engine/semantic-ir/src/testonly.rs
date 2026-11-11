use std::collections::HashMap;

use rg_def_map::{
    DefMap,
    testonly::{DefMapDb, DefMapFixture},
};
use rg_ir_model::{ConstOrStaticRef, CrateId, CrateRef, EvaluatedConstValue, PackageSlot};
use rg_parse::ParseDb;
use rg_source::ReadError;

pub use crate::package::{SemanticCrate, SemanticPackage};
use crate::{
    CrateItemQuery, CrateItemSource, ItemLookupFacts, ItemLookupIndex, ItemReader,
    ItemResolutionQuery, ItemStore, build::impl_headers::impl_header_resolutions,
};

/// Completed semantic arenas for type and feature kernels exercised directly by fixtures.
#[derive(Debug)]
pub struct SemanticIrDb {
    packages: Vec<SemanticPackage>,
}

impl SemanticIrDb {
    pub fn package(&self, package: PackageSlot) -> Option<&SemanticPackage> {
        self.packages.get(package.0)
    }

    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    pub fn reader(&self) -> ItemReader<'_> {
        ItemReader(self)
    }

    pub fn apply_initializer_values(
        &mut self,
        values: impl IntoIterator<Item = (ConstOrStaticRef, EvaluatedConstValue)>,
    ) {
        for (item, value) in values {
            let krate = item.origin().origin_crate();
            self.packages[krate.package.0]
                .crate_items_mut(krate.crate_id)
                .expect("fixture initializer has a declaration")
                .set_initializer_value(item, value);
        }
    }
}

impl CrateItemSource for SemanticIrDb {
    fn items(&self, krate: CrateRef) -> Result<Option<&ItemStore>, ReadError> {
        Ok(self
            .package(krate.package)
            .and_then(|package| package.crate_items(krate.crate_id)))
    }

    fn lookup_index(&self, krate: CrateRef) -> Result<Option<&ItemLookupIndex>, ReadError> {
        Ok(self
            .package(krate.package)
            .and_then(|package| package.crate_lookup_index(krate.crate_id)))
    }

    fn crates(&self) -> Vec<CrateRef> {
        self.packages
            .iter()
            .enumerate()
            .flat_map(|(slot, package)| {
                (0..package.crates().len()).map(move |id| CrateRef {
                    package: PackageSlot(slot),
                    crate_id: CrateId(id),
                })
            })
            .collect()
    }
}

/// End-to-end fixture for tests that need semantic item data.
pub struct SemanticIrFixture {
    def_map: DefMapFixture,
    semantic_ir: SemanticIrDb,
}

impl SemanticIrFixture {
    pub fn build(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build(fixture))
    }

    pub fn build_with_sysroot(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build_with_sysroot(fixture))
    }

    pub fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self::build_from_def_map(DefMapFixture::build_with_fake_sysroot(fixture))
    }

    fn build_from_def_map(mut def_map: DefMapFixture) -> Self {
        let generated_items = def_map.take_generated_items();
        let packages = (0..def_map.parse_db().package_count())
            .map(|slot| {
                let defs = def_map
                    .def_map_db()
                    .package(PackageSlot(slot))
                    .expect("fixture package has definitions");
                let items = defs
                    .crates()
                    .iter()
                    .enumerate()
                    .map(|(id, defs)| {
                        let krate = CrateRef {
                            package: PackageSlot(slot),
                            crate_id: CrateId(id),
                        };
                        ItemStore::lower_crate(
                            defs.def_map(),
                            def_map.item_trees(krate),
                            &generated_items[&krate],
                        )
                        .expect("fixture items lower")
                    })
                    .collect();
                SemanticPackage::new(items)
            })
            .collect();
        drop(generated_items);
        let mut semantic_ir = SemanticIrDb { packages };
        let definitions = def_map.def_map_db().reader();
        let items = semantic_ir.reader();
        let query = ItemResolutionQuery::new(&definitions, &items);
        let mut headers = Vec::new();
        let mut facts = HashMap::new();
        for krate in semantic_ir.crates() {
            let store = items
                .items(krate)
                .expect("fixture items read")
                .expect("known target has items");
            headers.extend(
                impl_header_resolutions(store, &query).expect("fixture impl headers resolve"),
            );
            let crate_items = CrateItemQuery::new(&definitions, &items, krate);
            facts.insert(
                krate,
                ItemLookupFacts::build(&crate_items, || Ok(()))
                    .expect("fixture lookup facts resolve"),
            );
        }
        let mut heads = HashMap::new();
        for header in headers {
            let krate = header.impl_ref.origin.origin_crate();
            semantic_ir.packages[krate.package.0]
                .crate_items_mut(krate.crate_id)
                .expect("fixture impl has an item store")
                .set_impl_header_facts(
                    header.impl_ref.id,
                    header.resolved_self_ty,
                    header.resolved_trait_ref,
                );
            if let Some(head) = header.self_head {
                heads.insert(header.impl_ref, head);
            }
        }
        for package in &mut semantic_ir.packages {
            package.rebuild_lookup_indexes(&heads, &facts);
        }

        Self {
            def_map,
            semantic_ir,
        }
    }

    pub fn parse_db(&self) -> &ParseDb {
        self.def_map.parse_db()
    }

    pub fn def_map_fixture(&self) -> &DefMapFixture {
        &self.def_map
    }

    pub fn def_map_db(&self) -> &DefMapDb {
        self.def_map.def_map_db()
    }

    pub fn semantic_ir_db(&self) -> &SemanticIrDb {
        &self.semantic_ir
    }

    pub fn semantic_ir_db_mut(&mut self) -> &mut SemanticIrDb {
        &mut self.semantic_ir
    }

    pub fn def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.def_map.def_map(crate_ref)
    }

    pub fn crate_items(&self, crate_ref: CrateRef) -> Option<&ItemStore> {
        self.semantic_ir
            .package(crate_ref.package)?
            .crate_items(crate_ref.crate_id)
    }
}
