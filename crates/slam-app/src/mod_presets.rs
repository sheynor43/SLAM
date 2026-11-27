//! Pinned mod presets of the song select's footer (ADR-0175): a preset is a set of mods
//! with their settings, kept in the settings file as one string, mod acronyms separated by
//! spaces, a mod's non-default settings after its acronym in parentheses as `key=value`
//! pairs, like lazer's score blocks name them: `HD DT(speed_change=1.3,adjust_pitch=true)`.
//! Text that does not name known mods is dropped when read.

use slam_formats::osr::SettingValue;
use slam_osu::mods::{GameplayMod as _, Mod};

/// Most presets (the footer's room).
pub const MAX: usize = slam_ui::song_select::MAX_PRESETS;

/// The text of `mods`, in their order. Settings of a kind the text cannot hold (nulls, raw
/// JSON, strings with its separators) are left out.
pub fn encode<'a>(mods: impl IntoIterator<Item = &'a Mod>) -> String {
    let mut out = String::new();
    for m in mods {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(m.acronym());
        let settings: Vec<String> = m
            .settings()
            .into_iter()
            .filter_map(|(key, value)| {
                let value = match value {
                    SettingValue::Bool(b) => b.to_string(),
                    SettingValue::Int(i) => i.to_string(),
                    SettingValue::Float(f) if f.is_finite() => f.to_string(),
                    _ => return None,
                };
                Some(format!("{key}={value}"))
            })
            .collect();
        if !settings.is_empty() {
            out.push('(');
            out.push_str(&settings.join(","));
            out.push(')');
        }
    }
    out
}

fn value_of(text: &str) -> SettingValue {
    match text {
        "true" => SettingValue::Bool(true),
        "false" => SettingValue::Bool(false),
        _ => text
            .parse::<i64>()
            .map(SettingValue::Int)
            .or_else(|_| text.parse::<f64>().map(SettingValue::Float))
            .unwrap_or_else(|_| SettingValue::String(text.to_owned())),
    }
}

/// The mods `text` names. `None` for no mods, a mod this build does not know, or one named
/// twice.
pub fn parse(text: &str) -> Option<Vec<Mod>> {
    let mut mods: Vec<Mod> = Vec::new();
    for token in text.split_whitespace() {
        let (acronym, settings) = match token.split_once('(') {
            Some((acronym, rest)) => (acronym, rest.strip_suffix(')')?),
            None => (token, ""),
        };
        let mut m = Mod::from_acronym(acronym);
        if !m.is_supported() || mods.iter().any(|o| o.acronym() == m.acronym()) {
            return None;
        }
        for pair in settings.split(',').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=')?;
            m.set_setting(key, &value_of(value));
        }
        mods.push(m);
    }
    (!mods.is_empty()).then_some(mods)
}

/// `text` as it would be written back (known mods only, settings in a fixed order), or
/// `None` when it names no usable preset.
pub fn normalise(text: &str) -> Option<String> {
    parse(text).map(|mods| encode(&mods))
}

/// The acronyms of `text`, separated by spaces: what a footer button shows.
pub fn label(text: &str) -> String {
    text.split_whitespace()
        .map(|token| token.split('(').next().unwrap_or(token))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether two presets are the same mods with the same settings, whatever their order.
pub fn same(a: &str, b: &str) -> bool {
    let key = |text: &str| {
        let mut parts: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
        parts.sort();
        parts
    };
    key(a) == key(b)
}

/// The presets of a settings file: unusable ones, repeats and those past [`MAX`] dropped.
pub fn sanitise(read: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in read {
        if out.len() >= MAX {
            break;
        }
        if let Some(text) = normalise(&text)
            && !out.iter().any(|o| same(o, &text))
        {
            out.push(text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_round_trips_with_its_settings() {
        let text = "HD DT(speed_change=1.3,adjust_pitch=true)";
        let mods = parse(text).expect("known mods");
        assert_eq!(mods.len(), 2);
        assert_eq!(encode(&mods), text);
        assert_eq!(label(text), "HD DT");
    }

    #[test]
    fn a_default_mod_has_no_parentheses() {
        assert_eq!(normalise("  HD   EZ ").as_deref(), Some("HD EZ"));
    }

    #[test]
    fn unknown_empty_and_repeated_mods_are_dropped() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("HD ZZ"), None);
        assert_eq!(parse("HD HD"), None);
        assert_eq!(parse("DT(speed_change=1.3"), None);
    }

    #[test]
    fn presets_compare_regardless_of_order() {
        assert!(same("HD DT", "DT HD"));
        assert!(!same("HD DT", "HD"));
        assert!(!same("DT(speed_change=1.3)", "DT(speed_change=1.4)"));
    }

    #[test]
    fn reading_drops_bad_repeated_and_surplus_presets() {
        let read = [
            "HD".to_owned(),
            "nonsense".to_owned(),
            "HD".to_owned(),
            "DT HD".to_owned(),
            "HD DT".to_owned(),
        ]
        .into_iter()
        .chain((0..10).map(|_| "EZ".to_owned()));
        assert_eq!(sanitise(read), ["HD", "DT HD", "EZ"]);
        let many = ["HD", "HR", "DT", "EZ", "PF", "HT", "NC", "SD", "DC"]
            .into_iter()
            .map(str::to_owned);
        assert_eq!(sanitise(many).len(), MAX);
    }
}
