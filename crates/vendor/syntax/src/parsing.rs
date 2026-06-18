//! Lexing and bridging to parser, which does the actual parsing.

use crate::{
    TextRange,
    syntax_node::{SyntaxTree, SyntaxTreeBuilder},
};

pub(crate) fn parse_text(text: &str, edition: parser::Edition) -> std::sync::Arc<SyntaxTree> {
    let _p = tracing::info_span!("parse_text").entered();
    let lexed = parser::LexedStr::new(edition, text);
    let parser_input = lexed.to_input(edition);
    let parser_output = parser::TopEntryPoint::SourceFile.parse(&parser_input);
    let (tree, _eof) = build_tree(text, lexed, parser_output);
    tree
}

pub(crate) fn parse_text_at(
    text: &str,
    entry: parser::TopEntryPoint,
    edition: parser::Edition,
) -> std::sync::Arc<SyntaxTree> {
    let _p = tracing::info_span!("parse_text_at").entered();
    let lexed = parser::LexedStr::new(edition, text);
    let parser_input = lexed.to_input(edition);
    let parser_output = entry.parse(&parser_input);
    let (tree, _eof) = build_tree(text, lexed, parser_output);
    tree
}

pub(crate) fn build_tree(
    text: &str,
    lexed: parser::LexedStr<'_>,
    parser_output: parser::Output,
) -> (std::sync::Arc<SyntaxTree>, bool) {
    let _p = tracing::info_span!("build_tree").entered();
    let mut builder = SyntaxTreeBuilder::new(text);

    let is_eof = lexed.intersperse_trivia(&parser_output, &mut |step| match step {
        parser::StrStep::Token { kind, text } => builder.token(kind, text),
        parser::StrStep::Enter { kind } => builder.start_node(kind),
        parser::StrStep::Exit => builder.finish_node(),
        parser::StrStep::Error { msg, pos } => {
            builder.error(msg.to_owned(), pos.try_into().unwrap())
        }
    });

    for (i, err) in lexed.errors() {
        let text_range = lexed.text_range(i);
        let text_range = TextRange::new(
            text_range.start.try_into().unwrap(),
            text_range.end.try_into().unwrap(),
        );
        builder.error_with_range(err, text_range)
    }

    (builder.finish(), is_eof)
}
