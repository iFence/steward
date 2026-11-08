use crate::i18n::UiStrings;

use super::{row, RowAction, SearchContribution, SearchSource};

pub(crate) struct BuiltinSource<'a> {
    pub strings: &'a UiStrings,
}

impl SearchSource for BuiltinSource<'_> {
    fn contribute(&mut self, query: &str) -> SearchContribution {
        let mut contribution = SearchContribution::default();
        if query.is_empty() {
            return contribution;
        }
        if let Some(value) = steward_core_engine::try_evaluate(query) {
            let text = steward_core_engine::format_value(value);
            contribution.push(
                row("calc", &self.strings.calculator, &text, None),
                RowAction::CopyText(text),
            );
        } else if steward_core_engine::try_openable(query).is_some() {
            contribution.push(
                row("url", &self.strings.open_url, query, None),
                RowAction::OpenUrl(query.to_owned()),
            );
        }
        contribution
    }
}
