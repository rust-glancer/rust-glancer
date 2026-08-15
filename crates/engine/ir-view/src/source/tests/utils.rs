use rg_ir_model::{CrateRef, PackageSlot, identity::DeclarationRef};

use crate::source::{
    IndexedSourceFact, IndexedSourceRole, IndexedSourceSurface, SourceOccurrenceView,
};
use crate::testonly::ViewFixture;

pub(super) fn check_source_occurrences(ident: &str, fixture: &str, expected: &str) {
    let fixture = ViewFixture::build(fixture);
    let package = fixture
        .parse_db()
        .packages()
        .first()
        .expect("fixture should contain one package");
    let target = package
        .targets()
        .first()
        .expect("fixture package should contain one target");
    let file_id = package
        .parsed_files()
        .find(|file| file.path().ends_with("src/lib.rs"))
        .expect("fixture should contain src/lib.rs")
        .file_id();
    let parsed_file = package
        .parsed_file(file_id)
        .expect("fixture source file should be parsed");
    let crate_ref = CrateRef {
        package: PackageSlot(0),
        crate_id: rg_ir_model::CrateId(target.id.0),
    };
    let view_db = fixture.view_db();

    let mut occurrences = Vec::new();
    for occurrence in SourceOccurrenceView::new(&view_db)
        .occurrences_in_crate(crate_ref, Some(file_id))
        .expect("fixture source occurrences should scan")
    {
        let (fact, _, occurrence_file, span, role, surface) = occurrence.into_parts();
        if occurrence_file != file_id {
            continue;
        }
        let Some(text) = parsed_file
            .text_for_span(span)
            .expect("source occurrence text should load")
        else {
            continue;
        };
        if text != ident {
            continue;
        }

        occurrences.push(format!(
            "{} @ {}",
            render_occurrence(fact, role, surface),
            render_span(
                span,
                parsed_file
                    .line_index()
                    .expect("fixture line index should load")
            )
        ));
    }
    occurrences.sort();

    let actual = if occurrences.is_empty() {
        "<none>".to_string()
    } else {
        occurrences.join("\n")
    };
    let expected = expected
        .trim()
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(actual, expected);
}

fn render_span(span: rg_parse::Span, line_index: &rg_parse::LineIndex) -> String {
    let line_column = span.line_column(line_index);
    format!(
        "{}:{}-{}:{}",
        line_column.start.line + 1,
        line_column.start.column + 1,
        line_column.end.line + 1,
        line_column.end.column + 1
    )
}

fn render_occurrence(
    fact: IndexedSourceFact,
    role: IndexedSourceRole,
    surface: IndexedSourceSurface,
) -> String {
    match surface {
        IndexedSourceSurface::RecordExprShorthandFieldKey { .. }
        | IndexedSourceSurface::RecordPatShorthandFieldKey { .. } => {
            let IndexedSourceFact::RecordField { owner, key, .. } = fact else {
                panic!("record field surface should carry a record field fact");
            };
            format!(
                "record_shorthand_field {owner}::{}",
                key.declaration_label()
            )
        }
        IndexedSourceSurface::RecordExprShorthandValue { key, .. } => {
            format!("record_shorthand_value {}", key.declaration_label())
        }
        IndexedSourceSurface::RecordPatShorthandBinding { key, .. } => {
            format!("record_shorthand_binding {}", key.declaration_label())
        }
        IndexedSourceSurface::Plain | IndexedSourceSurface::RecordFieldKeyExplicit => match fact {
            IndexedSourceFact::Declaration(declaration) => match declaration {
                DeclarationRef::BodyBinding(_) => "binding".to_string(),
                DeclarationRef::Field(_) => "field".to_string(),
                DeclarationRef::EnumVariant(_) => "enum_variant".to_string(),
                DeclarationRef::Item(_) => "item".to_string(),
                DeclarationRef::LocalDef(_) if role == IndexedSourceRole::Reference => {
                    "local_def_reference".to_string()
                }
                DeclarationRef::LocalDef(_) => "local_def".to_string(),
                DeclarationRef::Module(_) => "module".to_string(),
            },
            IndexedSourceFact::FunctionBody(_) => "body".to_string(),
            IndexedSourceFact::Expr(_) => "expr".to_string(),
            IndexedSourceFact::TypePath(type_path) => {
                format!("type_path {}", type_path.path())
            }
            IndexedSourceFact::ValuePath { path, .. } => format!("value_path {path}"),
            IndexedSourceFact::RecordField { owner, key, .. } => {
                format!("record_field {owner}::{}", key.declaration_label())
            }
            IndexedSourceFact::UsePath { path, .. } => format!("use_path {path}"),
        },
    }
}
