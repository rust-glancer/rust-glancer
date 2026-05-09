use std::fmt::Write as _;

use expect_test::Expect;

use crate::{
    FieldItem, FieldList, FileTree, ItemKind, ItemNode, ItemTreeDb, ItemTreeId, ModuleSource,
    Package as ItemTreePackage, ParamKind, TargetRoot, VisibilityLevel,
};
use rg_parse::{FileId, Package, ParseDb, Target};
use rg_workspace::WorkspaceMetadata;
use test_fixture::fixture_crate;

pub(super) fn check_project_item_tree(fixture: &str, expect: Expect) {
    let db = ItemTreeFixtureDb::build(fixture);
    let actual = ProjectItemTreeSnapshot::new(&db, SnapshotMode::Structure).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_item_tree_with_declarations(fixture: &str, expect: Expect) {
    let db = ItemTreeFixtureDb::build(fixture);
    let actual = ProjectItemTreeSnapshot::new(&db, SnapshotMode::Declarations).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_item_tree_with_spans(fixture: &str, expect: Expect) {
    let db = ItemTreeFixtureDb::build(fixture);
    let actual = ProjectItemTreeSnapshot::new(&db, SnapshotMode::Spans).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

struct ItemTreeFixtureDb {
    parse: ParseDb,
    item_tree: ItemTreeDb,
}

impl ItemTreeFixtureDb {
    fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let metadata = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        let mut parse = ParseDb::build(&metadata).expect("fixture parse db should build");
        let item_tree = ItemTreeDb::build(&mut parse).expect("fixture item tree db should build");
        Self { parse, item_tree }
    }
}

/// Project-level item-tree snapshot context.
/// Renders package sections such as `package demo`.
struct ProjectItemTreeSnapshot<'a> {
    db: &'a ItemTreeFixtureDb,
    mode: SnapshotMode,
}

impl<'a> ProjectItemTreeSnapshot<'a> {
    fn new(db: &'a ItemTreeFixtureDb, mode: SnapshotMode) -> Self {
        Self { db, mode }
    }

    fn render(&self) -> String {
        let package_dumps = sorted_packages(&self.db.parse)
            .into_iter()
            .map(|(package_slot, package)| {
                let item_trees = self
                    .db
                    .item_tree
                    .package(package_slot)
                    .expect("package item trees should exist while rendering snapshot");
                PackageItemTreeSnapshot {
                    package,
                    item_trees,
                    mode: self.mode,
                }
                .render()
            })
            .collect::<Vec<_>>();

        package_dumps.join("\n\n")
    }
}

/// Package-level item-tree snapshot context with file-label access.
/// Renders target/file sections such as `file lib.rs`.
struct PackageItemTreeSnapshot<'a> {
    package: &'a Package,
    item_trees: &'a crate::Package,
    mode: SnapshotMode,
}

impl<'a> PackageItemTreeSnapshot<'a> {
    fn render(&self) -> String {
        let target_dumps = sorted_item_tree_target_roots(self.package, self.item_trees)
            .into_iter()
            .map(|target_root| {
                let target = self
                    .package
                    .target(target_root.target)
                    .expect("parsed target should exist while rendering snapshot");
                self.render_target_root(target, target_root.root_file)
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let file_dumps = sorted_item_tree_files(self.package, self.item_trees)
            .into_iter()
            .map(|file_tree| {
                self.render_file_item_tree(file_tree, &file_tree.top_level)
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        format!(
            "package {}\n\ntargets\n{target_dumps}\n\nfiles\n{file_dumps}",
            self.package.package_name()
        )
    }

    fn render_target_root(&self, target: &Target, root_file: rg_parse::FileId) -> String {
        let mut dump = String::new();
        writeln!(
            &mut dump,
            "- {} [{}] -> {}",
            target.name,
            target.kind,
            self.file_label(root_file)
        )
        .expect("string writes should not fail");

        dump
    }

    fn render_file_item_tree(&self, file_tree: &crate::FileTree, items: &[ItemTreeId]) -> String {
        let mut dump = String::new();
        writeln!(&mut dump, "file {}", self.file_label(file_tree.file))
            .expect("string writes should not fail");

        for item_id in items {
            let item = file_tree
                .item(*item_id)
                .expect("item id should exist while rendering item tree");
            self.render_item(file_tree, item, 0, &mut dump);
        }

        dump
    }

    fn render_item(
        &self,
        file_tree: &crate::FileTree,
        item: &ItemNode,
        depth: usize,
        dump: &mut String,
    ) {
        let indent = "  ".repeat(depth);
        let mut line = format!("{indent}- ");

        if item.visibility != crate::VisibilityLevel::Private {
            line.push_str(&format!("{} ", item.visibility));
        }

        line.push_str(&item.kind.to_string());

        if let Some(name) = &item.name {
            line.push(' ');
            line.push_str(name);
        }

        if let ItemKind::Module(module) = &item.kind {
            line.push_str(&format!(" [{}]", self.render_module_source(&module.source)));
        }

        if let ItemKind::ExternCrate(extern_crate) = &item.kind {
            let name = extern_crate.name.as_deref().unwrap_or("<missing>");
            line.push_str(&format!(" [{name}{}]", extern_crate.alias));
        }

        if matches!(self.mode, SnapshotMode::Spans) {
            let line_column = item.span.line_column(
                self.package
                    .parsed_file(item.file_id)
                    .expect("item file should exist while rendering source span")
                    .line_index()
                    .expect("item file line index should load while rendering source span"),
            );
            line.push_str(&format!(
                " [{} {}:{}-{}:{} ({}..{})]",
                self.file_label(item.file_id),
                line_column.start.line + 1,
                line_column.start.column + 1,
                line_column.end.line + 1,
                line_column.end.column + 1,
                item.span.text.start,
                item.span.text.end,
            ));
        }

        writeln!(dump, "{line}").expect("string writes should not fail");

        if matches!(self.mode, SnapshotMode::Declarations) {
            self.render_declaration_payload(file_tree, item, depth, dump);
        }

        if let ItemKind::Use(use_item) = &item.kind {
            for import in &use_item.imports {
                let path = import.path.to_string();
                let path = if path.is_empty() {
                    "<empty>".to_string()
                } else {
                    path
                };

                writeln!(
                    dump,
                    "{}  - import {} {}{}",
                    indent, import.kind, path, import.alias
                )
                .expect("string writes should not fail");
            }
        }

        if let ItemKind::Module(module) = &item.kind {
            if let ModuleSource::Inline { items } = &module.source {
                for child_id in items {
                    let child = file_tree
                        .item(*child_id)
                        .expect("inline child item id should exist while rendering");
                    self.render_item(file_tree, child, depth + 1, dump);
                }
            }
        }
    }

    fn render_declaration_payload(
        &self,
        file_tree: &crate::FileTree,
        item: &ItemNode,
        depth: usize,
        dump: &mut String,
    ) {
        let indent = "  ".repeat(depth);

        match &item.kind {
            ItemKind::Const(const_item) => {
                self.render_generics(&const_item.generics, depth, dump);
                if let Some(ty) = &const_item.ty {
                    writeln!(dump, "{indent}  - ty {ty}").expect("string writes should not fail");
                }
            }
            ItemKind::Enum(enum_item) => {
                self.render_generics(&enum_item.generics, depth, dump);
                for variant in &enum_item.variants {
                    writeln!(dump, "{indent}  - variant {}", variant.name)
                        .expect("string writes should not fail");
                    self.render_fields(&variant.fields, depth + 2, dump);
                }
            }
            ItemKind::Function(function_item) => {
                self.render_generics(&function_item.generics, depth, dump);
                let params = function_item
                    .params
                    .iter()
                    .map(render_param)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(dump, "{indent}  - params ({params})")
                    .expect("string writes should not fail");
                if let Some(ret_ty) = &function_item.ret_ty {
                    writeln!(dump, "{indent}  - ret {ret_ty}")
                        .expect("string writes should not fail");
                }
            }
            ItemKind::Impl(impl_item) => {
                self.render_generics(&impl_item.generics, depth, dump);
                if let Some(trait_ref) = &impl_item.trait_ref {
                    writeln!(dump, "{indent}  - trait {trait_ref}")
                        .expect("string writes should not fail");
                }
                writeln!(dump, "{indent}  - self {}", impl_item.self_ty)
                    .expect("string writes should not fail");
                for child_id in &impl_item.items {
                    let child = file_tree
                        .item(*child_id)
                        .expect("impl child item id should exist while rendering declarations");
                    self.render_item(file_tree, child, depth + 1, dump);
                }
            }
            ItemKind::Static(static_item) => {
                if let Some(ty) = &static_item.ty {
                    writeln!(dump, "{indent}  - ty {ty}").expect("string writes should not fail");
                }
            }
            ItemKind::Struct(struct_item) => {
                self.render_generics(&struct_item.generics, depth, dump);
                self.render_fields(&struct_item.fields, depth + 1, dump);
            }
            ItemKind::Trait(trait_item) => {
                self.render_generics(&trait_item.generics, depth, dump);
                if !trait_item.super_traits.is_empty() {
                    writeln!(
                        dump,
                        "{indent}  - supertraits {}",
                        trait_item
                            .super_traits
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                    .expect("string writes should not fail");
                }
                for child_id in &trait_item.items {
                    let child = file_tree
                        .item(*child_id)
                        .expect("trait child item id should exist while rendering declarations");
                    self.render_item(file_tree, child, depth + 1, dump);
                }
            }
            ItemKind::TypeAlias(type_alias) => {
                self.render_generics(&type_alias.generics, depth, dump);
                if !type_alias.bounds.is_empty() {
                    writeln!(
                        dump,
                        "{indent}  - bounds {}",
                        type_alias
                            .bounds
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                    .expect("string writes should not fail");
                }
                if let Some(aliased_ty) = &type_alias.aliased_ty {
                    writeln!(dump, "{indent}  - aliased {aliased_ty}")
                        .expect("string writes should not fail");
                }
            }
            ItemKind::Union(union_item) => {
                self.render_generics(&union_item.generics, depth, dump);
                self.render_named_fields(&union_item.fields, depth + 1, dump);
            }
            _ => {}
        }
    }

    fn render_generics(&self, generics: &crate::GenericParams, depth: usize, dump: &mut String) {
        let generics = generics.to_string();
        if generics.is_empty() {
            return;
        }

        writeln!(dump, "{}  - generics {generics}", "  ".repeat(depth))
            .expect("string writes should not fail");
    }

    fn render_fields(&self, fields: &FieldList, depth: usize, dump: &mut String) {
        match fields {
            FieldList::Named(fields) => self.render_named_fields(fields, depth, dump),
            FieldList::Tuple(fields) => {
                for (idx, field) in fields.iter().enumerate() {
                    writeln!(
                        dump,
                        "{}- {}field #{idx}: {}",
                        "  ".repeat(depth),
                        visibility_prefix(&field.visibility),
                        field.ty,
                    )
                    .expect("string writes should not fail");
                }
            }
            FieldList::Unit => {}
        }
    }

    fn render_named_fields(&self, fields: &[FieldItem], depth: usize, dump: &mut String) {
        for field in fields {
            writeln!(
                dump,
                "{}- {}field {}: {}",
                "  ".repeat(depth),
                visibility_prefix(&field.visibility),
                field
                    .key
                    .as_ref()
                    .map(|key| key.declaration_label())
                    .unwrap_or_else(|| "<missing>".to_string()),
                field.ty,
            )
            .expect("string writes should not fail");
        }
    }

    fn render_module_source(&self, source: &ModuleSource) -> String {
        match source {
            ModuleSource::Inline { .. } => "inline".to_string(),
            ModuleSource::OutOfLine {
                definition_file: Some(file_id),
            } => {
                format!("out_of_line {}", self.file_label(*file_id))
            }
            ModuleSource::OutOfLine {
                definition_file: None,
            } => "out_of_line <missing>".to_string(),
        }
    }

    fn file_label(&self, file_id: rg_parse::FileId) -> String {
        file_label(self.package, file_id)
    }
}

#[derive(Debug, Clone, Copy)]
enum SnapshotMode {
    Structure,
    Declarations,
    Spans,
}

fn render_param(param: &crate::ParamItem) -> String {
    match (param.kind, &param.ty) {
        (ParamKind::SelfParam, _) => param.pat.clone(),
        (ParamKind::Normal, Some(ty)) => format!("{}: {ty}", param.pat),
        (ParamKind::Normal, None) => param.pat.clone(),
    }
}

fn visibility_prefix(visibility: &VisibilityLevel) -> String {
    match visibility {
        VisibilityLevel::Private => String::new(),
        _ => format!("{visibility} "),
    }
}

fn sorted_packages(parse: &ParseDb) -> Vec<(usize, &Package)> {
    let mut packages = parse.packages().iter().enumerate().collect::<Vec<_>>();
    packages.sort_by(|left, right| left.1.package_name().cmp(right.1.package_name()));
    packages
}

fn sorted_item_tree_target_roots<'a>(
    package: &Package,
    item_trees: &'a ItemTreePackage,
) -> Vec<&'a TargetRoot> {
    let mut target_roots = item_trees.target_roots().iter().collect::<Vec<_>>();
    target_roots.sort_by(|left, right| {
        let left_target = package
            .target(left.target)
            .expect("parsed target should exist while sorting item-tree target roots");
        let right_target = package
            .target(right.target)
            .expect("parsed target should exist while sorting item-tree target roots");

        (
            left_target.kind.sort_order(),
            left_target.name.as_str(),
            left_target.src_path.as_path(),
        )
            .cmp(&(
                right_target.kind.sort_order(),
                right_target.name.as_str(),
                right_target.src_path.as_path(),
            ))
    });
    target_roots
}

fn sorted_item_tree_files<'a>(
    package: &Package,
    item_trees: &'a ItemTreePackage,
) -> Vec<&'a FileTree> {
    let mut files = item_trees.files().collect::<Vec<_>>();
    files.sort_by(|left, right| {
        let left_path = package
            .file_path(left.file)
            .expect("item-tree file should exist while sorting");
        let right_path = package
            .file_path(right.file)
            .expect("item-tree file should exist while sorting");
        left_path.cmp(right_path)
    });
    files
}

fn file_label(package: &Package, file_id: FileId) -> String {
    package
        .file_path(file_id)
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}
