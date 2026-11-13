//! Shared lookup preparation and persistent answers to actual candidate questions.

use std::{iter, sync::Arc};

use rg_ir_model::CrateRef;
use rg_source::QueryContext;

use crate::{ItemLookupShard, LookupAnswer, LookupKeys, LookupQuestion, target_declarations};

/// This operation-local preparation shares repeated visibility and lookup-key reads. Every
/// candidate answer observes this query, then acquires its selected shards through its own frame.
/// Missing keys rule out unrelated targets; selected shards still establish narrower empty
/// answers, such as a known receiver that has no method with the requested name.
#[derive(PartialEq, Eq)]
struct LookupPreparation {
    indexes: Vec<PreparedLookupIndex>,
}

#[derive(PartialEq, Eq)]
struct PreparedLookupIndex {
    keys: Arc<LookupKeys>,
    shards: Arc<Vec<ajika::Output<ItemLookupShard>>>,
}

#[ajika::query(transient, returns(ref))]
fn lookup_preparation(
    db: &QueryContext<'_>,
    use_site: CrateRef,
) -> ajika::Result<LookupPreparation> {
    let declarations = target_declarations(db, use_site)?;
    let env = declarations.lookup_env.value_ref(db)?;
    let mut indexes = Vec::new();
    for &krate in env.visible_crates() {
        db.check_cancelled()?;
        let declarations = target_declarations(db, krate)?;
        indexes.push(PreparedLookupIndex {
            keys: declarations.lookup_keys.value(db)?,
            shards: declarations.lookup_shards.value(db)?,
        });
    }
    Ok(LookupPreparation { indexes })
}

#[ajika::query(returns(ref))]
pub(super) fn lookup_candidates(
    db: &QueryContext<'_>,
    use_site: CrateRef,
    question: LookupQuestion,
) -> ajika::Result<LookupAnswer> {
    let preparation = lookup_preparation(db, use_site)?;
    let mut answer = question.answer(iter::empty());
    for target in &preparation.indexes {
        db.check_cancelled()?;
        let selected = question.shards(&target.keys);
        if selected.is_empty() {
            continue;
        }
        let mut shards = Vec::new();
        for &shard in &selected {
            shards.push(target.shards[shard].value_ref(db)?);
        }
        answer.extend(question.answer(shards.into_iter()));
    }
    Ok(answer)
}
