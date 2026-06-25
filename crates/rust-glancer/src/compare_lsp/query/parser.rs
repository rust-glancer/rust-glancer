//! Parser for compact hardcoded LSP query vectors.

use crate::compare_lsp::query::{QueryCase, QueryKind, SourcePosition};

pub(super) fn parse_query_cases(input: &'static str) -> Vec<QueryCase> {
    let mut cases = Vec::new();
    let mut current_kind = None;

    for (line_index, raw_line) in input.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(kind) = parse_header(line, line_number) {
            current_kind = Some(kind);
            continue;
        }

        let kind = current_kind.unwrap_or_else(|| {
            panic!("rust-analyzer LSP query case before method header on line {line_number}")
        });
        cases.push(parse_query_case(line, line_number, kind));
    }

    cases
}

fn parse_header(line: &'static str, line_number: usize) -> Option<QueryKind> {
    if !line.starts_with('[') {
        return None;
    }

    let method = line
        .strip_prefix('[')
        .and_then(|line| line.strip_suffix(']'))
        .unwrap_or_else(|| {
            panic!("invalid rust-analyzer LSP query header on line {line_number}: `{line}`")
        });

    Some(parse_query_kind(method, line_number))
}

fn parse_query_case(line: &'static str, line_number: usize, kind: QueryKind) -> QueryCase {
    let (spec, label) = line
        .split_once('#')
        .unwrap_or_else(|| panic!("missing `# <label>` suffix on line {line_number}: `{line}`"));
    let label = label.trim();
    if label.is_empty() {
        panic!("query label is empty on line {line_number}: `{line}`");
    }

    let input = spec.trim();
    if input.is_empty() {
        panic!("missing query input on line {line_number}: `{line}`");
    }

    match kind {
        QueryKind::References { .. }
        | QueryKind::GotoDefinition
        | QueryKind::TypeDefinition
        | QueryKind::Implementation
        | QueryKind::DocumentHighlight
        | QueryKind::Hover => {
            let (source_path, position) = parse_location(input, line_number);
            QueryCase::position(label, kind, source_path, position)
        }
        QueryKind::DocumentSymbol => QueryCase::file(label, kind, input),
        QueryKind::WorkspaceSymbol => QueryCase::workspace_query(label, kind, input),
    }
}

fn parse_location(location: &'static str, line_number: usize) -> (&'static str, SourcePosition) {
    let (path_and_line, character) = location.rsplit_once(':').unwrap_or_else(|| {
        panic!("source location must be `path:line:character` on line {line_number}: `{location}`")
    });
    let (source_path, line) = path_and_line.rsplit_once(':').unwrap_or_else(|| {
        panic!("source location must be `path:line:character` on line {line_number}: `{location}`")
    });
    if source_path.is_empty() {
        panic!("source path is empty on line {line_number}: `{location}`");
    }

    let line = parse_u32("line", line, line_number);
    let character = parse_u32("character", character, line_number);
    (source_path, SourcePosition::new(line, character))
}

fn parse_query_kind(method: &'static str, line_number: usize) -> QueryKind {
    match method {
        "textDocument/references(includeDeclaration=true)" => QueryKind::References {
            include_declaration: true,
        },
        "textDocument/references(includeDeclaration=false)" => QueryKind::References {
            include_declaration: false,
        },
        method if method == QueryKind::GotoDefinition.lsp_method() => QueryKind::GotoDefinition,
        method if method == QueryKind::TypeDefinition.lsp_method() => QueryKind::TypeDefinition,
        method if method == QueryKind::Implementation.lsp_method() => QueryKind::Implementation,
        method if method == QueryKind::DocumentHighlight.lsp_method() => {
            QueryKind::DocumentHighlight
        }
        method if method == QueryKind::DocumentSymbol.lsp_method() => QueryKind::DocumentSymbol,
        method if method == QueryKind::WorkspaceSymbol.lsp_method() => QueryKind::WorkspaceSymbol,
        method if method == QueryKind::Hover.lsp_method() => QueryKind::Hover,
        _ => panic!("unsupported LSP query method `{method}` on line {line_number}"),
    }
}

fn parse_u32(name: &str, value: &str, line_number: usize) -> u32 {
    value
        .parse()
        .unwrap_or_else(|_| panic!("{name} `{value}` is not a valid u32 on line {line_number}"))
}
