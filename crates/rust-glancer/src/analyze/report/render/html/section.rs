use crate::analyze::report::{ReportDocument, ReportSection};

use super::html_id;

/// Section groups become top-level tabs in the HTML report.
pub(super) struct SectionGroup<'a> {
    pub(super) key: String,
    pub(super) title: String,
    pub(super) sections: Vec<&'a ReportSection>,
}

impl<'a> SectionGroup<'a> {
    pub(super) fn capture(document: &'a ReportDocument) -> Vec<Self> {
        let mut groups = Vec::<Self>::new();
        for section in &document.sections {
            let (key, title) = section
                .group
                .as_ref()
                .map(|group| (group.key.as_str(), group.title.as_str()))
                .unwrap_or_else(|| (section.key.as_str(), Self::section_title(section)));

            if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
                group.sections.push(section);
            } else {
                groups.push(Self {
                    key: key.to_string(),
                    title: title.to_string(),
                    sections: vec![section],
                });
            }
        }

        groups
    }

    pub(super) fn panel_id(&self) -> String {
        html_id("group", &self.key)
    }

    pub(super) fn section_title(section: &ReportSection) -> &str {
        section.title.as_deref().unwrap_or(section.key.as_str())
    }
}
