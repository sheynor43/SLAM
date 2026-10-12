use fluent_bundle::FluentArgs;

use super::*;

const EN: &[(&str, &str)] = &[(
    "a.ftl",
    "hello = Hello\nonly-en = Only English\nwith-attr = Value\n    .tip = Tip\ngreet = Hi, { $name }!\ncount = { $n } items\n",
)];
const RU: &[(&str, &str)] = &[("a.ftl", "hello = Привет\ngreet = Привет, { $name }!\n")];

fn sources<'a>(
    en: &'a [(&'a str, &'a str)],
    ru: &'a [(&'a str, &'a str)],
) -> (LanguageSource<'a>, LanguageSource<'a>) {
    (
        LanguageSource {
            langid: "en",
            files: en,
        },
        LanguageSource {
            langid: "ru",
            files: ru,
        },
    )
}

fn localizer() -> Localizer {
    let (en, ru) = sources(EN, RU);
    Localizer::new(&en, &[ru]).unwrap()
}

#[test]
fn substitutes_string_and_number_arguments() {
    let l = localizer();
    let mut args = FluentArgs::new();
    args.set("name", "osu");
    assert_eq!(l.format("greet", Some(&args)), "Hi, osu!");
    let mut args = FluentArgs::new();
    args.set("n", 1234);
    assert_eq!(l.format("count", Some(&args)), "1234 items");
}

#[test]
fn no_isolation_marks() {
    let l = localizer();
    let mut args = FluentArgs::new();
    args.set("name", "x");
    let text = l.format("greet", Some(&args));
    assert!(!text.contains('\u{2068}') && !text.contains('\u{2069}'));
}

#[test]
fn falls_back_to_english_then_key() {
    let mut l = localizer();
    l.set_language("ru").unwrap();
    assert_eq!(l.format("hello", None), "Привет");
    assert_eq!(l.format("only-en", None), "Only English");
    assert_eq!(l.format("no-such-key", None), "no-such-key");
    assert_eq!(l.format("with-attr.tip", None), "Tip");
    assert_eq!(l.format("with-attr.nope", None), "with-attr.nope");
}

#[test]
fn switching_changes_output_and_bumps_generation() {
    let mut l = localizer();
    assert_eq!(l.generation(), 0);
    assert_eq!(l.format("hello", None), "Hello");
    l.set_language("ru").unwrap();
    assert_eq!(l.generation(), 1);
    assert_eq!(l.format("hello", None), "Привет");
    l.set_language("en").unwrap();
    assert_eq!(l.generation(), 2);
    assert_eq!(l.format("hello", None), "Hello");
    assert_eq!(l.language().to_string(), "en");
}

#[test]
fn unknown_language_leaves_state_unchanged() {
    let mut l = localizer();
    l.set_language("ru").unwrap();
    let generation = l.generation();
    assert_eq!(
        l.set_language("de"),
        Err(I18nError::UnknownLanguage("de".into()))
    );
    assert!(matches!(
        l.set_language("not a tag!"),
        Err(I18nError::InvalidLangId(_))
    ));
    assert_eq!(l.generation(), generation);
    assert_eq!(l.language().to_string(), "ru");
}

#[test]
fn parse_errors_are_reported() {
    let bad: &[(&str, &str)] = &[("bad.ftl", "key = { $x\n")];
    let (en, _) = sources(bad, RU);
    assert!(matches!(
        Localizer::new(&en, &[]),
        Err(I18nError::Parse { .. })
    ));
}

fn count(l: &Localizer, n: i64) -> String {
    let mut args = FluentArgs::new();
    args.set("count", n);
    l.format("song-select-beatmap-count", Some(&args))
        .into_owned()
}

#[test]
fn builtin_plurals() {
    let mut l = Localizer::builtin().unwrap();
    assert_eq!(count(&l, 1), "1 beatmap");
    assert_eq!(count(&l, 2), "2 beatmaps");
    l.set_language("ru").unwrap();
    assert_eq!(count(&l, 1), "1 карта");
    assert_eq!(count(&l, 2), "2 карты");
    assert_eq!(count(&l, 5), "5 карт");
    assert_eq!(count(&l, 21), "21 карта");
    assert_eq!(count(&l, 11), "11 карт");
    assert_eq!(l.format("settings-language-name", None), "Русский");
}

#[test]
fn check_detects_differences() {
    let en: &[(&str, &str)] = &[("a.ftl", "a = A\n    .t = T\nb = B\n    .u = U\nc = C\n")];
    let ru: &[(&str, &str)] = &[("a.ftl", "a = A\nb = B\n    .u = U\n    .v = V\nz = Z\n")];
    let (en, ru) = sources(en, ru);
    let report = check(&en, &[ru]).unwrap();
    assert!(!report.is_clean());
    let lang = &report.languages[1];
    assert_eq!(lang.missing_keys, ["c"]);
    assert_eq!(lang.extra_keys, ["z"]);
    assert_eq!(lang.missing_attributes, ["a.t"]);
    assert_eq!(lang.extra_attributes, ["b.v"]);
    let text = report.to_string();
    assert!(text.contains("missing key: c") && text.contains("extra attribute: b.v"));
}

#[test]
fn check_detects_parse_errors() {
    let en: &[(&str, &str)] = &[("a.ftl", "a = A\n")];
    let ru: &[(&str, &str)] = &[("a.ftl", "a = A\nb = { $x\n")];
    let (en, ru) = sources(en, ru);
    let report = check(&en, &[ru]).unwrap();
    assert!(!report.languages[1].parse_errors.is_empty());
    assert!(!report.is_clean());
}

#[test]
fn builtin_languages_are_clean() {
    let report = check(&BUILTIN_LANGUAGES[0], &BUILTIN_LANGUAGES[1..]).unwrap();
    assert!(report.is_clean(), "{report}");
    Localizer::builtin().unwrap();
}
