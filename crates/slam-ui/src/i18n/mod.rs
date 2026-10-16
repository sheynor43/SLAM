//! Fluent localization (ADR-0011): the localizer, built-in languages and the
//! consistency check. Nothing here touches the disk; built-in locales are embedded.

mod check;

use std::borrow::Cow;
use std::fmt;

use fluent_bundle::FluentResource;
use fluent_bundle::concurrent::FluentBundle;
use unic_langid::LanguageIdentifier;

pub use check::{LanguageReport, Report, check};
pub use fluent_bundle::FluentArgs;

/// Language of the fallback bundle, always present.
pub const FALLBACK_LANGUAGE: &str = "en";

/// A language as a list of `(file name, .ftl source)` pairs.
#[derive(Debug, Clone, Copy)]
pub struct LanguageSource<'a> {
    pub langid: &'a str,
    pub files: &'a [(&'a str, &'a str)],
}

macro_rules! builtin_files {
    ($lang:literal: $($file:literal),+ $(,)?) => {
        &[$(($file, include_str!(concat!("../../../../locales/", $lang, "/", $file)))),+]
    };
}

/// Languages shipped inside the binary. English comes first.
pub const BUILTIN_LANGUAGES: &[LanguageSource<'static>] = &[
    LanguageSource {
        langid: "en",
        files: builtin_files!("en": "common.ftl"),
    },
    LanguageSource {
        langid: "ru",
        files: builtin_files!("ru": "common.ftl"),
    },
];

/// Problems while building bundles from sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum I18nError {
    /// The language tag is not a valid BCP 47 identifier.
    InvalidLangId(String),
    /// A `.ftl` file has syntax errors.
    Parse {
        langid: String,
        file: String,
        errors: Vec<String>,
    },
    /// Two files of one language define the same message or term.
    Overriding {
        langid: String,
        file: String,
        errors: Vec<String>,
    },
    /// The requested language is not available.
    UnknownLanguage(String),
}

impl fmt::Display for I18nError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLangId(id) => write!(f, "invalid language identifier `{id}`"),
            Self::Parse {
                langid,
                file,
                errors,
            } => write!(f, "{langid}/{file}: {}", errors.join("; ")),
            Self::Overriding {
                langid,
                file,
                errors,
            } => write!(f, "{langid}/{file}: {}", errors.join("; ")),
            Self::UnknownLanguage(id) => write!(f, "unknown language `{id}`"),
        }
    }
}

impl std::error::Error for I18nError {}

type Bundle = FluentBundle<FluentResource>;

fn parse_langid(id: &str) -> Result<LanguageIdentifier, I18nError> {
    id.parse()
        .map_err(|_| I18nError::InvalidLangId(id.to_owned()))
}

fn build_bundle(source: &LanguageSource<'_>) -> Result<(LanguageIdentifier, Bundle), I18nError> {
    let langid = parse_langid(source.langid)?;
    let mut bundle = FluentBundle::new_concurrent(vec![langid.clone()]);
    // Fluent wraps placeable values in FSI/PDI isolation marks; our text renderer has
    // no glyphs for them and would draw tofu, so they are off.
    bundle.set_use_isolating(false);
    for (file, text) in source.files {
        let resource = FluentResource::try_new((*text).to_owned()).map_err(|(_, errors)| {
            I18nError::Parse {
                langid: source.langid.to_owned(),
                file: (*file).to_owned(),
                errors: errors.iter().map(ToString::to_string).collect(),
            }
        })?;
        bundle
            .add_resource(resource)
            .map_err(|errors| I18nError::Overriding {
                langid: source.langid.to_owned(),
                file: (*file).to_owned(),
                errors: errors.iter().map(ToString::to_string).collect(),
            })?;
    }
    Ok((langid, bundle))
}

/// Looks up UI strings: active language, then English, then the key itself.
pub struct Localizer {
    fallback: Bundle,
    /// Non-English languages that can be activated.
    languages: Vec<(LanguageIdentifier, Bundle)>,
    /// Index into `languages`; `None` means English only.
    active: Option<usize>,
    english: LanguageIdentifier,
    generation: u64,
}

impl Localizer {
    /// Builds a localizer with English active. `others` are the switchable languages;
    /// an entry equal to English is ignored.
    pub fn new(
        english: &LanguageSource<'_>,
        others: &[LanguageSource<'_>],
    ) -> Result<Self, I18nError> {
        let (english_id, fallback) = build_bundle(english)?;
        let mut languages = Vec::with_capacity(others.len());
        for source in others {
            let (id, bundle) = build_bundle(source)?;
            if id != english_id {
                languages.push((id, bundle));
            }
        }
        Ok(Self {
            fallback,
            languages,
            active: None,
            english: english_id,
            generation: 0,
        })
    }

    /// Localizer over the built-in languages, English active.
    pub fn builtin() -> Result<Self, I18nError> {
        Self::new(&BUILTIN_LANGUAGES[0], &BUILTIN_LANGUAGES[1..])
    }

    /// Language identifiers that `set_language` accepts, English included.
    pub fn available_languages(&self) -> impl Iterator<Item = &LanguageIdentifier> {
        std::iter::once(&self.english).chain(self.languages.iter().map(|(id, _)| id))
    }

    /// The active language.
    pub fn language(&self) -> &LanguageIdentifier {
        match self.active {
            Some(i) => &self.languages[i].0,
            None => &self.english,
        }
    }

    /// Switches the active language. On error nothing changes.
    pub fn set_language(&mut self, langid: &str) -> Result<(), I18nError> {
        let id = parse_langid(langid)?;
        let active = if id == self.english {
            None
        } else {
            Some(
                self.languages
                    .iter()
                    .position(|(l, _)| *l == id)
                    .ok_or_else(|| I18nError::UnknownLanguage(langid.to_owned()))?,
            )
        };
        self.active = active;
        self.generation += 1;
        Ok(())
    }

    /// Bumped on every successful language switch; cached strings from an older
    /// generation must be formatted again.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Formats `key` (`message` or `message.attribute`). Falls back to English, then
    /// to the key itself.
    pub fn format<'a>(&'a self, key: &'a str, args: Option<&'a FluentArgs<'_>>) -> Cow<'a, str> {
        if let Some(i) = self.active
            && let Some(text) = Self::format_in(&self.languages[i].1, key, args)
        {
            return text;
        }
        if let Some(text) = Self::format_in(&self.fallback, key, args) {
            if self.active.is_some() {
                tracing::debug!(key, "missing translation, using English");
            }
            return text;
        }
        tracing::warn!(key, "unknown localization key");
        Cow::Borrowed(key)
    }

    fn format_in<'a>(
        bundle: &'a Bundle,
        key: &str,
        args: Option<&FluentArgs<'_>>,
    ) -> Option<Cow<'a, str>> {
        let (id, attribute) = match key.split_once('.') {
            Some((id, attribute)) => (id, Some(attribute)),
            None => (key, None),
        };
        let message = bundle.get_message(id)?;
        let pattern = match attribute {
            Some(name) => message.get_attribute(name)?.value(),
            None => message.value()?,
        };
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, args, &mut errors);
        if !errors.is_empty() {
            tracing::warn!(key, ?errors, "localization formatting errors");
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests;
