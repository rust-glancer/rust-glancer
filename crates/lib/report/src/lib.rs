//! Build and render the reports printed by the analysis and LSP comparison commands.

mod document;
mod render;

pub use self::{
    document::{
        ReportAlign, ReportBlock, ReportColumn, ReportDocument, ReportDocumentBuilder, ReportField,
        ReportFieldsBuilder, ReportRow, ReportRowBuilder, ReportSection, ReportSectionBuilder,
        ReportTableBuilder, ReportUnit, ReportValue,
    },
    render::{HtmlRenderer, RichJsonRenderer, TextRenderer},
};
