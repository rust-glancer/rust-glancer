//! Qualified path completion assembly for body and import positions.

use rg_body_ir::{
    BodyEnumVariantRef, BodyTypePathResolution, PathCompletionNamespace, PathCompletionSite,
    ResolvedEnumVariantRef, ResolvedFunctionRef,
};
use rg_def_map::{
    DefId, DefMapPathCompletionSite, ModuleRef, Path, ScopeNamespace, VisibleScopeDef,
};
use rg_parse::Span;
use rg_semantic_ir::{Documentation, EnumVariantRef, TypeDefId};

use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionEdit, CompletionInsertText, CompletionItem,
        CompletionKind, CompletionTarget,
    },
};

use super::{
    CompletionMetadata, CompletionQuery,
    completion_sort::CompletionSortPolicy,
    def_completion_detail,
    function::{FunctionCallCompletion, FunctionCompletionRenderer, FunctionCompletionRequest},
};

pub(super) struct PathCompletionResolver<'a, 'db, 'source> {
    analysis: &'a Analysis<'db>,
    query: CompletionQuery<'source>,
}

impl<'a, 'db, 'source> PathCompletionResolver<'a, 'db, 'source> {
    pub(super) fn new(analysis: &'a Analysis<'db>, query: CompletionQuery<'source>) -> Self {
        Self { analysis, query }
    }

    /// Collects qualified-path completions inside a body, such as
    /// `let value = crate::$0` or `let value: crate::api::Us$0`.
    pub(super) fn body_completions(
        &self,
        site: PathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let Some(body) = self.analysis.body_ir.body_data(site.body)? else {
            return Ok(Vec::new());
        };

        let edit = CompletionEdit {
            replace: site.member_prefix_span,
        };
        let mut completions = self.module_path_completions(
            body.owner_module(),
            &site.qualifier,
            site.member_prefix_span,
            PathCompletionFilter::from(site.namespace),
            FunctionCallCompletion::FunctionCall,
        )?;

        if matches!(site.namespace, PathCompletionNamespace::Values) {
            self.enum_variant_completions(site, edit, &mut completions)?;
            completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        }

        Ok(completions)
    }

    /// Collects qualified import completions, such as `use crate::api::$0`.
    pub(super) fn use_completions(
        &self,
        site: DefMapPathCompletionSite,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        self.module_path_completions(
            site.module,
            &site.qualifier,
            site.member_prefix_span,
            PathCompletionFilter::All,
            FunctionCallCompletion::Plain,
        )
    }

    /// Resolves the qualifier in `crate::api::$0` and renders definitions visible
    /// from the resolved module.
    fn module_path_completions(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
        member_prefix_span: Span,
        filter: PathCompletionFilter,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        let resolved = self
            .analysis
            .def_map
            .resolve_path_in_type_namespace(importing_module, qualifier)?;
        let edit = CompletionEdit {
            replace: member_prefix_span,
        };
        let mut completions = Vec::new();

        // Module path completion needs a module scope to list. Non-module qualifiers
        // such as type names are ignored here because they expose associated items
        // through a different lookup model.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in self
                .analysis
                .def_map
                .visible_scope_defs(importing_module, source_module)?
            {
                if filter.accepts(visible_def.namespace) {
                    self.push_visible_scope_completion(
                        visible_def,
                        edit,
                        function_call_completion,
                        &mut completions,
                    )?;
                }
            }
        }

        completions.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        Ok(completions)
    }

    /// Adds enum variants for type-qualified value paths, such as `Action::Sta$0`.
    fn enum_variant_completions(
        &self,
        site: PathCompletionSite,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        let resolution = self.analysis.body_ir.resolve_type_path_in_scope(
            &self.analysis.def_map,
            &self.analysis.semantic_ir,
            site.body,
            site.scope,
            &site.qualifier,
        )?;

        match resolution {
            BodyTypePathResolution::BodyLocal(item_ref) => {
                let Some(body) = self.analysis.body_ir.body_data(item_ref.body)? else {
                    return Ok(());
                };
                let Some(item) = body.local_item(item_ref.item) else {
                    return Ok(());
                };
                for (index, variant) in item.enum_variants().iter().enumerate() {
                    self.push_enum_variant_completion(
                        ResolvedEnumVariantRef::BodyLocal(BodyEnumVariantRef {
                            item: item_ref,
                            index,
                        }),
                        variant.name.to_string(),
                        variant.docs.as_ref().map(Documentation::text),
                        edit,
                        completions,
                    );
                }
            }
            BodyTypePathResolution::TypeDefs(type_defs)
            | BodyTypePathResolution::SelfType(type_defs) => {
                for ty in type_defs {
                    let TypeDefId::Enum(enum_id) = ty.id else {
                        continue;
                    };
                    let Some(data) = self.analysis.semantic_ir.enum_data_for_type_def(ty)? else {
                        continue;
                    };
                    for (index, variant) in data.variants.iter().enumerate() {
                        self.push_enum_variant_completion(
                            ResolvedEnumVariantRef::Semantic(EnumVariantRef {
                                target: ty.target,
                                enum_id,
                                index,
                            }),
                            variant.name.to_string(),
                            variant.docs.as_ref().map(Documentation::text),
                            edit,
                            completions,
                        );
                    }
                }
            }
            BodyTypePathResolution::Traits(_) | BodyTypePathResolution::Unknown => {}
        }

        Ok(())
    }

    fn push_enum_variant_completion(
        &self,
        variant: ResolvedEnumVariantRef,
        label: String,
        documentation: Option<String>,
        edit: CompletionEdit,
        completions: &mut Vec<CompletionItem>,
    ) {
        let target = CompletionTarget::EnumVariant(variant);
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == label)
        {
            return;
        }

        completions.push(CompletionItem {
            label: label.clone(),
            kind: CompletionKind::EnumVariant,
            target,
            applicability: CompletionApplicability::Known,
            detail: Some(def_completion_detail(CompletionKind::EnumVariant, &label)),
            documentation,
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &label,
                CompletionKind::EnumVariant,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
    }

    /// Adds one visible module-scope definition for path completion, such as
    /// `HashMap` in `std::collections::Ha$0`.
    fn push_visible_scope_completion(
        &self,
        visible_def: VisibleScopeDef,
        edit: CompletionEdit,
        function_call_completion: FunctionCallCompletion,
        completions: &mut Vec<CompletionItem>,
    ) -> anyhow::Result<()> {
        if let Some(completion) =
            self.function_completion(&visible_def, edit, function_call_completion)?
        {
            if completions.iter().any(|existing| {
                existing.target == completion.target && existing.label == completion.label
            }) {
                return Ok(());
            }
            completions.push(completion);
            return Ok(());
        }

        let Some((kind, metadata)) = self.visible_scope_completion_metadata(&visible_def)? else {
            return Ok(());
        };
        let target = CompletionTarget::Def(visible_def.def);
        if completions
            .iter()
            .any(|completion| completion.target == target && completion.label == metadata.label)
        {
            return Ok(());
        }

        completions.push(CompletionItem {
            label: metadata.label.clone(),
            kind,
            target,
            applicability: CompletionApplicability::Known,
            detail: metadata.detail,
            documentation: metadata.documentation,
            sort_text: CompletionSortPolicy::General.sort_text(
                None,
                &metadata.label,
                kind,
                CompletionApplicability::Known,
                target,
            ),
            insert_text: CompletionInsertText::Plain,
            edit: Some(edit),
        });
        Ok(())
    }

    fn function_completion(
        &self,
        visible_def: &VisibleScopeDef,
        edit: CompletionEdit,
        function_call_completion: FunctionCallCompletion,
    ) -> anyhow::Result<Option<CompletionItem>> {
        let DefId::Local(local_def) = visible_def.def else {
            return Ok(None);
        };
        let Some(function) = self
            .analysis
            .semantic_ir
            .function_for_local_def(local_def)?
        else {
            return Ok(None);
        };
        let function = ResolvedFunctionRef::Semantic(function);
        Ok(FunctionCompletionRenderer::new(self.analysis, self.query)
            .completion(FunctionCompletionRequest {
                function,
                label_override: Some(&visible_def.label),
                kind: CompletionKind::Function,
                applicability: CompletionApplicability::Known,
                edit,
                call_completion: function_call_completion,
                sort_policy: CompletionSortPolicy::General,
                sort_priority: None,
            })?
            .map(|completion| completion.item))
    }

    fn visible_scope_completion_metadata(
        &self,
        visible_def: &VisibleScopeDef,
    ) -> anyhow::Result<Option<(CompletionKind, CompletionMetadata)>> {
        let (kind, metadata) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.analysis.def_map.module(module)? else {
                    return Ok(None);
                };
                (
                    CompletionKind::Module,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(format!("mod {}", visible_def.label)),
                        documentation: data.docs.as_ref().map(Documentation::text),
                    },
                )
            }
            DefId::Local(local_def) => {
                let Some(data) = self.analysis.def_map.local_def(local_def)? else {
                    return Ok(None);
                };
                let kind = CompletionKind::from_local_def_kind(data.kind);
                (
                    kind,
                    CompletionMetadata {
                        label: visible_def.label.clone(),
                        detail: Some(def_completion_detail(kind, &visible_def.label)),
                        documentation: None,
                    },
                )
            }
        };

        Ok(Some((kind, metadata)))
    }
}

/// Namespace policy for the segment being completed in a qualified path.
///
/// Type positions like `let value: crate::$0` accept type-namespace candidates.
/// Value positions like `let value = crate::$0` accept all candidates so modules
/// and types can still be used as prefixes on the way to a value item such as
/// `crate::api::build_user()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCompletionFilter {
    Types,
    All,
}

impl PathCompletionFilter {
    fn accepts(self, namespace: ScopeNamespace) -> bool {
        match self {
            Self::Types => matches!(namespace, ScopeNamespace::Types),
            Self::All => true,
        }
    }
}

impl From<PathCompletionNamespace> for PathCompletionFilter {
    fn from(namespace: PathCompletionNamespace) -> Self {
        match namespace {
            PathCompletionNamespace::Types => Self::Types,
            PathCompletionNamespace::Values => Self::All,
        }
    }
}
