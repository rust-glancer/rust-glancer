//! Indexing selects a receiver before leaving its output projection to fulfillment.

use rg_def_map::DefMapSource;
use rg_item_tree::LangItem;
use rg_semantic_ir::SemanticItemSource;
use rg_source::ReadError;
use rg_ty::solver::{InferenceTable, List, Outcome, TraitApplication, Ty};

use super::LiveBodyQuery;

pub(crate) struct LiveIndexTarget<'s> {
    pub output: Ty<'s>,
    pub table: InferenceTable<'s>,
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Try receiver adjustments in order, retaining only the selected receiver's evidence.
    /// An unknown base must wait: searching impls for `?Base: Index<Idx>` would guess the
    /// container instead of learning it from the body.
    pub(crate) fn index<'s>(
        &self,
        base: Ty<'s>,
        index: Ty<'s>,
        table: &InferenceTable<'s>,
    ) -> Result<Option<LiveIndexTarget<'s>>, ReadError> {
        // A pending producer may become `!`. Wait for its answer before equating its
        // destination with the parameter, just as ordinary argument inference does.
        if table.has_pending_projection(index) {
            return Ok(None);
        }
        let Some(def) = self.context.item_lookup_query().lang_trait(LangItem::Index) else {
            return Ok(None);
        };
        // Autoderef itself can register goals and refine variables. Keep that traversal off
        // the body table too, then lazily stop as soon as an adjustment supplies Index.
        let trial = table.probe();
        let cx = trial.interner();
        let callbacks = cx.track_callbacks();
        let mut output = None;
        for receiver in trial.method_receivers(base) {
            if receiver.is_var() || receiver.is_unknown() {
                break;
            }
            // A missing child does not erase the receiver's shape. For `[u8; LENGTH]`,
            // let the unknown length match an impl's const parameter while retaining u8.
            let receiver = trial.instantiate_nested_unknowns(receiver);
            let mut outcome = Outcome::Unavailable;
            output = trial.commit_if_some(|trial| {
                // Index's parameter can differ from the expression's type: `&String` can
                // supply an `&str` index. Let the trait constrain a separate parameter first.
                let parameter = trial.new_type_var();
                let application = TraitApplication {
                    def,
                    args: List::new(cx, &[receiver.into(), parameter.into()]),
                };
                // Rejecting Index on one receiver must not constrain the index or the next
                // Deref step. A pending bound can still supply useful evidence: for
                // `impl<T: Marker> Index<usize> for Bag<T>` with Output = T, retain usize
                // and the output projection so a later expectation can determine T. The
                // solver keeps unresolved bounds queued and reconciles competing impls.
                trial.register(application.clause(cx));
                outcome = trial.fulfill();
                if !matches!(outcome, Outcome::Proven | Outcome::Ambiguous) {
                    return Ok(None);
                }

                let source = trial.resolve_root_var(index);
                let target = trial.resolve_root_var(parameter);
                let coerced = source.is_never()
                    || trial.try_unify(source, target).is_ok()
                    || trial.try_coerce_reference(source, target);
                if !coerced {
                    outcome = Outcome::NoSolution;
                    return Ok(None);
                }
                // The argument can select an impl or disprove its bounds. Keep its evidence
                // only if the receiver is still usable after that refinement.
                outcome = trial.fulfill();
                if !matches!(outcome, Outcome::Proven | Outcome::Ambiguous) {
                    return Ok(None);
                }
                self.projection(application, "Output", trial)
            })?;
            if outcome != Outcome::NoSolution {
                // A possible earlier adjustment takes precedence over later ones. If callback
                // data is unavailable, leave lookup deferred instead of guessing past it.
                break;
            }
        }
        if callbacks.failure().is_some() {
            return Ok(None);
        }
        Ok(output.map(|output| LiveIndexTarget {
            output,
            table: trial,
        }))
    }
}
