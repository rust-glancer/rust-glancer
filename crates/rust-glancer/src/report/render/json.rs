use crate::report::ReportDocument;

pub(crate) struct RichJsonRenderer;

impl RichJsonRenderer {
    pub(crate) fn render(&self, document: &ReportDocument) -> serde_json::Result<String> {
        serde_json::to_string_pretty(document)
    }
}
