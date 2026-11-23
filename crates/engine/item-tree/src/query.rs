use std::sync::Arc;

use rg_ir_model::{FileId, PackageSlot, Span};
use rg_source::QueryContext;
use rg_std::Shrink as _;
use rg_workspace::PackageInput;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    DeclarationSourceStore, ExpandedItemSyntax, FileTree, ItemTreeId, lower::FileLowering,
};

/// Declaration meaning and separate source data from lowering one file.
///
/// Adding a comment before `fn load() -> User` changes its positions, but can leave its lowered
/// declaration unchanged. Separate outputs let semantic queries depend on `items` without also
/// depending on `sources`. A body from a `cfg_select!` arm reads the tokens for its arm,
/// since that body is inside a token tree in the ordinary file parse.
#[ajika::output]
pub struct FileDeclarations {
    pub items: FileTree,
    pub sources: DeclarationSourceStore<Span>,
    pub cfg_select_bodies: FileExpandedBodies,
}

/// Lowers one file in its package context. Macro spans retain both the package slot and the
/// canonical file ID. Module membership is discovered separately because it also depends on
/// the logical directory from which the file was reached.
#[ajika::query]
pub fn file_declarations(
    db: &QueryContext<'_>,
    package: PackageSlot,
    file: FileId,
) -> ajika::Result<FileDeclarations> {
    let edition = PackageInput::at(package).edition(db)?;
    let syntax = rg_parse::source_syntax(db, file, edition)?;
    let (mut tree, mut sources, bodies) = FileLowering::lower(package, file, edition, &syntax);
    sources.shrink_to_fit();
    tree.shrink_to_fit();
    let cfg_select_bodies = FileExpandedBodies::new(db, bodies)?;
    FileDeclarations::new(db, tree, sources, cfg_select_bodies)
}

/// Finds the expansion for a body whose declaration belongs to a file's item tree.
///
/// File lowering fills this index for `cfg_select!` arms. An item in an arm has a file-local
/// `ItemTreeId`, and its body syntax comes from parsing the arm's tokens. The body items in
/// one arm share a packed-token output that the reader can open on demand.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct FileExpandedBodies {
    items: Vec<(ItemTreeId, usize)>,
    expansions: Vec<ajika::Output<ExpandedItemSyntax>>,
}

impl FileExpandedBodies {
    fn new(db: &QueryContext<'_>, bodies: Vec<ExpandedItemSyntax>) -> ajika::Result<Self> {
        let mut items = Vec::new();
        let mut expansions = Vec::with_capacity(bodies.len());
        for (index, bodies) in bodies.into_iter().enumerate() {
            items.extend(bodies.body_items.iter().map(|(item, _)| (*item, index)));
            expansions.push(db.output(("cfg body tokens", index), Arc::new(bodies))?);
        }
        items.sort_unstable_by_key(|(item, _)| item.0);
        Ok(Self { items, expansions })
    }

    pub fn get(&self, item: ItemTreeId) -> Option<&ajika::Output<ExpandedItemSyntax>> {
        let index = self
            .items
            .binary_search_by_key(&item.0, |(item, _)| item.0)
            .ok()?;
        self.expansions.get(self.items[index].1)
    }
}

/// Acquiring source data is an explicit dependency, separate from reading declaration meaning.
/// Read it from the operation's query context so reused declaration IDs resolve to that snapshot's
/// positions, including after an edit that only moved the declaration.
pub fn file_declaration_sources<'db>(
    db: &'db QueryContext<'db>,
    package: PackageSlot,
    file: FileId,
) -> ajika::Result<&'db DeclarationSourceStore<Span>> {
    file_declarations(db, package, file)?.sources.value_ref(db)
}
