//! Adapt declaration fragments to the complete syntax tree expected by prettyplease.

/// Give long parameter lists and where clauses their own indented lines in a hover.
pub(crate) fn function_signature(signature: &str) -> Option<String> {
    // Hover signatures omit the body. Supply an empty one for the formatter, then remove it
    // again so the hover does not suggest that the actual function has an empty body.
    // Parsing can fail for display placeholders such as `<unsupported>`; in that case the
    // caller keeps the original signature so its available type information is still shown.
    let function = syn::parse_str::<syn::ItemFn>(&format!("{signature} {{}}")).ok()?;
    let file = syn::File {
        shebang: None,
        attrs: Vec::new(),
        items: vec![syn::Item::Fn(function)],
    };
    let formatted = prettyplease::unparse(&file);
    Some(
        formatted
            .trim_end()
            .strip_suffix("{}")?
            .trim_end()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::function_signature;

    #[test]
    fn formats_function_and_method_signatures() {
        for (input, expected, message) in [
            (
                "pub fn make() -> u8",
                "pub fn make() -> u8",
                "short function",
            ),
            (
                "pub fn visit<F>(&self, callback: F) where F: for<'a> FnOnce(&'a str) -> &'a str",
                "pub fn visit<F>(&self, callback: F)\nwhere\n    F: for<'a> FnOnce(&'a str) -> &'a str,",
                "method with a higher-ranked callback bound",
            ),
            (
                "pub fn transform(&mut self, values: &[(u32, u64)], callback: fn(u32, u64) -> Result<(u32, u64), Error>) -> Result<(u32, u64), Error>",
                "pub fn transform(\n    &mut self,\n    values: &[(u32, u64)],\n    callback: fn(u32, u64) -> Result<(u32, u64), Error>,\n) -> Result<(u32, u64), Error>",
                "nested commas belong to their types",
            ),
            (
                "pub fn r#match<'r#gen, const N: usize>(r#type: &'r#gen [u8; N]) -> &'r#gen [u8; N]",
                "pub fn r#match<'r#gen, const N: usize>(r#type: &'r#gen [u8; N]) -> &'r#gen [u8; N]",
                "raw identifiers, lifetimes, and const parameters",
            ),
        ] {
            assert_eq!(
                function_signature(input).as_deref(),
                Some(expected),
                "{message}"
            );
        }
    }
}
