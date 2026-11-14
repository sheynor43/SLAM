use std::sync::atomic::AtomicU64;
use std::time::Instant;

use super::*;

fn localizer() -> Arc<Localizer> {
    Arc::new(Localizer::builtin().unwrap())
}

fn theme_of(settings: &Settings) -> Theme {
    super::super::interface_theme(settings, settings)
}

fn russian() -> Settings {
    let mut settings = Settings::default();
    settings.general.language = Some("ru".to_owned());
    settings
}

#[test]
fn a_new_language_switches_the_localizer_and_asks_for_views() {
    let (link, _views, (mut requests, _out, _old)) = rings();
    let localizer = localizer();
    let mut applied =
        AppliedLanguage::new(Arc::clone(&localizer), None, Some(link), Theme::default());
    let settings = Settings::default();
    let rows = Rows::new(&settings);
    applied.apply(&settings, &rows, theme_of(&settings));
    // Nothing changed: no switch, no request.
    assert_eq!(localizer.generation(), 0);
    assert!(requests.pop().is_err());
    let settings = russian();
    applied.apply(&settings, &rows, theme_of(&settings));
    assert_eq!(localizer.format("settings-layout-language", None), "Язык");
    let rebuild = requests.pop().expect("views asked for");
    assert_eq!(rebuild.settings.general.language.as_deref(), Some("ru"));
    assert_eq!((rebuild.switch, applied.switches()), (1, 1));
    // Applied: the next ticks ask for nothing more.
    applied.apply(&settings, &rows, theme_of(&settings));
    assert!(requests.pop().is_err());
    assert_eq!(localizer.generation(), 1);
}

#[test]
fn another_colour_scheme_or_menu_scale_asks_for_views() {
    let (link, _views, (mut requests, _out, _old)) = rings();
    let localizer = localizer();
    let mut applied =
        AppliedLanguage::new(Arc::clone(&localizer), None, Some(link), Theme::default());
    let mut settings = Settings::default();
    let rows = Rows::new(&settings);
    settings.general.colour_scheme = slam_ui::theme::ColourScheme::Blue;
    assert!(
        !applied.apply(&settings, &rows, theme_of(&settings)),
        "the language did not switch"
    );
    let rebuild = requests.pop().expect("views asked for");
    assert_eq!(rebuild.theme.scheme, slam_ui::theme::ColourScheme::Blue);
    applied.apply(&settings, &rows, theme_of(&settings));
    assert!(requests.pop().is_err());
    settings.general.menu_scale = 1.25;
    applied.apply(&settings, &rows, theme_of(&settings));
    let rebuild = requests.pop().expect("views asked for");
    assert_eq!((rebuild.switch, applied.switches()), (2, 2));
    assert_eq!(localizer.generation(), 0);
}

#[test]
fn back_to_no_language_is_english() {
    let localizer = localizer();
    localizer.set_language("ru").unwrap();
    let mut applied = AppliedLanguage::new(
        Arc::clone(&localizer),
        Some("ru".to_owned()),
        None,
        Theme::default(),
    );
    let settings = Settings::default();
    applied.apply(&settings, &Rows::new(&settings), theme_of(&settings));
    assert_eq!(localizer.language().to_string(), FALLBACK_LANGUAGE);
}

#[test]
fn a_request_the_full_ring_refuses_goes_next_tick() {
    let (link, _views, (mut requests, _out, _old)) = rings();
    let mut applied = AppliedLanguage::new(localizer(), None, Some(link), Theme::default());
    let rows = Rows::new(&Settings::default());
    // Two switches fill the ring; the third waits.
    let mut settings = russian();
    for language in ["ru", "en", "ru"] {
        settings.general.language = Some(language.to_owned());
        applied.apply(&settings, &rows, theme_of(&settings));
    }
    assert!(applied.pending.is_some());
    assert!(requests.pop().is_ok());
    applied.apply(&settings, &rows, theme_of(&settings));
    assert!(applied.pending.is_none());
    let languages: Vec<_> = std::iter::from_fn(|| requests.pop().ok())
        .map(|r| r.settings.general.language)
        .collect();
    assert_eq!(
        languages,
        [Some("en".to_owned()), Some("ru".to_owned())],
        "in the order switched"
    );
}

#[test]
fn the_thread_lays_out_views_and_frees_the_ones_given_back() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let localizer = localizer();
    let (mut link, mut views, thread) = spawn(
        registry,
        Arc::clone(&localizer),
        Arc::new(AtomicU64::new(crate::play::pack_size(1920, 1080))),
    )
    .expect("thread starts");
    let settings = russian();
    localizer.set_language("ru").unwrap();
    let rebuild = Box::new(Rebuild {
        rows: Rows::new(&settings),
        theme: theme_of(&settings),
        settings,
        switch: 7,
    });
    assert!(link.request(rebuild).is_ok());
    let deadline = Instant::now() + Duration::from_secs(20);
    let received = loop {
        if let Some(views) = views.take() {
            break views;
        }
        assert!(Instant::now() < deadline, "no views");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(received.switch, 7, "numbered as the request");
    views.give_back(received);
    // The thread empties the ring of old views.
    let deadline = Instant::now() + Duration::from_secs(5);
    while views.old.slots() < RING * 2 {
        assert!(Instant::now() < deadline, "old views not freed");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(thread);
}

#[test]
fn take_keeps_the_newest_views_and_gives_the_older_back() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let settings = Settings::default();
    let rows = Rows::new(&settings);
    let localizer = localizer();
    let (_link, mut views, (_requests, mut out, mut old)) = rings();
    let make = || {
        Box::new(interface_views(
            &registry,
            Theme::default(),
            &localizer,
            &rows,
            &settings,
        ))
    };
    let (first, second) = (make(), make());
    let newest: *const InterfaceViews = &*second;
    assert!(out.push(first).is_ok());
    assert!(out.push(second).is_ok());
    let taken = views.take().expect("views arrived");
    assert!(std::ptr::eq(&*taken, newest));
    assert!(old.pop().is_ok(), "the older one went back");
    assert!(old.pop().is_err());
    assert!(views.take().is_none());
}

#[test]
fn an_unknown_language_asks_for_no_views() {
    let (link, _views, (mut requests, _out, _old)) = rings();
    let localizer = localizer();
    let mut applied =
        AppliedLanguage::new(Arc::clone(&localizer), None, Some(link), Theme::default());
    let mut settings = Settings::default();
    settings.general.language = Some("de".to_owned());
    applied.apply(&settings, &Rows::new(&settings), theme_of(&settings));
    assert!(requests.pop().is_err());
    assert_eq!(localizer.generation(), 0);
}

#[test]
fn dropdowns_that_came_before_the_new_screen_move_to_it() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let settings = Settings::default();
    let localizer = localizer();
    let mut rows = Rows::new(&settings);
    // Laid out before the skins of a new folder came.
    let mut new = interface_views(&registry, Theme::default(), &localizer, &rows, &settings);
    rows.swap_skins(&mut vec!["Alpha".to_owned(), "beta".to_owned()]);
    let mut old = interface_views(&registry, Theme::default(), &localizer, &rows, &settings);
    let row = crate::shell::rows::skin_row();
    let options = |screen: &mut SettingsView| screen.dropdown_mut(row).map(|v| v.options());
    assert_eq!(options(&mut old.settings), Some(3));
    assert_eq!(options(&mut new.settings), Some(1));
    carry_dropdowns(&mut old.settings, &mut new.settings, &[row]);
    assert_eq!(options(&mut new.settings), Some(3));
    assert_eq!(options(&mut old.settings), Some(1));
}

/// Views numbered by their switch.
#[derive(Debug, PartialEq)]
struct Views(u64);

impl Numbered for Views {
    fn switch(&self) -> u64 {
        self.0
    }
}

const MS: u64 = 1_000_000;
const PHASE: u64 = slam_ui::curtain::PHASE_NS;

#[test]
fn views_wait_behind_the_closed_curtain_which_opens_once_their_glyphs_are_made() {
    let mut swap = LanguageSwap::<Views>::default();
    assert_eq!(swap.receive(0, 1, None), None);
    assert!(swap.curtain().is_shown(), "closing");
    // The views come while it closes: held.
    assert_eq!(swap.receive(10 * MS, 1, Some(Views(1))), None);
    assert_eq!(swap.take(), None);
    swap.advance(PHASE, || 0);
    assert!(swap.curtain().is_closed());
    assert_eq!(swap.take(), Some(Views(1)));
    swap.advance(PHASE + MS, || 5);
    assert!(swap.curtain().is_closed(), "glyphs still pending");
    swap.advance(PHASE + 2 * MS, || 0);
    assert!(!swap.curtain().is_closed(), "opening");
    swap.advance(2 * PHASE + 2 * MS, || 0);
    assert!(!swap.curtain().is_shown());
}

#[test]
fn the_curtain_waits_for_the_last_switch_views_and_older_ones_go_back() {
    let mut swap = LanguageSwap::<Views>::default();
    swap.receive(0, 1, Some(Views(1)));
    // A second switch before the curtain closed: its views replace the first ones.
    assert_eq!(swap.receive(MS, 2, Some(Views(2))), Some(Views(1)));
    swap.advance(PHASE, || 0);
    assert_eq!(swap.take(), Some(Views(2)));
    swap.advance(PHASE + MS, || 0);
    assert!(!swap.curtain().is_closed());
    // A third while opening shows behind the curtain closing again.
    swap.receive(PHASE + 2 * MS, 3, None);
    swap.advance(2 * PHASE + MS, || 0);
    assert!(swap.curtain().is_shown(), "closing again");
    swap.advance(3 * PHASE + MS, || 0);
    assert!(swap.curtain().is_closed());
    swap.advance(3 * PHASE + 2 * MS, || 0);
    assert!(
        swap.curtain().is_closed(),
        "views of switch 3 not shown yet"
    );
    swap.receive(3 * PHASE + 3 * MS, 3, Some(Views(3)));
    assert_eq!(swap.take(), Some(Views(3)));
    swap.advance(3 * PHASE + 4 * MS, || 0);
    assert!(!swap.curtain().is_closed());
}

#[test]
fn views_never_made_open_the_curtain_after_the_hold_limit() {
    let mut swap = LanguageSwap::<Views>::default();
    swap.receive(0, 1, None);
    swap.advance(PHASE, || 0);
    swap.advance(PHASE + slam_ui::curtain::HOLD_LIMIT_NS, || 0);
    assert!(!swap.curtain().is_closed());
    swap.advance(2 * PHASE + slam_ui::curtain::HOLD_LIMIT_NS, || 0);
    // Late views swap in at once, with the curtain open.
    swap.receive(
        3 * PHASE + slam_ui::curtain::HOLD_LIMIT_NS,
        1,
        Some(Views(1)),
    );
    assert_eq!(swap.take(), Some(Views(1)));
}

#[test]
fn views_ahead_of_the_snapshot_still_wait_behind_the_curtain() {
    let mut swap = LanguageSwap::<Views>::default();
    // The views of switch 1 come before a snapshot numbers it.
    swap.receive(0, 0, Some(Views(1)));
    assert!(swap.curtain().is_shown(), "closing");
    assert_eq!(swap.take(), None);
    swap.receive(MS, 1, None);
    swap.advance(PHASE, || 0);
    assert_eq!(swap.take(), Some(Views(1)));
    swap.advance(PHASE + MS, || 0);
    assert!(!swap.curtain().is_closed(), "opening");
}
