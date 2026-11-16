//! Retain and expand the declarations inside one saved trait or impl.
//!
//! Module collection records the owner without expanding its member macros. When a query needs
//! those members, this module reconstructs their declaration list using the saved call-site scope.
//!
//! Written members are read from the file declaration query. Generated owners retain only their
//! immediate declarations and cfg-select arm syntax; their parent expansion can then be discarded.
//! Members cannot introduce modules, so they need no module-directory context. An `include!`
//! resolves relative to the retained source file, just as it did at the owner's declaration.

use std::collections::HashMap;

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{CrateRef, LocalDefId, LocalImplId, ModuleId};
use rg_item_tree::{
    BuiltinMacroItem, CfgSelectArmPayload, ItemKind, ItemNode, ItemTreeId, ItemTreeRef,
    file_declarations,
};
use rg_source::QueryContext;
use rg_workspace::{PackageInput, TargetInput};
use wincode::{SchemaRead, SchemaWrite};

use super::ItemOrder;
use crate::{ItemSource, ItemSourceKind};

mod expand;
pub use self::expand::ExpandedAssociatedItems;

/// Identity assigned when a trait or impl is collected, before semantic headers are lowered.
/// Two includes can reuse the same syntax while creating distinct owners in distinct scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssociatedItemOwner {
    Trait(LocalDefId),
    Impl(LocalImplId),
}

/// Enough source and scope information to read one owner's members after module collection ends.
///
/// For `impl Bag { methods!(); fn len(&self) { ... } }`, this keeps the call and `len` as source
/// declarations. It does not yet know which functions `methods!()` adds or their semantic IDs.
/// Written declarations can be read again from their file query; generated declarations need a
/// retained copy because the surrounding expansion's syntax is discarded.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub enum AssociatedItemInput {
    Source(AssociatedItemSource),
    /// Compiler-provided impls can supply only a header. This is an explicit empty member list,
    /// not a fallback for an owner whose capture was lost.
    Empty,
}

impl AssociatedItemInput {
    /// Read immediate declarations for lookup hints without expanding macros or signatures.
    pub fn declarations<'a>(
        &'a self,
        db: &'a QueryContext<'a>,
        krate: CrateRef,
    ) -> ajika::Result<Vec<(ItemSource, &'a ItemNode)>> {
        match self {
            Self::Source(source) => source.declarations(db, krate),
            Self::Empty => Ok(Vec::new()),
        }
    }

    pub(crate) fn capture(
        module: ModuleId,
        source: ItemSource,
        origin_source: ItemTreeRef,
        dollar_crate: Option<CrateRef>,
        order: ItemOrder,
        items: &[ItemTreeId],
        item: impl Fn(ItemTreeId) -> ItemNode,
    ) -> Self {
        Self::Source(AssociatedItemSource::capture(
            module,
            source,
            origin_source,
            dollar_crate,
            order,
            items,
            item,
        ))
    }
}

/// Syntax and lexical context of a source-backed owner. Constructed together during collection,
/// so deferred expansion sees the module and textual macro scope of this particular owner.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct AssociatedItemSource {
    module: ModuleId,
    source: ItemSource,
    origin_source: ItemTreeRef,
    /// Definition crate for `$crate` in a generated owner. Lexical lookup still uses `module`.
    dollar_crate: Option<CrateRef>,
    order: ItemOrder,
    items: Vec<ItemTreeId>,
    generated: HashMap<ItemTreeId, ItemNode>,
}

impl AssociatedItemSource {
    /// Read only the owner's immediate declarations. This also supplies cheap written-name hints
    /// without resolving any macro calls or constructing semantic signatures.
    fn declarations<'a>(
        &'a self,
        db: &'a QueryContext<'a>,
        krate: CrateRef,
    ) -> ajika::Result<Vec<(ItemSource, &'a ItemNode)>> {
        let config = PackageInput::at(krate.package).cfg(db)?;
        let kind = TargetInput::at(krate).kind(db)?;
        let cfg = CfgEvaluator::new(&config, kind.enables_test_cfg());
        let mut items = Vec::new();
        for &id in &self.items {
            let source = self.source.with_item(id);
            let node = self.item(db, krate, source)?;
            if cfg.is_enabled(&node.cfg) {
                items.push((source, node));
            }
        }
        Ok(items)
    }

    fn item<'a>(
        &'a self,
        db: &'a QueryContext<'a>,
        krate: CrateRef,
        source: ItemSource,
    ) -> ajika::Result<&'a ItemNode> {
        let node = match source.kind {
            ItemSourceKind::ItemTree(written) => {
                file_declarations(db, krate.package, written.file_id)?
                    .items
                    .value_ref(db)?
                    .items
                    .get(written.item)
            }
            ItemSourceKind::Generated(generated) => self.generated.get(&generated.item()),
            _ => None,
        };
        node.ok_or_else(|| ajika::Error::application("associated declaration source is missing"))
    }

    /// Keep the owner's position in module order so later macro lookup uses the definitions
    /// visible here, even if another definition with the same name appears farther down the file.
    fn capture(
        module: ModuleId,
        source: ItemSource,
        origin_source: ItemTreeRef,
        dollar_crate: Option<CrateRef>,
        order: ItemOrder,
        items: &[ItemTreeId],
        item: impl Fn(ItemTreeId) -> ItemNode,
    ) -> Self {
        let mut generated = HashMap::new();
        if matches!(source.kind, ItemSourceKind::Generated(_)) {
            let mut pending = items.to_vec();
            while let Some(id) = pending.pop() {
                let node = item(id);
                // cfg_select arms refer to nodes in the same arena. Retain those nodes too,
                // without keeping unrelated declarations from the surrounding expansion.
                if let ItemKind::MacroCall(call) = &node.kind
                    && let Some(BuiltinMacroItem::CfgSelect { arms }) = &call.builtin
                {
                    for arm in arms {
                        if let CfgSelectArmPayload::Items(items) = &arm.payload {
                            pending.extend(items);
                        }
                    }
                }
                generated.insert(id, node);
            }
        }
        Self {
            module,
            source,
            origin_source,
            dollar_crate,
            order,
            items: items.to_vec(),
            generated,
        }
    }
}
