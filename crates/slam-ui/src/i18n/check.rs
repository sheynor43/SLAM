//! Consistency check of languages against English.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fluent_bundle::FluentResource;
use fluent_syntax::ast::Entry;

use super::{I18nError, LanguageSource, parse_langid};

/// Message ids with their attribute names.
type Keys = BTreeMap<String, BTreeSet<String>>;

/// Differences of one language from English.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageReport {
    pub langid: String,
    /// `file: error` lines.
    pub parse_errors: Vec<String>,
    /// Messages English has and this language lacks.
    pub missing_keys: Vec<String>,
    /// Messages this language has and English lacks.
    pub extra_keys: Vec<String>,
    /// `message.attribute` present in English only.
    pub missing_attributes: Vec<String>,
    /// `message.attribute` present in this language only.
    pub extra_attributes: Vec<String>,
}

impl LanguageReport {
    pub fn is_clean(&self) -> bool {
        self.parse_errors.is_empty()
            && self.missing_keys.is_empty()
            && self.extra_keys.is_empty()
            && self.missing_attributes.is_empty()
            && self.extra_attributes.is_empty()
    }
}

/// Reports for every non-English language, plus English parse errors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub languages: Vec<LanguageReport>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.languages.iter().all(LanguageReport::is_clean)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for lang in &self.languages {
            if lang.is_clean() {
                writeln!(f, "{}: ok", lang.langid)?;
                continue;
            }
            writeln!(f, "{}:", lang.langid)?;
            let sections = [
                ("parse error", &lang.parse_errors),
                ("missing key", &lang.missing_keys),
                ("extra key", &lang.extra_keys),
                ("missing attribute", &lang.missing_attributes),
                ("extra attribute", &lang.extra_attributes),
            ];
            for (label, items) in sections {
                for item in items {
                    writeln!(f, "  {label}: {item}")?;
                }
            }
        }
        Ok(())
    }
}

fn collect(source: &LanguageSource<'_>, report: &mut LanguageReport) -> Keys {
    let mut keys = Keys::new();
    for (file, text) in source.files {
        let resource = match FluentResource::try_new((*text).to_owned()) {
            Ok(resource) => resource,
            Err((resource, errors)) => {
                for error in errors {
                    report.parse_errors.push(format!("{file}: {error}"));
                }
                resource
            }
        };
        for entry in resource.entries() {
            if let Entry::Message(message) = entry {
                let attributes = keys.entry(message.id.name.to_owned()).or_default();
                attributes.extend(message.attributes.iter().map(|a| a.id.name.to_owned()));
            }
        }
    }
    keys
}

/// Compares every language in `others` with `english`.
pub fn check(
    english: &LanguageSource<'_>,
    others: &[LanguageSource<'_>],
) -> Result<Report, I18nError> {
    parse_langid(english.langid)?;
    let mut report = Report::default();
    let mut english_report = LanguageReport {
        langid: english.langid.to_owned(),
        ..LanguageReport::default()
    };
    let reference = collect(english, &mut english_report);
    report.languages.push(english_report);
    for source in others {
        parse_langid(source.langid)?;
        let mut lang = LanguageReport {
            langid: source.langid.to_owned(),
            ..LanguageReport::default()
        };
        let keys = collect(source, &mut lang);
        for (id, attributes) in &reference {
            let Some(own) = keys.get(id) else {
                lang.missing_keys.push(id.clone());
                continue;
            };
            lang.missing_attributes
                .extend(attributes.difference(own).map(|a| format!("{id}.{a}")));
            lang.extra_attributes
                .extend(own.difference(attributes).map(|a| format!("{id}.{a}")));
        }
        lang.extra_keys.extend(
            keys.keys()
                .filter(|id| !reference.contains_key(*id))
                .cloned(),
        );
        report.languages.push(lang);
    }
    Ok(report)
}
