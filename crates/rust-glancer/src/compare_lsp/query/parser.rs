//! Parser for compact hardcoded LSP query vectors.

use crate::compare_lsp::query::{QueryCase, QueryKind, SourcePosition};

pub(super) fn parse_query_cases(input: &'static str) -> Result<Vec<QueryCase>, String> {
    let mut cases = Vec::new();

    for (line_index, raw_line) in input.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        cases.push(parse_query_case(line).map_err(|error| {
            format!("invalid rust-analyzer LSP query case on line {line_number}: {error}")
        })?);
    }

    Ok(cases)
}

fn parse_query_case(line: &'static str) -> Result<QueryCase, String> {
    let (spec, label) = line
        .split_once('#')
        .ok_or_else(|| "missing `# <label>` suffix".to_string())?;
    let label = label.trim();
    if label.is_empty() {
        return Err("query label is empty".to_string());
    }

    let mut parts = spec.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "missing LSP method".to_string())?;
    let location = parts
        .next()
        .ok_or_else(|| "missing source location".to_string())?;
    if parts.next().is_some() {
        return Err("unexpected extra fields before label".to_string());
    }

    let (source_path, position) = parse_location(location)?;
    let kind = parse_query_kind(method)?;

    Ok(QueryCase::new(label, source_path, position, kind))
}

fn parse_location(location: &'static str) -> Result<(&'static str, SourcePosition), String> {
    let (path_and_line, character) = location
        .rsplit_once(':')
        .ok_or_else(|| "source location must be `path:line:character`".to_string())?;
    let (source_path, line) = path_and_line
        .rsplit_once(':')
        .ok_or_else(|| "source location must be `path:line:character`".to_string())?;
    if source_path.is_empty() {
        return Err("source path is empty".to_string());
    }

    let line = parse_u32("line", line)?;
    let character = parse_u32("character", character)?;
    Ok((source_path, SourcePosition::new(line, character)))
}

fn parse_query_kind(method: &'static str) -> Result<QueryKind, String> {
    let (method, params) = parse_method_spec(method)?;
    match method {
        method
            if method
                == (QueryKind::References {
                    include_declaration: true,
                })
                .lsp_method() =>
        {
            let Some(params) = params else {
                return Err("textDocument/references requires includeDeclaration".to_string());
            };
            Ok(QueryKind::References {
                include_declaration: parse_include_declaration(params)?,
            })
        }
        method if method == QueryKind::GotoDefinition.lsp_method() => {
            ensure_no_params(method, params)?;
            Ok(QueryKind::GotoDefinition)
        }
        method if method == QueryKind::Hover.lsp_method() => {
            ensure_no_params(method, params)?;
            Ok(QueryKind::Hover)
        }
        _ => Err(format!("unsupported LSP method `{method}`")),
    }
}

fn parse_method_spec(method: &'static str) -> Result<(&'static str, Option<&'static str>), String> {
    let Some((name, params)) = method.split_once('(') else {
        return Ok((method, None));
    };
    let params = params
        .strip_suffix(')')
        .ok_or_else(|| "method parameters must end with `)`".to_string())?;
    if name.is_empty() {
        return Err("LSP method is empty".to_string());
    }
    Ok((name, Some(params)))
}

fn parse_include_declaration(params: &'static str) -> Result<bool, String> {
    let Some(value) = params.strip_prefix("includeDeclaration=") else {
        return Err("references params must be `includeDeclaration=true|false`".to_string());
    };
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("includeDeclaration must be `true` or `false`".to_string()),
    }
}

fn ensure_no_params(method: &str, params: Option<&str>) -> Result<(), String> {
    if params.is_some() {
        return Err(format!("`{method}` does not accept query-case parameters"));
    }
    Ok(())
}

fn parse_u32(name: &str, value: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("{name} `{value}` is not a valid u32"))
}
