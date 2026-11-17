//! Candidate lookup over one use site's saved visibility.
//!
//! Preparing a lookup reads only its small visibility and language-item environment. Candidate
//! questions go through the source that owns the indexes: production readers use Ajika, while kernel
//! fixtures can supply borrowed indexes. In either case lookup only selects declarations; type
//! reasoning still proves their applicability.

use std::{fmt, sync::Arc};

use rg_ir_model::{
    CrateRef, FunctionRef, ImplRef, SemanticItemRef, TraitDefRef, TraitImplRef, TypeAliasRef,
    TypeDefRef,
};
use rg_item_tree::LangItem;
use rg_source::ReadError;
use rg_std::{Cancelable, CancellationToken, Cancelled, UniqueVec};
use rg_text::Name;

use super::{
    ItemStoreQuery, SemanticItemSource,
    candidates::{LookupAnswer, LookupQuestion},
};
use crate::{ItemLookupEnv, ItemLookupIndex, ItemReader, TraitImplSelfHead};

type CandidateLookup<'a> =
    dyn Fn(&LookupQuestion) -> Result<LookupAnswer, ReadError> + Send + Sync + 'a;

/// A use-site view. Candidate answers have the same error channel as its semantic source.
#[derive(Clone)]
pub struct ItemLookupQuery<'item> {
    lookup_env: &'item ItemLookupEnv,
    candidates: Arc<CandidateLookup<'item>>,
    cancellation: CancellationToken,
}

impl fmt::Debug for ItemLookupQuery<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemLookupQuery").finish_non_exhaustive()
    }
}

impl Cancelable for ItemLookupQuery<'_> {
    fn check_cancelled(&self, checkpoint: &'static str) -> Result<(), Cancelled> {
        Cancelable::check_cancelled(&self.cancellation, checkpoint)
    }
}

impl<'item> ItemLookupQuery<'item> {
    pub fn build_from(
        items: ItemReader<'item>,
        use_site: CrateRef,
        cancellation: &CancellationToken,
    ) -> Result<Self, ReadError> {
        rg_std::check_cancel!(cancellation, "prepare candidate lookup");
        let lookup_env = items
            .0
            .lookup_env(use_site)?
            .expect("use-site crate has lookup environment");
        Ok(Self {
            lookup_env,
            candidates: Arc::new(move |question| items.0.lookup_candidates(use_site, question)),
            cancellation: cancellation.clone(),
        })
    }

    /// Kernel fixtures already own their indexes and have no query runtime to acquire them from.
    pub fn from_indexes(
        lookup_env: &'item ItemLookupEnv,
        indexes: Vec<&'item ItemLookupIndex>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            lookup_env,
            candidates: Arc::new(move |question| {
                Ok(question.answer(indexes.iter().flat_map(|index| index.shards_for(question))))
            }),
            cancellation,
        }
    }

    fn ask(&self, question: LookupQuestion) -> Result<LookupAnswer, ReadError> {
        rg_std::check_cancel!(self, "lookup candidates");
        let answer = (self.candidates)(&question)?;
        rg_std::check_cancel!(self, "lookup candidates");
        Ok(answer)
    }
    /// Returns the exact visible nominal type carrying one compiler language identity.
    pub fn lang_type(&self, lang_item: LangItem) -> Option<TypeDefRef> {
        let SemanticItemRef::TypeDef(type_ref) = self.lookup_env.lang_item(lang_item)? else {
            return None;
        };
        Some(type_ref)
    }

    /// Returns the exact visible trait carrying one compiler language identity.
    pub fn lang_trait(&self, lang_item: LangItem) -> Option<TraitDefRef> {
        let SemanticItemRef::Trait(trait_ref) = self.lookup_env.lang_item(lang_item)? else {
            return None;
        };
        Some(trait_ref)
    }

    /// Returns the exact visible function carrying one compiler language identity.
    pub fn lang_function(&self, lang_item: LangItem) -> Option<FunctionRef> {
        let SemanticItemRef::Function(function_ref) = self.lookup_env.lang_item(lang_item)? else {
            return None;
        };
        Some(function_ref)
    }

    /// Returns the exact visible type alias carrying one compiler language identity.
    pub fn lang_type_alias(&self, lang_item: LangItem) -> Option<TypeAliasRef> {
        let SemanticItemRef::TypeAlias(type_alias_ref) = self.lookup_env.lang_item(lang_item)?
        else {
            return None;
        };
        Some(type_alias_ref)
    }

    /// Expands visible inherent impls to their function items through the caller's query source.
    pub fn inherent_functions_for_type<'query, S>(
        &self,
        item_query: &ItemStoreQuery<'query, S>,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<FunctionRef>, ReadError>
    where
        S: SemanticItemSource<'query>,
    {
        let mut functions = UniqueVec::new();
        for impl_ref in self.inherent_impls_for_type(ty)? {
            functions.extend(item_query.associated_functions(impl_ref.member_store())?);
        }
        Ok(functions)
    }

    pub fn inherent_functions_for_type_and_name(
        &self,
        ty: TypeDefRef,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, ReadError> {
        let LookupAnswer::Functions(answer) =
            self.ask(LookupQuestion::InherentFunctions(ty, Name::new(name)))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer.unwrap_or_default())
    }
    pub fn structural_inherent_functions_by_name(
        &self,
        name: &str,
    ) -> Result<UniqueVec<FunctionRef>, ReadError> {
        let LookupAnswer::Functions(answer) =
            self.ask(LookupQuestion::StructuralFunctions(Name::new(name)))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer.unwrap_or_default())
    }
    pub fn structural_inherent_impls(&self) -> Result<UniqueVec<ImplRef>, ReadError> {
        let LookupAnswer::Impls(answer) = self.ask(LookupQuestion::StructuralImpls)? else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn inherent_impls_for_type(&self, ty: TypeDefRef) -> Result<UniqueVec<ImplRef>, ReadError> {
        let LookupAnswer::Impls(answer) = self.ask(LookupQuestion::InherentImpls(ty))? else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn impls_for_type(&self, ty: TypeDefRef) -> Result<UniqueVec<ImplRef>, ReadError> {
        let LookupAnswer::Impls(answer) = self.ask(LookupQuestion::ImplsForType(ty))? else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn impls_for_trait(&self, trait_ref: TraitDefRef) -> Result<UniqueVec<ImplRef>, ReadError> {
        let LookupAnswer::Impls(answer) = self.ask(LookupQuestion::ImplsForTrait(trait_ref))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn trait_impls_for_type(
        &self,
        ty: TypeDefRef,
    ) -> Result<UniqueVec<TraitImplRef>, ReadError> {
        let LookupAnswer::TraitImpls(answer) = self.ask(LookupQuestion::TraitImplsForType(ty))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer.unwrap_or_default())
    }
    pub fn trait_impls_for_trait(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, ReadError> {
        let LookupAnswer::TraitImpls(answer) = self.ask(LookupQuestion::TraitImpls(trait_ref))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn trait_impl_candidates_for_self_head(
        &self,
        trait_ref: TraitDefRef,
        self_head: Option<TraitImplSelfHead>,
    ) -> Result<Option<UniqueVec<TraitImplRef>>, ReadError> {
        let LookupAnswer::TraitImpls(answer) =
            self.ask(LookupQuestion::TraitImplsForHead(trait_ref, self_head))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn traits_with_functions(&self) -> Result<UniqueVec<TraitDefRef>, ReadError> {
        let LookupAnswer::Traits(answer) = self.ask(LookupQuestion::TraitsWithFunctions)? else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn traits_with_associated_items(&self) -> Result<UniqueVec<TraitDefRef>, ReadError> {
        let LookupAnswer::Traits(answer) = self.ask(LookupQuestion::TraitsWithAssociatedItems)?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn traits_with_function_name(
        &self,
        name: &str,
    ) -> Result<UniqueVec<TraitDefRef>, ReadError> {
        let LookupAnswer::Traits(answer) =
            self.ask(LookupQuestion::TraitsWithFunctionName(Name::new(name)))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn traits_with_const_name(&self, name: &str) -> Result<UniqueVec<TraitDefRef>, ReadError> {
        let LookupAnswer::Traits(answer) =
            self.ask(LookupQuestion::TraitsWithConstName(Name::new(name)))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn trait_functions(
        &self,
        trait_ref: TraitDefRef,
    ) -> Result<Option<UniqueVec<FunctionRef>>, ReadError> {
        let LookupAnswer::Functions(answer) =
            self.ask(LookupQuestion::TraitFunctions(trait_ref))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
    pub fn trait_functions_by_name(
        &self,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> Result<Option<UniqueVec<FunctionRef>>, ReadError> {
        let LookupAnswer::Functions(answer) = self.ask(LookupQuestion::TraitFunctionsByName(
            trait_ref,
            Name::new(name),
        ))?
        else {
            unreachable!("candidate answer matches its question")
        };
        Ok(answer)
    }
}
