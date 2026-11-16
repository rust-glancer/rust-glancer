//! Prepare compiler identities after module declarations and owner headers have been published.
//!
//! `Deref::Target` needs a real member ID, which is allocated only after expanding Deref's members.
//! Reading that owner's normal query keeps one allocation path for lang items and ordinary items.
//! The resulting tables retain only references, so preparing them does not keep the member stores
//! resident. Neither member lowering nor header collection depends on these tables.

use rg_ir_model::CrateRef;
use rg_source::QueryContext;
use rg_std::Shrink as _;

use crate::{ItemLookupEnv, LangItemIndex, associated_declarations, target_declarations};

#[ajika::query]
impl LangItemIndex {
    /// Complete one crate's identities using only owners selected during declaration indexing.
    /// Reading the compact member output lets signature edits leave the resulting index unchanged.
    #[ajika::query(returns(ref))]
    pub(crate) fn for_crate(db: &QueryContext<'_>, krate: CrateRef) -> ajika::Result<Self> {
        let target = target_declarations(db, krate)?;
        let mut index = target.scope_lang_items.value_ref(db)?.clone();
        for &owner in target.lang_item_owners.value_ref(db)? {
            db.check_cancelled()?;
            let members = associated_declarations(db, krate, owner)?
                .as_ref()
                .ok_or_else(|| ajika::Error::contract("lang-item owner has no member store"))?;
            index.extend(members.lang_items.value_ref(db)?);
        }
        index.shrink_to_fit();
        Ok(index)
    }
}

#[ajika::query]
impl ItemLookupEnv {
    /// Prepare final lang-item choices for one use site. Startup and saves demand this before
    /// publishing the project, but readers can also call it directly without any preload step.
    #[ajika::query(returns(ref))]
    pub fn for_crate(db: &QueryContext<'_>, krate: CrateRef) -> ajika::Result<Self> {
        let target = target_declarations(db, krate)?;
        let visible = target.visible_crates.value_ref(db)?;
        let mut indexes = Vec::with_capacity(visible.len());
        for &krate in visible {
            db.check_cancelled()?;
            indexes.push(LangItemIndex::for_crate(db, krate)?);
        }
        Ok(Self::build(indexes))
    }
}
