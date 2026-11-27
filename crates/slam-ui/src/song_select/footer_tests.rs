//! The footer bar of the song select and what stands above it (ADR-0175, ADR-0177).

use super::SongSelectInput as I;
use super::*;

const MOD_COLUMNS: ModColumns = [&["EZ", "HT"], &["HR", "DT"], &["RX"], &[], &[]];

fn layout() -> SongSelectLayout {
    SongSelectLayout::new(1280.0, 720.0, &Theme::default()).with_mod_columns(&MOD_COLUMNS)
}

fn screen() -> SongSelect {
    let mut s = SongSelect::new();
    let list = Arc::new(SongList {
        sets: vec![SetRow {
            key: 1,
            artist: "artist".to_owned(),
            title: "title".to_owned(),
            creator: "mapper".to_owned(),
            difficulties: vec![DifficultyRow::new(SearchSource::default())],
        }],
    });
    let filter = SongFilter::for_list(&list);
    s.set_list(list, filter);
    *s.mods_mut() = ModSelect::new(MOD_COLUMNS);
    s
}

fn presets(labels: &[&str]) -> Arc<[Arc<str>]> {
    labels.iter().map(|l| Arc::from(*l)).collect()
}

fn centre_of(s: &SongSelect, button: FooterButton) -> (f32, f32) {
    let [x, y] = s
        .footer_buttons(&layout())
        .rect(button)
        .unwrap_or_else(|| panic!("{button:?} is not shown"))
        .centre();
    (x, y)
}

/// Clicks `button` with the primary button.
fn click(s: &mut SongSelect, button: FooterButton) -> Option<SongSelectAction> {
    let (x, y) = centre_of(s, button);
    let l = layout();
    s.input(I::PointerMove { x, y }, &l);
    assert_eq!(s.input(I::PointerPress { x, y }, &l), None);
    s.input(I::PointerRelease, &l)
}

fn right_click(s: &mut SongSelect, button: FooterButton) -> Option<SongSelectAction> {
    let (x, y) = centre_of(s, button);
    s.input(I::SecondaryPress { x, y }, &layout())
}

fn select_a_mod(s: &mut SongSelect) {
    s.mods_mut().set_active(0, 0, true);
}

#[test]
fn back_and_mods_are_always_there_and_the_rest_waits_for_mods() {
    let mut s = screen();
    assert_eq!(s.footer_state(), FooterState::default());
    let buttons = s.footer_buttons(&layout());
    assert!(buttons.pin.is_none() && buttons.reset.is_none());
    assert_eq!(buttons.preset_count, 0);
    select_a_mod(&mut s);
    let state = s.footer_state();
    assert!(state.pin && state.reset);
}

#[test]
fn reset_shows_only_with_mods_selected_and_takes_its_place_away() {
    let mut s = screen();
    s.set_mod_presets(presets(&["HD"]));
    let l = layout();
    let without = s.footer_buttons(&l);
    assert!(without.reset.is_none());
    select_a_mod(&mut s);
    let with = s.footer_buttons(&l);
    let reset = with.reset.expect("reset shown");
    assert_eq!(
        click(&mut s, FooterButton::Reset),
        Some(SongSelectAction::DeselectMods)
    );
    // The application deselected the mods: the button is gone and nothing answers there.
    s.mods_mut().set_active(0, 0, false);
    assert!(s.footer_buttons(&l).reset.is_none());
    let [x, y] = reset.centre();
    assert!(!s.snapshot().footer.reset);
    assert_eq!(s.footer_buttons(&l).at(x, y), None);
}

#[test]
fn the_pin_shows_for_unpinned_mods_while_there_is_room() {
    let mut s = screen();
    select_a_mod(&mut s);
    assert!(s.footer_state().pin);
    s.set_current_preset(Some(0));
    assert!(!s.footer_state().pin, "already a preset");
    s.set_current_preset(None);
    let full: Vec<&str> = vec!["HD"; MAX_PRESETS];
    s.set_mod_presets(presets(&full));
    assert!(!s.footer_state().pin, "no room for another");
    assert_eq!(s.footer_state().presets, MAX_PRESETS);
    s.set_mod_presets(presets(&full[..MAX_PRESETS - 1]));
    assert!(s.footer_state().pin);
}

#[test]
fn the_pin_asks_to_pin_the_current_mods() {
    let mut s = screen();
    select_a_mod(&mut s);
    assert_eq!(
        click(&mut s, FooterButton::Pin),
        Some(SongSelectAction::PinPreset)
    );
}

#[test]
fn a_click_applies_a_preset_and_a_right_click_removes_it() {
    let mut s = screen();
    s.set_mod_presets(presets(&["HD DT", "EZ", "HR"]));
    for i in 0..3 {
        assert_eq!(
            click(&mut s, FooterButton::Preset(i)),
            Some(SongSelectAction::ApplyPreset(i))
        );
        assert_eq!(
            right_click(&mut s, FooterButton::Preset(i)),
            Some(SongSelectAction::RemovePreset(i))
        );
    }
    // The primary click does not remove and the secondary does not apply.
    assert_eq!(s.snapshot().presets.len(), 3);
}

#[test]
fn back_leaves_and_mods_toggles_the_panel() {
    let mut s = screen();
    assert_eq!(
        click(&mut s, FooterButton::Back),
        Some(SongSelectAction::Back)
    );
    assert_eq!(click(&mut s, FooterButton::Mods), None);
    assert!(s.snapshot().mods.open);
    // With the panel open Back closes it first, as Esc does.
    assert_eq!(click(&mut s, FooterButton::Back), None);
    assert!(!s.snapshot().mods.open);
    click(&mut s, FooterButton::Mods);
    click(&mut s, FooterButton::Mods);
    assert!(!s.snapshot().mods.open);
}

#[test]
fn the_footer_works_while_the_panel_is_open() {
    let mut s = screen();
    s.set_mod_presets(presets(&["HD"]));
    click(&mut s, FooterButton::Mods);
    assert_eq!(
        click(&mut s, FooterButton::Preset(0)),
        Some(SongSelectAction::ApplyPreset(0))
    );
    assert!(s.snapshot().mods.open);
}

#[test]
fn a_press_released_elsewhere_does_nothing() {
    let mut s = screen();
    let l = layout();
    let (x, y) = centre_of(&s, FooterButton::Back);
    s.input(I::PointerPress { x, y }, &l);
    s.input(I::PointerMove { x: x + 600.0, y }, &l);
    assert_eq!(s.input(I::PointerRelease, &l), None);
    assert_eq!(s.snapshot().footer_pressed, None);
}

#[test]
fn hover_follows_the_pointer() {
    let mut s = screen();
    let l = layout();
    let (x, y) = centre_of(&s, FooterButton::Mods);
    s.input(I::PointerMove { x, y }, &l);
    assert_eq!(s.snapshot().footer_hover, Some(FooterButton::Mods));
    s.input(I::PointerMove { x: 640.0, y: 300.0 }, &l);
    assert_eq!(s.snapshot().footer_hover, None);
}

#[test]
fn only_back_works_while_a_difficulty_loads() {
    let mut s = screen();
    s.set_mod_presets(presets(&["HD"]));
    select_a_mod(&mut s);
    s.set_status(SongSelectStatus::Loading);
    assert_eq!(click(&mut s, FooterButton::Preset(0)), None);
    assert_eq!(click(&mut s, FooterButton::Reset), None);
    assert_eq!(click(&mut s, FooterButton::Mods), None);
    assert!(!s.snapshot().mods.open);
    assert_eq!(
        click(&mut s, FooterButton::Back),
        Some(SongSelectAction::Back)
    );
}

#[test]
fn a_press_on_the_bar_does_not_reach_the_list() {
    let mut s = screen();
    let l = layout();
    // Between the buttons, on the bar, under where list rows run.
    let x = l.footer.bar.width - 5.0;
    let y = l.footer.bar.y + 3.0;
    assert_eq!(s.input(I::PointerPress { x, y }, &l), None);
    assert_eq!(s.input(I::PointerRelease, &l), None);
}

#[test]
fn the_content_stands_above_the_footer_at_common_sizes() {
    for (w, h) in [(1920.0, 1080.0), (1280.0, 720.0)] {
        for ui_scale in [0.85, 1.0] {
            let theme = Theme::default().with_ui_scale(ui_scale);
            let l = SongSelectLayout::new(w, h, &theme).with_mod_columns(&MOD_COLUMNS);
            let at = format!("{w}x{h} x{ui_scale}");
            let bar = l.footer.bar;
            assert_eq!(bar.y + bar.height, h, "{at}");
            assert!(l.list.y + l.list.height <= bar.y, "{at}");
            assert!(l.info.y + l.info.height <= bar.y, "{at}");
            assert!(l.hint.y + l.hint.height <= bar.y, "{at}");
            // The search line is right under the toolbar, as lazer's filter control.
            let toolbar = theme.toolbar_height * l.scale;
            assert!(l.search.y >= toolbar, "{at}");
            assert!(l.search.y - toolbar <= 8.0 * l.scale, "{at}");
            // The mod select: below the toolbar, the columns and the hint above the footer
            // without overlapping the hint line.
            let m = &l.mods;
            assert!(m.title.y >= toolbar, "{at}");
            assert!(m.hint.y + m.hint.height <= bar.y, "{at}");
            assert!(m.content_bottom() <= m.hint.y, "{at}");
            assert!(m.content_bottom() <= bar.y, "{at}");
            // The floating settings panel keeps above the footer however it is placed.
            let floating = m.customise.at((w, h), 8);
            let panel = floating.panel(8);
            assert!(panel.y + panel.height <= bar.y, "{at}");
            assert!(panel.y >= 0.0 && panel.x + panel.width <= w, "{at}");
        }
    }
}

#[test]
fn the_buttons_stay_inside_the_window_with_the_most_presets() {
    for (w, h) in [(1280.0, 720.0), (800.0, 600.0)] {
        let l = SongSelectLayout::new(w, h, &Theme::default());
        let b = l.footer.buttons(FooterState {
            presets: MAX_PRESETS,
            pin: true,
            reset: true,
        });
        let last = b.reset.unwrap();
        assert!(last.x + last.width <= w, "{w}x{h}");
        // A narrow window shows fewer presets, none over the pin.
        let pin = b.pin.unwrap();
        for preset in &b.presets[..b.preset_count] {
            assert!(preset.x + preset.width <= pin.x, "{w}x{h}");
        }
        if w < 1000.0 {
            assert!(b.preset_count < MAX_PRESETS);
        }
    }
}
