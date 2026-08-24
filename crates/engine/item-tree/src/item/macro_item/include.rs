//! Retains the small path language used by source-like `include!` calls.
//!
//! A literal include such as `include!("generated.rs")` can be resolved while ItemTree still has
//! the call-site file. Build scripts more commonly produce a shape like
//! `include!(concat!(env!("OUT_DIR"), "/bindings.rs"))`, where resolution also needs the generated
//! source list and compile-time environment recovered from a prior Cargo build. An equivalent call
//! may only become visible after a declarative macro expands; in that case DefMap carries this value
//! through its macro source-file request boundary.
//!
//! Keeping syntax and resolution separate lets both paths use the same deliberately small model.
//! It is not general constant evaluation: unsupported expressions simply remain unresolved, and
//! resolution never executes a macro or reads environment variables from the LSP process.

use std::path::{Path, PathBuf};

use rg_syntax::{AstNode as _, SyntaxKind, ast};
use rg_text::RustEdition;
use rg_tt::TopSubtree;

/// Small compile-time string language accepted for include paths.
///
/// String literals, `env!("NAME")`, parentheses, and nested `concat!(...)` calls are flattened into
/// literal and environment components. That covers ordinary relative includes and the common
/// `OUT_DIR` build-script form without pretending to evaluate arbitrary Rust expressions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IncludePathExpression {
    components: Vec<IncludePathComponent>,
}

impl IncludePathExpression {
    pub(crate) fn from_macro_call(item: &ast::MacroCall, edition: RustEdition) -> Option<Self> {
        if Self::macro_call_terminal_name(item).as_deref() != Some("include") {
            return None;
        }
        Self::from_arguments(&Self::macro_arguments_text(item)?, edition)
    }

    /// Parses the argument token tree retained for a generated `include!` call.
    pub fn from_macro_arguments(arguments: &TopSubtree, edition: RustEdition) -> Option<Self> {
        Self::from_arguments(&arguments.view().token_trees().to_string(), edition)
    }

    fn from_arguments(arguments: &str, edition: RustEdition) -> Option<Self> {
        let [expression] = Self::parse_arguments(arguments, edition)?.try_into().ok()?;
        Some(Self {
            components: Self::components(expression, edition)?,
        })
    }

    fn components(
        expression: ast::Expr,
        edition: RustEdition,
    ) -> Option<Vec<IncludePathComponent>> {
        match expression {
            ast::Expr::Literal(literal) => {
                let ast::LiteralKind::String(value) = literal.kind() else {
                    return None;
                };
                Some(vec![IncludePathComponent::Literal(
                    value.value().ok()?.into_owned(),
                )])
            }
            ast::Expr::ParenExpr(expression) => Self::components(expression.expr()?, edition),
            ast::Expr::MacroExpr(expression) => {
                let call = expression.macro_call()?;
                let arguments = Self::macro_arguments_text(&call)?;
                match Self::macro_call_terminal_name(&call)?.as_str() {
                    "env" => {
                        let [argument] = Self::parse_arguments(&arguments, edition)?
                            .try_into()
                            .ok()?;
                        let ast::Expr::Literal(literal) = argument else {
                            return None;
                        };
                        let ast::LiteralKind::String(value) = literal.kind() else {
                            return None;
                        };
                        Some(vec![IncludePathComponent::Environment(
                            value.value().ok()?.into_owned(),
                        )])
                    }
                    "concat" => {
                        let mut components = Vec::new();
                        for argument in Self::parse_arguments(&arguments, edition)? {
                            components.extend(Self::components(argument, edition)?);
                        }
                        Some(components)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Resolves this expression from a real call site and optional Cargo-generated sources.
    ///
    /// Literal-only paths are relative to `current_file`, matching normal `include!` behavior.
    /// Expressions with environment components first use values retained for the selected Cargo
    /// unit, and accept the result only when rustc dep-info named that concrete file. As an 80/20
    /// fallback, `concat!(env!("OUT_DIR"), <literal suffix>)` can select a unique generated file by
    /// its path below the recovered output directory. That structural path still requires one
    /// unique dep-info match; it does not guess from files that merely happen to exist.
    pub fn resolve(
        &self,
        current_file: &rg_parse::ParsedFile<'_>,
        generated_sources: Option<&rg_workspace::CargoGeneratedSources>,
    ) -> Option<PathBuf> {
        // Source-relative includes do not need Cargo-generated source metadata at all.
        if self
            .components
            .iter()
            .all(|component| matches!(component, IncludePathComponent::Literal(_)))
        {
            let path = self.render_with(|_| None)?;
            return current_file.resolve_path(&path);
        }

        let generated_sources = generated_sources?;

        // Prefer the exact compile-time environment captured beside the same historical Cargo
        // unit. Dep-info still acts as the allow-list, so a stale or foreign value cannot pull an
        // arbitrary file into analysis.
        if let Some(path) = self.render_with(|name| generated_sources.compile_env_value(name)) {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                current_file.resolve_path(path.to_str()?)?
            };
            if generated_sources
                .generated_files()
                .iter()
                .any(|known| known == &path)
            {
                return Some(path);
            }
        }

        // Keep the common OUT_DIR-plus-literal form as a separate structural proof. This avoids
        // depending entirely on string rendering: the suffix must name one unique dep-info input
        // below the attributed unit's concrete output directory.
        let suffix = self.out_dir_suffix()?;
        generated_sources
            .generated_file_for_out_dir_suffix(&suffix)
            .map(Path::to_path_buf)
    }

    fn render_with<'a>(&self, compile_env: impl Fn(&str) -> Option<&'a str>) -> Option<String> {
        let mut path = String::new();
        for component in &self.components {
            match component {
                IncludePathComponent::Literal(value) => path.push_str(value),
                IncludePathComponent::Environment(name) => path.push_str(compile_env(name)?),
            }
        }
        Some(path)
    }

    fn out_dir_suffix(&self) -> Option<PathBuf> {
        let [IncludePathComponent::Environment(name), suffix @ ..] = self.components.as_slice()
        else {
            return None;
        };
        if name != "OUT_DIR" {
            return None;
        }

        let mut rendered = String::new();
        for component in suffix {
            let IncludePathComponent::Literal(value) = component else {
                return None;
            };
            rendered.push_str(value);
        }
        Some(PathBuf::from(rendered))
    }

    fn parse_arguments(arguments: &str, edition: RustEdition) -> Option<Vec<ast::Expr>> {
        let arguments = arguments.trim();
        if arguments.is_empty() {
            return Some(Vec::new());
        }
        let arguments = arguments.strip_suffix(',').unwrap_or(arguments);
        let tuple = format!("({arguments},)");
        let ast::Expr::TupleExpr(tuple) = ast::Expr::parse(&tuple, Self::syntax_edition(edition))
            .ok()
            .ok()?
        else {
            return None;
        };
        Some(tuple.fields().collect())
    }

    fn macro_arguments_text(item: &ast::MacroCall) -> Option<String> {
        let token_tree = item.token_tree()?;
        let tokens = token_tree
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .collect::<Vec<_>>();
        let (open, close) = (tokens.first()?, tokens.last()?);
        if !Self::matching_delimiters(open.kind(), close.kind()) {
            return None;
        }

        let mut arguments = String::new();
        for token in &tokens[1..tokens.len() - 1] {
            arguments.push_str(token.text());
        }
        Some(arguments)
    }

    fn macro_call_terminal_name(item: &ast::MacroCall) -> Option<String> {
        item.path()?
            .segment()?
            .name_ref()
            .map(|name| name.text().to_string())
    }

    fn syntax_edition(edition: RustEdition) -> rg_syntax::Edition {
        match edition {
            RustEdition::Edition2015 => rg_syntax::Edition::Edition2015,
            RustEdition::Edition2018 => rg_syntax::Edition::Edition2018,
            RustEdition::Edition2021 => rg_syntax::Edition::Edition2021,
            RustEdition::Edition2024 => rg_syntax::Edition::Edition2024,
        }
    }

    fn matching_delimiters(open: SyntaxKind, close: SyntaxKind) -> bool {
        matches!(
            (open, close),
            (SyntaxKind::L_PAREN, SyntaxKind::R_PAREN)
                | (SyntaxKind::L_CURLY, SyntaxKind::R_CURLY)
                | (SyntaxKind::L_BRACK, SyntaxKind::R_BRACK)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum IncludePathComponent {
    Literal(String),
    Environment(String),
}

#[cfg(test)]
mod tests {
    use rg_syntax::{AstNode as _, Edition, SourceFile, ast};
    use rg_text::RustEdition;

    use super::{IncludePathComponent, IncludePathExpression};

    #[test]
    fn parses_nested_concat_and_environment_components() {
        let expression =
            parse(r#"include!(concat!(env!("OUT_DIR"), concat!("/", env!("FILE"))));"#)
                .expect("supported include expression should parse");

        assert_eq!(
            expression.components,
            vec![
                IncludePathComponent::Environment("OUT_DIR".to_string()),
                IncludePathComponent::Literal("/".to_string()),
                IncludePathComponent::Environment("FILE".to_string()),
            ]
        );
        assert_eq!(
            expression.render_with(|name| match name {
                "OUT_DIR" => Some("/tmp/out"),
                "FILE" => Some("generated.rs"),
                _ => None,
            }),
            Some("/tmp/out/generated.rs".to_string())
        );
    }

    #[test]
    fn leaves_unsupported_or_incomplete_path_languages_unresolved() {
        for source in [
            r#"include!(option_env!("OUT_DIR"));"#,
            r#"include!(concat!(env!("OUT_DIR"), GENERATED_FILE));"#,
            r#"include!(env!("OUT_DIR", "custom error"));"#,
            r#"include!(format!("generated.rs"));"#,
        ] {
            assert!(parse(source).is_none(), "{source} should stay unsupported");
        }

        let expression = parse(r#"include!(concat!(env!("CUSTOM_ROOT"), "/file.rs"));"#)
            .expect("supported environment expression should parse");
        assert!(expression.render_with(|_| None).is_none());
        assert!(expression.out_dir_suffix().is_none());
    }

    fn parse(source: &str) -> Option<IncludePathExpression> {
        let file = SourceFile::parse(source, Edition::Edition2024).ok().ok()?;
        let call = file.syntax().descendants().find_map(ast::MacroCall::cast)?;
        IncludePathExpression::from_macro_call(&call, RustEdition::Edition2024)
    }
}
