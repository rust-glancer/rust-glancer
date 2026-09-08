//! Resolves Rust item paths from the scope that owns their documentation.
//!
//! Markdown parsing belongs to the editor feature. This view receives one link destination
//! and its offset in the docs, and uses the same indexed declarations and member queries as
//! source navigation. It neither builds documentation URLs nor retains a parsed document.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{GenericDefRef, ModuleRef, Path, SemanticItemRef, identity::DeclarationRef};
use rg_item_tree::{FromAst, TypePath, TypeRef};
use rg_parse::{LineIndex, parse_source_file};
use rg_semantic_ir::{ItemStoreQuery, TypePathContext};
use rg_std::{ExpectedUnique, UniqueVec};
use rg_syntax::{AstNode as _, ast};
use rg_text::{NameInterner, RustEdition};

use crate::{
    IndexedViewDb, SymbolKind,
    item::declaration::DeclarationView,
    lookup::resolution::ResolutionView,
    member::MemberView,
    source::{IndexedAssociatedPathQualifier, IndexedSignatureTypeScope},
    ty::TyView,
};

/// Whether a Markdown destination names a Rust declaration.
///
/// A destination that is not an item path can keep its usual Markdown meaning. An item path
/// that we cannot resolve needs different treatment: displaying its label is safer than
/// sending the editor to `super::Profile` as if it were a URL.
pub enum DocumentationLinkResolution {
    /// Ordinary URLs, section anchors, and prose are left to the Markdown renderer.
    NotAnItemLink,
    /// The destination looks like an item path, but no unique supported declaration was found.
    Unresolved,
    /// One declaration was found; source-position lookup still happens in navigation.
    Declaration(DeclarationRef),
}

/// Looks up documentation paths using the declaration that owns the comment.
///
/// For example, a comment may use `Account` because its module imports `Profile as Account`.
/// Hovering that documented item from another module must still find the original `Profile`.
/// This view chooses the comment's scope and handles rustdoc spellings such as `struct@Profile`;
/// the ordinary resolution and member views do the underlying item lookup.
pub struct DocumentationView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> DocumentationView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Find the declaration named by one link destination, such as `super::Profile`.
    ///
    /// `owner` is the documented declaration, not the place where it was hovered. `offset`
    /// points to the link in that declaration's unmodified docs. Module docs need it because
    /// outer comments use the parent scope while inner comments use the module itself.
    pub fn resolve_link(
        &self,
        owner: DeclarationRef,
        offset: usize,
        destination: &str,
    ) -> anyhow::Result<DocumentationLinkResolution> {
        let edition = self
            .db
            .crate_edition(owner.origin().origin_crate())
            .context("read documentation owner edition")?;
        let Some((type_path, disambiguator)) = Self::parse_destination(destination, edition) else {
            return Ok(DocumentationLinkResolution::NotAnItemLink);
        };
        // TODO: support qualified trait paths and source targets for primitive documentation.
        if type_path.anchor.is_some() {
            return Ok(DocumentationLinkResolution::Unresolved);
        }
        let Some(scope) = self
            .scope(owner, offset)
            .context("read documentation scope")?
        else {
            return Ok(DocumentationLinkResolution::Unresolved);
        };

        let Some(path) = type_path.as_def_map_path() else {
            return Ok(DocumentationLinkResolution::Unresolved);
        };
        let mut declarations = self
            .resolve_item_path(&scope, &path)
            .context("resolve documentation item path")?;
        // Only try a member interpretation when the whole path found no declarations.
        // A disambiguator filters those results later; it does not change this lookup order.
        if declarations.is_empty() {
            declarations = self
                .resolve_member_path(&scope, type_path, &path)
                .context("resolve documentation member path")?;
        }
        self.select_unique_target(declarations, disambiguator)
            .context("select documentation link target")
    }

    /// Parse rustdoc's destination spelling into a Rust path and an optional item-kind hint.
    /// A destination that cannot be parsed as a whole path keeps its usual Markdown meaning.
    fn parse_destination(
        destination: &str,
        edition: RustEdition,
    ) -> Option<(TypePath, Option<&str>)> {
        let (destination, disambiguator) = Self::split_disambiguator(destination);
        // Parse the path with the Rust parser so raw identifiers and generic arguments follow
        // Rust syntax. The temporary alias gives a type path its normal grammar entry point;
        // it is never indexed, and its source offsets never become navigation destinations.
        let source = format!("type DocLink = {destination};");
        let Ok(parsed) = parse_source_file(&source, edition).ok() else {
            return None;
        };
        let ast::Type::PathType(ty) = parsed
            .syntax()
            .children()
            .find_map(ast::TypeAlias::cast)
            .and_then(|alias| alias.ty())?
        else {
            return None;
        };
        if ty.syntax().text() != destination {
            return None;
        }
        let TypeRef::Path(type_path) = TypeRef::from_ast(
            &ast::Type::PathType(ty),
            (&LineIndex::new(&source), &mut NameInterner::default()),
        ) else {
            return None;
        };
        Some((type_path, disambiguator))
    }

    /// Resolve the whole path as an imported name, module path, or enum variant.
    /// `Self` uses its explicit owner binding, independent of names in the module.
    fn resolve_item_path(
        &self,
        scope: &DocumentationScope,
        path: &Path,
    ) -> anyhow::Result<UniqueVec<DeclarationRef>> {
        let resolution = ResolutionView::new(self.db);
        if path.is_self_type() {
            resolution.declarations_for_semantic_type_path(scope.context, path)
        } else {
            resolution.declarations_for_use_path(scope.context.module, path)
        }
        .map(|declarations| declarations.into_iter().collect())
    }

    /// Interpret `User::new` as a member named `new` on the type denoted by `User`.
    /// The parsed type path retains generic arguments needed by associated-item lookup.
    fn resolve_member_path(
        &self,
        scope: &DocumentationScope,
        mut type_path: TypePath,
        path: &Path,
    ) -> anyhow::Result<UniqueVec<DeclarationRef>> {
        let mut declarations = UniqueVec::new();
        let Some((prefix, name)) = path.split_prefix_name() else {
            return Ok(declarations);
        };
        let members = MemberView::new(self.db);
        let prefix_ty = TyView::new(self.db)
            .ty_for_type_path(scope.context, &prefix)
            .context("resolve documentation member owner")?;

        // Fields have their own declaration identities and are not associated-item candidates.
        for ty in prefix_ty.nominal_type_defs() {
            for field in members
                .field_candidates_for_type_def(ty)
                .context("read documentation target fields")?
            {
                if field.key().map(ToString::to_string).as_deref() == Some(name) {
                    declarations.push(DeclarationRef::Field(field.field_ref()));
                }
            }
        }

        // An item's generic bounds can give meaning to paths such as `T::make`.
        // Module and macro docs have no generic owner, so they use the module lookup.
        let candidates = if let Some(generic_owner) = scope.generic_owner {
            type_path.segments.pop();
            let qualifier = IndexedAssociatedPathQualifier::Type(TypeRef::Path(type_path));
            let signature_scope = IndexedSignatureTypeScope::new(scope.context, generic_owner);
            members.associated_item_candidates_for_signature(signature_scope, &qualifier)
        } else {
            members.associated_item_candidates_for_module(scope.context.module, &prefix, &prefix_ty)
        }
        .context("resolve documentation associated items")?;
        for candidate in candidates {
            let Some(definition) = members
                .associated_item_definition(candidate)
                .context("read documentation associated item")?
            else {
                continue;
            };
            if definition.label() == name {
                declarations.push(candidate.item().declaration_ref());
            }
        }
        Ok(declarations)
    }

    /// A name can exist in several namespaces: `Profile` may be both a type and a function.
    /// Apply hints such as `struct@` before requiring one distinct declaration. Repeated
    /// evidence for the same declaration is fine; two different matches remain ambiguous.
    fn select_unique_target(
        &self,
        declarations: UniqueVec<DeclarationRef>,
        disambiguator: Option<&str>,
    ) -> anyhow::Result<DocumentationLinkResolution> {
        let view = DeclarationView::new(self.db);
        let mut matching = ExpectedUnique::new();
        for declaration in declarations {
            let kind = if matches!(declaration, DeclarationRef::Module(_)) {
                SymbolKind::Module
            } else if let Some(item) = view
                .declaration(declaration)
                .context("read documentation target kind")?
            {
                item.kind()
            } else {
                continue;
            };
            if Self::matches_disambiguator(disambiguator, kind) {
                matching.push(declaration);
            }
        }
        Ok(match matching.into_option() {
            Some(declaration) => DocumentationLinkResolution::Declaration(declaration),
            None => DocumentationLinkResolution::Unresolved,
        })
    }

    /// Separate the Rust path from rustdoc's hints about the item kind.
    /// `struct@Profile` becomes `Profile` plus a struct hint; `User::new()` gets a function hint.
    /// The label is kept elsewhere, so changing the lookup spelling does not alter what users read.
    fn split_disambiguator(destination: &str) -> (&str, Option<&str>) {
        let destination = destination.trim().trim_matches('`');
        if let Some((kind, path)) = destination.split_once('@') {
            return (path, Some(kind));
        }
        for suffix in ["!()", "!{}", "![]", "!"] {
            if let Some(path) = destination.strip_suffix(suffix) {
                return (path, Some("macro"));
            }
        }
        if let Some(path) = destination.strip_suffix("()") {
            return (path, Some("fn"));
        }
        (destination, None)
    }

    /// Hints can name one item kind or a whole namespace, as with `type@` and `value@`.
    fn matches_disambiguator(disambiguator: Option<&str>, kind: SymbolKind) -> bool {
        match disambiguator {
            None => true,
            Some("struct") => kind == SymbolKind::Struct,
            Some("enum") => kind == SymbolKind::Enum,
            Some("union") => kind == SymbolKind::Union,
            Some("trait") => kind == SymbolKind::Trait,
            Some("typealias" | "tyalias") => kind == SymbolKind::TypeAlias,
            Some("mod" | "module") => kind == SymbolKind::Module,
            Some("fn" | "function" | "method") => {
                matches!(kind, SymbolKind::Function | SymbolKind::Method)
            }
            Some("const" | "constant") => kind == SymbolKind::Const,
            Some("static") => kind == SymbolKind::Static,
            Some("macro" | "derive") => kind == SymbolKind::Macro,
            Some("field") => kind == SymbolKind::Field,
            Some("variant") => kind == SymbolKind::EnumVariant,
            Some("type") => matches!(
                kind,
                SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Union
                    | SymbolKind::Trait
                    | SymbolKind::TypeAlias
                    | SymbolKind::Module
            ),
            Some("value") => matches!(
                kind,
                SymbolKind::Function
                    | SymbolKind::Method
                    | SymbolKind::Const
                    | SymbolKind::Static
                    | SymbolKind::EnumVariant
            ),
            Some(_) => false,
        }
    }

    /// Recover the imports, generic bounds, and meaning of `Self` available to a comment.
    /// Fields and enum variants borrow that context from their containing type; associated
    /// items use their own declaration context so impl and trait owners remain available.
    fn scope(
        &self,
        owner: DeclarationRef,
        offset: usize,
    ) -> anyhow::Result<Option<DocumentationScope>> {
        let items = ItemStoreQuery::new(self.db);
        let item = match owner {
            DeclarationRef::Module(module_ref) => {
                let Some(module) = self
                    .db
                    .module_data(module_ref)
                    .context("read documented module")?
                else {
                    return Ok(None);
                };
                // The module's displayed docs join comments from outside and inside it.
                // Select the scope using the retained boundary, not the hover's location.
                let module = if module
                    .docs
                    .as_ref()
                    .is_some_and(|docs| !docs.is_inner_at(offset))
                {
                    ModuleRef {
                        module: module.parent.unwrap_or(module_ref.module),
                        ..module_ref
                    }
                } else {
                    module_ref
                };
                return Ok(Some(DocumentationScope {
                    context: TypePathContext::module(module),
                    generic_owner: None,
                }));
            }
            DeclarationRef::LocalDef(local) => {
                let Some(data) = self
                    .db
                    .local_def_data(local)
                    .context("read documented local item")?
                else {
                    return Ok(None);
                };
                return Ok(Some(DocumentationScope {
                    context: TypePathContext::module(ModuleRef {
                        origin: local.origin,
                        module: data.module,
                    }),
                    generic_owner: None,
                }));
            }
            DeclarationRef::Item(item) => item,
            DeclarationRef::Field(field) => SemanticItemRef::TypeDef(field.owner),
            DeclarationRef::EnumVariant(variant) => {
                let Some(data) = items
                    .enum_variant_data(variant)
                    .context("read documented variant")?
                else {
                    return Ok(None);
                };
                SemanticItemRef::TypeDef(data.owner)
            }
            DeclarationRef::BodyBinding(_) => return Ok(None),
        };
        let generic_owner = GenericDefRef::from(item);
        let Some(context) = items
            .type_path_context_for_generic_def(generic_owner)
            .context("read documented item context")?
        else {
            return Ok(None);
        };
        Ok(Some(DocumentationScope {
            context,
            generic_owner: Some(generic_owner),
        }))
    }
}

/// The lookup context of a comment, including any type or trait meant by `Self`.
struct DocumentationScope {
    /// Supplies module imports and the type, trait, or impl that binds `Self`.
    context: TypePathContext,
    /// Supplies generic bounds for links such as `T::make` on an item with `T: Factory`.
    generic_owner: Option<GenericDefRef>,
}
