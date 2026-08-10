//! Attribute path and attribute-input classification.

use rg_syntax::{AstNode as _, SyntaxKind, ast};

use crate::query::completion::site::{AttributeCompletionContext, AttributeCompletionKind};

use super::CompletionSyntaxContext;

impl CompletionSyntaxContext<'_> {
    /// Select the small grammar owned by the attribute around the cursor.
    ///
    /// The result also remembers already-written comma-list entries so accepting completion does
    /// not suggest a second `Debug`, lint name, representation, or compatibility key.
    pub(super) fn attribute_completion_context(&self) -> Option<AttributeCompletionContext> {
        let attr = self
            .marker
            .parent()?
            .ancestors()
            .find_map(ast::Attr::cast)?;
        let attr_text = attr.syntax().text().to_string();

        if let Some(path) = attr.path()
            && path.syntax().text().to_string().contains(Self::MARKER)
        {
            let qualifier = Self::marker_path_qualifier(&path.syntax().text().to_string())?;
            return Some(AttributeCompletionContext::new(
                AttributeCompletionKind::Path { qualifier },
            ));
        }

        let attr_path = attr.path().map(|path| path.syntax().text().to_string());
        let simple_name = attr.simple_name().map(|name| name.to_string());
        let existing = Self::comma_list_entries(&attr_text);
        let kind = match simple_name.as_deref() {
            Some("derive") => AttributeCompletionKind::Derive {
                qualifier: Self::path_around_marker(&attr_text).flatten(),
                existing,
            },
            Some("allow" | "warn" | "deny" | "forbid" | "expect") => {
                AttributeCompletionKind::Lint { existing }
            }
            Some("repr") => AttributeCompletionKind::Repr { existing },
            Some("cfg" | "cfg_attr") => {
                if self.marker.kind() == SyntaxKind::STRING
                    && attr_text
                        .get(..attr_text.find(Self::MARKER)?)?
                        .rsplit_once("feature")
                        .is_some_and(|(_, tail)| tail.contains('='))
                {
                    AttributeCompletionKind::CfgFeature {
                        existing: Self::quoted_values_after(&attr_text, "feature"),
                    }
                } else {
                    AttributeCompletionKind::Cfg
                }
            }
            Some("deprecated" | "stable" | "unstable" | "rustc_const_stable") => {
                AttributeCompletionKind::Compatibility {
                    attribute: simple_name.expect("matched attribute should have a name"),
                    existing,
                }
            }
            _ if attr_path.as_deref() == Some("diagnostic::on_unimplemented") => {
                AttributeCompletionKind::Diagnostic { existing }
            }
            Some(_) | None => return None,
        };
        Some(AttributeCompletionContext::new(kind))
    }

    /// Read complete sibling entries from a parenthesized comma list around the marker.
    pub(super) fn comma_list_entries(text: &str) -> Vec<String> {
        let Some((_, body)) = text.split_once('(') else {
            return Vec::new();
        };
        let body = body.rsplit_once(')').map_or(body, |(body, _)| body);
        body.split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty() && !entry.contains(Self::MARKER))
            .map(ToString::to_string)
            .collect()
    }

    fn quoted_values_after(text: &str, key: &str) -> Vec<String> {
        text.split(key)
            .skip(1)
            .filter_map(|tail| tail.split_once('"').map(|(_, value)| value))
            .filter_map(|value| value.split_once('"').map(|(value, _)| value.to_string()))
            .filter(|value| !value.contains(Self::MARKER))
            .collect()
    }
}
