pub mod cli;
pub mod model;
pub mod text;

/// Gives the VS Code integration test one unambiguous semantic member completion.
pub struct CompletionFixture {
    pub semantic_member: usize,
}

pub fn completion_fixture(value: CompletionFixture) -> usize {
    value.semantic_member
}
