//! Receiver-method lookup across crate-level and body-local impl universes.

use std::{collections::HashSet, time::Instant};

use anyhow::Context as _;
use rg_ir_model::{ModuleRef, Path, TraitDefRef, identity::LexicalScopeRef};
use rg_semantic_ir::ItemStoreQuery;
use rg_text::Name;
use rg_ty::{Ty, lookup::MemberMethodCandidateRef};

use super::{MemberFunction, MemberMethodCandidate, MemberView};
use crate::{
    body::BodyResolutionView,
    lookup::importable::DefinitionImportSearch,
    ty::{ExpectedCallResult, IndexedType},
};

impl<'a, 'db> MemberView<'a, 'db> {
    /// Return methods visible for a type at one lexical body scope.
    pub fn method_candidates_for_ty<'view>(
        &'view self,
        scope: LexicalScopeRef,
        ty: &IndexedType,
        expected: Option<ExpectedCallResult<'_>>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let candidates = self
            .body_method_candidate_refs_for_ty(scope, ty.raw(), None, expected)
            .context("resolve method candidate references")?;
        self.method_candidates_from_refs(candidates)
            .context("project method candidates")
    }

    /// Find out-of-scope trait methods matching the typed prefix and pair them with import paths.
    ///
    /// Each crate's name table supplies a batch. Body resolution checks the receiver first.
    /// The import walk then looks up the traits behind the methods that passed.
    #[allow(clippy::too_many_arguments)]
    pub fn extension_method_candidates<'view>(
        &'view self,
        scope: LexicalScopeRef,
        module: ModuleRef,
        ty: &IndexedType,
        expected: Option<ExpectedCallResult<'_>>,
        prefix: &str,
        deadline: Instant,
        occupied: impl Fn(&str) -> bool,
    ) -> anyhow::Result<Vec<(MemberMethodCandidate<'view>, Path)>> {
        const MAX_CRATES: usize = 96;
        const MAX_FUNCTIONS: usize = 16;

        let items = ItemStoreQuery::new(self.db);
        let crates = self
            .db
            .semantic_ir
            .visible_crates(module.origin.origin_crate())?;
        let body = BodyResolutionView::new(self.db);
        let mut remaining = MAX_FUNCTIONS;
        let mut result = Vec::new();
        let mut imports = None;

        // Finish the candidates from each target while we still have budget. In particular, a
        // local extension should be checked before opening name tables for all dependencies.
        for &krate in crates.iter().take(MAX_CRATES) {
            rg_std::check_cancel!(self.db, "extension method completion");
            if remaining == 0 || Instant::now() >= deadline {
                break;
            }
            let names = self.db.semantic_ir.trait_function_names(krate)?;
            let first = names.partition_point(|(name, _)| name.as_str() < prefix);
            // TODO: Index names produced entirely by macros without expanding all trait owners.
            let mut matching = names[first..]
                .iter()
                .take_while(|(name, _)| {
                    name.as_str().starts_with(prefix) && Instant::now() < deadline
                })
                .filter(|(name, _)| !occupied(name.as_str()))
                .peekable();
            if matching.peek().is_none() {
                continue;
            }

            let methods = body.extension_methods_for_ty(
                scope,
                ty.raw(),
                matching,
                Some((&mut remaining, deadline)),
                expected.map(ExpectedCallResult::raw),
            )?;
            if methods.is_empty() {
                continue;
            }
            // The user types the method's name, but the new `use` needs a path to its trait.
            let mut methods_with_owners = Vec::with_capacity(methods.len());
            for method in methods {
                if let Some(trait_ref) = method.function().declaring_trait()
                    && let Some(data) = items.trait_data(trait_ref)?
                {
                    methods_with_owners.push((method, data.local_def));
                }
            }
            let definitions = methods_with_owners
                .iter()
                .map(|&(_, owner)| owner)
                .collect::<HashSet<_>>();
            if definitions.is_empty() {
                continue;
            }
            let imports = match &mut imports {
                Some(imports) => imports,
                None => imports.insert(DefinitionImportSearch::new(self.db, module, deadline)?),
            };
            let paths = imports.search(&definitions)?;
            for (method, owner) in methods_with_owners {
                if let Some(path) = paths.get(&owner)
                    && let Some(function) = self.function(method.function())?
                {
                    result.push((Self::method_candidate(function, method), path.clone()));
                }
            }
        }
        Ok(result)
    }

    /// Check the selected trait method after an edit. A method already available under
    /// the same name wins, just as it does when building the completion list.
    pub fn needs_method_import(
        &self,
        scope: LexicalScopeRef,
        ty: &IndexedType,
        trait_ref: TraitDefRef,
        name: &str,
    ) -> anyhow::Result<bool> {
        if !self
            .body_method_candidate_refs_for_ty(scope, ty.raw(), Some(name), None)?
            .is_empty()
        {
            return Ok(false);
        }
        let selected = (Name::new(name), trait_ref);
        let methods = BodyResolutionView::new(self.db).extension_methods_for_ty(
            scope,
            ty.raw(),
            std::iter::once(&selected),
            None,
            None,
        )?;
        Ok(methods.len() == 1)
    }

    /// Return body-aware method refs.
    fn body_method_candidate_refs_for_ty(
        &self,
        scope: LexicalScopeRef,
        ty: &Ty,
        name: Option<&str>,
        expected: Option<ExpectedCallResult<'_>>,
    ) -> anyhow::Result<Vec<MemberMethodCandidateRef>> {
        let body = scope.body_ir();
        let Some(candidates) = BodyResolutionView::new(self.db)
            .method_candidate_refs_for_ty(
                body,
                scope.scope_id(),
                ty,
                name,
                expected.map(ExpectedCallResult::raw),
            )
            .context("resolve body method candidates")?
        else {
            // Lexical trait scope belongs to the body's DefMap. If those facts disappeared between
            // source-site discovery and lookup, a crate-wide fallback would reintroduce traits
            // that Rust does not make callable here. Fail soft with no methods instead.
            return Ok(Vec::new());
        };

        Ok(candidates)
    }

    /// Load function data for method refs and keep candidates whose functions still exist.
    fn method_candidates_from_refs<'view>(
        &'view self,
        candidates: Vec<MemberMethodCandidateRef>,
    ) -> anyhow::Result<Vec<MemberMethodCandidate<'view>>> {
        let mut methods = Vec::new();
        for candidate in candidates {
            let Some(function) = self
                .function(candidate.function())
                .context("read method candidate function")?
            else {
                continue;
            };
            methods.push(Self::method_candidate(function, candidate));
        }

        Ok(methods)
    }

    /// Attach the declaration without copying the facts from lookup into another candidate.
    fn method_candidate<'view>(
        function: MemberFunction<'view>,
        candidate: MemberMethodCandidateRef,
    ) -> MemberMethodCandidate<'view> {
        MemberMethodCandidate {
            candidate,
            data: function.data,
        }
    }
}
