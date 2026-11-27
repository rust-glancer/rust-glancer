//! The action labels also identify the slots in a trial that was interrupted.

use crate::benchmark::Workload;

#[derive(Clone, Copy)]
pub(super) struct Action {
    pub(super) label: &'static str,
    pub(super) method: &'static str,
    pub(super) foreground: bool,
}

impl Action {
    pub(super) const FIRST_HOVER: Self = Self::hover("First usable hover");
    pub(super) const EDITOR_HINTS: Self = Self::hints("Editor hints");
    pub(super) const HOVER_CALL: Self = Self::hover("Hover call");
    pub(super) const DEFINITION: Self = Self::definition("Go to definition");
    pub(super) const EDIT_COMPLETION: Self = Self::completion("Complete while editing");
    pub(super) const SAVED_TYPE: Self = Self::hover("Save and read changed type");

    pub(super) const FIRST_HINTS: Self = Self::hints("First editor hints");
    pub(super) const REPEAT_HINTS: Self = Self::hints("Repeat editor hints");
    pub(super) const FIRST_ISOLATED_HOVER: Self = Self::hover("First isolated hover");
    pub(super) const REPEAT_ISOLATED_HOVER: Self = Self::hover("Repeat isolated hover");
    pub(super) const FIRST_ISOLATED_HINTS: Self = Self::hints("First isolated editor hints");
    pub(super) const REPEAT_ISOLATED_HINTS: Self = Self::hints("Repeat isolated editor hints");

    pub(super) const PROJECT_HINTS: Self = Self::hints("Project editor hints");
    pub(super) const PROJECT_DEFINITION: Self = Self::definition("Go to project definition");
    pub(super) const NEW_MODULE: Self = Self::definition("Go to new module after save");
    pub(super) const COMPLETE_IMPORT: Self = Self::completion("Complete ItemLimit import");
    pub(super) const FIND_AUTO_IMPORT: Self = Self::completion("Find ItemLimit auto-import");
    pub(super) const RESOLVE_AUTO_IMPORT: Self =
        Self::foreground("Resolve and accept auto-import", "completionItem/resolve");
    pub(super) const SIGNATURE: Self =
        Self::foreground("Constructor signature", "textDocument/signatureHelp");
    pub(super) const COMPLETE_FIELD: Self = Self::completion("Complete batch field");
    pub(super) const BATCH_DEFINITION: Self = Self::definition("Go to batch function");
    pub(super) const BATCH_TYPE: Self =
        Self::foreground("Go to batch type", "textDocument/typeDefinition");
    pub(super) const TEST_HINTS: Self = Self::hints("Test inlay hints");
    pub(super) const REFERENCES: Self =
        Self::foreground("Find local batch references", "textDocument/references");
    pub(super) const IMPLEMENTATION: Self =
        Self::foreground("Find Measure implementation", "textDocument/implementation");
    pub(super) const OUTLINE: Self =
        Self::background("Outline the saved API", "textDocument/documentSymbol");

    const API_QUERIES: &'static [Self] = &[
        Self::SIGNATURE,
        Self::COMPLETE_FIELD,
        Self::BATCH_DEFINITION,
        Self::BATCH_TYPE,
        Self::TEST_HINTS,
        Self::REFERENCES,
        Self::IMPLEMENTATION,
        Self::OUTLINE,
    ];

    pub(super) fn planned(workload: Workload) -> impl Iterator<Item = Self> {
        let (opening, continuation): (&[Self], &[Self]) = match workload {
            Workload::Editor => (
                &[
                    Self::EDITOR_HINTS,
                    Self::HOVER_CALL,
                    Self::DEFINITION,
                    Self::EDIT_COMPLETION,
                    Self::SAVED_TYPE,
                ],
                &[],
            ),
            Workload::FirstOpen | Workload::Restart | Workload::BuildOutputs => {
                (&[Self::FIRST_HOVER], &[])
            }
            Workload::FirstHints => (&[Self::FIRST_HINTS, Self::REPEAT_HINTS], &[]),
            Workload::IsolatedRequests => (
                &[
                    Self::FIRST_ISOLATED_HOVER,
                    Self::REPEAT_ISOLATED_HOVER,
                    Self::FIRST_ISOLATED_HINTS,
                    Self::REPEAT_ISOLATED_HINTS,
                ],
                &[],
            ),
            Workload::UserSimulation => (
                &[
                    Self::FIRST_HOVER,
                    Self::PROJECT_HINTS,
                    Self::PROJECT_DEFINITION,
                    Self::NEW_MODULE,
                    Self::COMPLETE_IMPORT,
                ],
                Self::API_QUERIES,
            ),
            Workload::LibraryEditing => (
                &[Self::FIND_AUTO_IMPORT, Self::RESOLVE_AUTO_IMPORT],
                Self::API_QUERIES,
            ),
        };
        opening.iter().chain(continuation).copied()
    }

    const fn foreground(label: &'static str, method: &'static str) -> Self {
        Self {
            label,
            method,
            foreground: true,
        }
    }

    const fn background(label: &'static str, method: &'static str) -> Self {
        Self {
            label,
            method,
            foreground: false,
        }
    }

    const fn hover(label: &'static str) -> Self {
        Self::foreground(label, "textDocument/hover")
    }

    const fn definition(label: &'static str) -> Self {
        Self::foreground(label, "textDocument/definition")
    }

    const fn completion(label: &'static str) -> Self {
        Self::foreground(label, "textDocument/completion")
    }

    const fn hints(label: &'static str) -> Self {
        Self::background(label, "textDocument/inlayHint")
    }
}
