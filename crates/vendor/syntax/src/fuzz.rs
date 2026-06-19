//! Some infrastructure for fuzzy testing.
//!
//! We don't normally run fuzzying, so this is hopelessly bitrotten :(

use parser::Edition;

use crate::{AstNode, SourceFile, validation};

fn check_file_invariants(file: &SourceFile) {
    let root = file.syntax();
    validation::validate_block_structure(root);
}

pub fn check_parser(text: &str) {
    let file = SourceFile::parse(text, Edition::CURRENT);
    check_file_invariants(&file.tree());
}
