use serde::Serialize;

use super::{
    block::ReportBlock, default_title, fields::ReportFieldsBuilder, table::ReportTableBuilder,
};

/// One report section, such as build stats, profile data, or memory data.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportSection {
    /// Section key. HTML uses it for anchors and tabs.
    pub(crate) key: String,
    /// Group used by renderers with tabs or navigation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<ReportSectionGroup>,
    /// Section heading. `None` makes the section act as a container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    /// Description text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// Blocks in report order.
    pub(crate) blocks: Vec<ReportBlock>,
}

impl ReportSection {
    pub(crate) fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            group: None,
            title: Some(title.into()),
            description: None,
            blocks: Vec::new(),
        }
    }

    pub(crate) fn push_block(&mut self, block: ReportBlock) {
        self.blocks.push(block);
    }
}

/// Group for related sections.
///
/// Text output can ignore this. HTML can turn it into tabs. JSON keeps it visible for scripts that
/// want the same shape.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReportSectionGroup {
    /// Group key.
    pub(crate) key: String,
    /// Group title.
    pub(crate) title: String,
}

impl ReportSectionGroup {
    fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
        }
    }
}

/// Builder for one report section.
pub(crate) struct ReportSectionBuilder {
    section: ReportSection,
}

impl ReportSectionBuilder {
    pub(super) fn new(key: impl Into<String>) -> Self {
        let key = key.into();
        Self {
            section: ReportSection::new(&key, default_title(&key)),
        }
    }

    pub(crate) fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.section.title = Some(title.into());
        self
    }

    pub(crate) fn untitled(&mut self) -> &mut Self {
        self.section.title = None;
        self
    }

    pub(crate) fn group(&mut self, key: impl Into<String>, title: impl Into<String>) -> &mut Self {
        self.section.group = Some(ReportSectionGroup::new(key, title));
        self
    }

    pub(crate) fn fields<R>(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportFieldsBuilder) -> R,
    ) -> &mut Self {
        let mut fields = ReportFieldsBuilder::new(key);
        configure(&mut fields);
        self.section.push_block(fields.build());
        self
    }

    pub(crate) fn table<R>(
        &mut self,
        key: impl Into<String>,
        configure: impl FnOnce(&mut ReportTableBuilder) -> R,
    ) -> &mut Self {
        let mut table = ReportTableBuilder::new(key);
        configure(&mut table);
        self.section.push_block(table.build());
        self
    }

    pub(super) fn build(self) -> ReportSection {
        self.section
    }
}
