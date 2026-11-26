use super::*;
use crate::settings::Settings as AppSettings;
use crate::shell::rows::{Rows, SECTIONS};
use slam_otd::model::parse_tablets;
use slam_otd::{AppInfo, Snapshot};

fn result(sample: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(sample).unwrap()["result"].clone()
}

fn snapshot() -> Box<Snapshot> {
    Box::new(Snapshot {
        app: AppInfo::from_value(&result(include_str!(
            "../../../../slam-otd/tests/data/get_application_info.json"
        )))
        .unwrap(),
        tablets: parse_tablets(&result(include_str!(
            "../../../../slam-otd/tests/data/get_tablets_ctl470.json"
        )))
        .unwrap(),
        settings: slam_otd::Settings::from_value(result(include_str!(
            "../../../../slam-otd/tests/data/get_settings.json"
        )))
        .unwrap(),
    })
}

fn screen() -> SettingsScreen {
    let rows = Rows::new(&AppSettings::default());
    SettingsScreen::new(&rows.specs(&AppSettings::default()), SECTIONS)
}

fn ready() -> (OtdSection, SettingsScreen) {
    let mut otd = OtdSection::default();
    otd.take(Event::Snapshot(snapshot()));
    let mut screen = screen();
    otd.refresh(&mut screen);
    (otd, screen)
}

fn shown(screen: &SettingsScreen, row: OtdRow) -> bool {
    !screen.is_hidden(row.index())
}

fn slider(screen: &SettingsScreen, row: OtdRow) -> f64 {
    match screen.widget(row.index()) {
        Some(RowWidget::Slider(s)) => s.value(),
        other => panic!("{row:?} is {other:?}"),
    }
}

fn toggle(screen: &SettingsScreen, row: OtdRow) -> bool {
    match screen.widget(row.index()) {
        Some(RowWidget::Toggle(t)) => t.value(),
        other => panic!("{row:?} is {other:?}"),
    }
}

#[test]
fn rows_follow_the_other_sections_and_fit_the_screen() {
    let specs = Rows::new(&AppSettings::default()).specs(&AppSettings::default());
    assert_eq!(specs.len(), FIRST + OtdRow::ALL.len());
    assert!(specs[FIRST..].iter().all(|s| s.section == SECTION));
    assert_eq!(OtdRow::at(FIRST), Some(OtdRow::NotRunning));
    assert_eq!(OtdRow::at(FIRST - 1), None);
    for row in OtdRow::ALL {
        assert_eq!(OtdRow::at(row.index()), Some(row));
    }
    // The preview's pads follow it directly.
    for pad in 0..PREVIEW_LINES - 1 {
        assert_eq!(
            OtdRow::PreviewPad(pad as u8).index(),
            OtdRow::Preview.index() + 1 + pad
        );
    }
}

#[test]
fn labels_cover_every_row_in_both_languages() {
    let localizer = Localizer::builtin().unwrap();
    for language in ["en", "ru"] {
        localizer.set_language(language).unwrap();
        let rows = Rows::new(&AppSettings::default());
        let labels = rows.labels(&localizer);
        assert_eq!(labels.rows.len(), FIRST + OtdRow::ALL.len());
        assert_eq!(labels.sections.len(), SECTIONS);
        for (row, label) in OtdRow::ALL.iter().zip(&labels.rows[FIRST..]) {
            if !matches!(row, OtdRow::Preview | OtdRow::PreviewPad(_)) {
                assert!(
                    !label.label.is_empty() && !label.label.starts_with("settings-"),
                    "{language} {row:?}: {:?}",
                    label.label
                );
            }
            if let Some(key) = row.button_key() {
                assert!(!label.button.starts_with("settings-"), "{key}");
            }
            assert_eq!(label.dynamic_label, row.dynamic());
        }
        let modes = &labels.rows[OtdRow::OutputMode.index()].options;
        assert_eq!(modes.len(), mode_options().len() + 1);
        assert!(modes.iter().all(|m| !m.starts_with("settings-")));
        assert_eq!(
            Rows::search_texts(&localizer).len(),
            FIRST + OtdRow::ALL.len()
        );
    }
}

#[test]
fn before_any_news_only_refresh_shows() {
    let otd = OtdSection::default();
    let mut screen = screen();
    otd.refresh(&mut screen);
    for row in OtdRow::ALL {
        assert_eq!(shown(&screen, row), row == OtdRow::Refresh, "{row:?}");
    }
}

#[test]
fn no_daemon_and_unsupported_show_their_notes_only() {
    for (event, note, download) in [
        (Event::NotRunning, OtdRow::NotRunning, true),
        (
            Event::Unsupported {
                version: "0.7.0.0".into(),
            },
            OtdRow::Unsupported,
            true,
        ),
        (Event::Failed("x".into()), OtdRow::Failed, false),
    ] {
        let mut otd = OtdSection::default();
        otd.take(event);
        let mut screen = screen();
        otd.refresh(&mut screen);
        for row in OtdRow::ALL {
            let expected =
                row == note || row == OtdRow::Refresh || (row == OtdRow::Download && download);
            assert_eq!(shown(&screen, row), expected, "{note:?}: {row:?}");
        }
    }
}

#[test]
fn the_download_button_asks_for_the_site() {
    let mut otd = OtdSection::default();
    otd.take(Event::NotRunning);
    assert_eq!(otd.press(OtdRow::Download), Pressed::Download);
    assert_eq!(otd.press(OtdRow::Refresh), Pressed::Nothing);
}

#[test]
fn a_snapshot_shows_the_connected_tablets_profile() {
    let (otd, screen) = ready();
    assert!(shown(&screen, OtdRow::Tablet));
    assert!(!shown(&screen, OtdRow::NextTablet), "one tablet");
    assert!(!shown(&screen, OtdRow::NoTablet));
    assert!(!shown(&screen, OtdRow::Download));
    assert!(
        screen.is_disabled(OtdRow::Apply.index()),
        "nothing to apply"
    );
    assert_eq!(
        &*screen.dynamic_labels()[OtdRow::Tablet.index()],
        "Wacom CTL-470"
    );

    // Absolute mode: the area groups and the preview, not the relative one.
    for row in [
        OtdRow::TabletAreaHeader,
        OtdRow::Preview,
        OtdRow::PreviewPad(0),
        OtdRow::DisplayAreaHeader,
        OtdRow::FiltersHeader,
    ] {
        assert!(shown(&screen, row), "{row:?}");
    }
    assert!(!shown(&screen, OtdRow::RelativeHeader));
    assert!(!shown(&screen, OtdRow::RelativeX));
    assert!(!shown(&screen, OtdRow::NoFilters));
    assert!((slider(&screen, OtdRow::TabletWidth) - 60.0).abs() < 1e-6);
    assert!((slider(&screen, OtdRow::TabletX) - 21.5).abs() < 0.01);
    assert!((slider(&screen, OtdRow::TabletRotation) + 90.0).abs() < 1e-6);
    assert!((slider(&screen, OtdRow::DisplayX) - 2880.0).abs() < 1e-6);
    assert!(toggle(&screen, OtdRow::Clipping));
    assert!(!toggle(&screen, OtdRow::LockAspect));
    let preview = screen.snapshot().preview;
    assert_eq!(preview.surface, [147.2, 92.0]);
    assert_eq!(preview.size, [60.0, 43.0]);
    assert_eq!(preview.rotation_deg, -90.0);

    // Five filters; only Antichatter (the fifth) is on, and only its settings show.
    for f in 0..MAX_FILTERS {
        assert_eq!(shown(&screen, OtdRow::Filter(f as u8)), f < 5, "{f}");
    }
    assert!(toggle(&screen, OtdRow::Filter(4)));
    assert!(!toggle(&screen, OtdRow::Filter(2)));
    assert_eq!(
        &*screen.dynamic_labels()[OtdRow::Filter(2).index()],
        "NoiseReduction"
    );
    for f in 0..4 {
        for s in 0..MAX_PARAMS {
            assert!(!shown(&screen, OtdRow::ParamNumber(f, s as u8)), "{f} {s}");
            assert!(!shown(&screen, OtdRow::ParamFlag(f, s as u8)), "{f} {s}");
        }
    }
    let numbers = (0..MAX_PARAMS)
        .filter(|s| shown(&screen, OtdRow::ParamNumber(4, *s as u8)))
        .count();
    let flags = (0..MAX_PARAMS)
        .filter(|s| shown(&screen, OtdRow::ParamFlag(4, *s as u8)))
        .count();
    assert_eq!(
        (numbers, flags),
        (10, 1),
        "Antichatter: PredictionEnabled is a flag"
    );
    assert_eq!(
        &*screen.dynamic_labels()[OtdRow::ParamNumber(4, 0).index()],
        "· Latency"
    );
    // The settings follow their filter on the screen.
    assert!(OtdRow::ParamNumber(4, 0).index() > OtdRow::Filter(4).index());
    assert!(OtdRow::ParamNumber(4, 0).index() < OtdRow::Filter(5).index());
    assert_eq!(otd.status, Status::Ready);
}

#[test]
fn edits_go_to_the_draft_and_enable_apply() {
    let (mut otd, mut screen) = ready();
    otd.apply(OtdRow::TabletWidth, RowValue::Slider(50.0));
    otd.apply(OtdRow::Filter(2), RowValue::Toggle(true));
    otd.refresh(&mut screen);
    assert!(!screen.is_disabled(OtdRow::Apply.index()));
    assert!(!screen.is_disabled(OtdRow::Revert.index()));
    let p = otd.draft.as_ref().unwrap().profile(0).unwrap();
    assert_eq!(p.absolute.unwrap().tablet.width, 50.0);
    assert!(p.filters[2].enabled);
    assert!((slider(&screen, OtdRow::TabletWidth) - 50.0).abs() < 1e-6);

    otd.press(OtdRow::Revert);
    otd.refresh(&mut screen);
    assert_eq!(
        otd.draft.as_ref(),
        Some(&otd.snapshot.as_ref().unwrap().settings)
    );
    assert!(screen.is_disabled(OtdRow::Apply.index()));
    assert!((slider(&screen, OtdRow::TabletWidth) - 60.0).abs() < 1e-6);
}

#[test]
fn locked_aspect_ratio_keeps_the_display_ratio() {
    let (mut otd, _) = ready();
    otd.apply(OtdRow::LockAspect, RowValue::Toggle(true));
    otd.apply(OtdRow::TabletWidth, RowValue::Slider(64.0));
    let t = otd
        .draft
        .as_ref()
        .unwrap()
        .profile(0)
        .unwrap()
        .absolute
        .unwrap()
        .tablet;
    assert_eq!(t.width, 64.0);
    assert!((t.height - 36.0).abs() < 1e-4, "1920×1080: {}", t.height);
}

#[test]
fn turning_a_filter_on_shows_its_settings_with_their_types() {
    let (mut otd, mut screen) = ready();
    // NoiseReduction: Samples (int), DistanceThreshold (float).
    otd.apply(OtdRow::Filter(2), RowValue::Toggle(true));
    otd.refresh(&mut screen);
    assert!(shown(&screen, OtdRow::ParamNumber(2, 0)));
    assert!(shown(&screen, OtdRow::ParamNumber(2, 1)));
    assert!(!shown(&screen, OtdRow::ParamNumber(2, 2)));
    assert!((slider(&screen, OtdRow::ParamNumber(2, 0)) - 10.0).abs() < 1e-9);
    otd.apply(OtdRow::ParamNumber(2, 0), RowValue::Slider(4.4));
    otd.apply(
        OtdRow::ParamNumber(2, 1),
        RowValue::Slider(0.30000000000000004),
    );
    let draft = otd.draft.as_ref().unwrap().as_value();
    let settings = &draft["Profiles"][0]["Filters"][2]["Settings"];
    assert_eq!(settings[0]["Value"].to_string(), "4");
    assert_eq!(settings[1]["Value"].to_string(), "0.3");
    // Off again: the settings hide, their values stay in the draft.
    otd.apply(OtdRow::Filter(2), RowValue::Toggle(false));
    otd.refresh(&mut screen);
    assert!(!shown(&screen, OtdRow::ParamNumber(2, 0)));
    assert_eq!(
        otd.draft.as_ref().unwrap().as_value()["Profiles"][0]["Filters"][2]["Settings"][0]["Value"]
            .to_string(),
        "4"
    );
}

#[test]
fn edits_are_locked_while_a_command_is_in_flight() {
    let (mut otd, mut screen) = ready();
    otd.apply(OtdRow::Clipping, RowValue::Toggle(false));
    // A worker whose daemon never answers.
    otd.service = Some(OtdService::spawn_with(|| Ok(Stuck)).unwrap());
    otd.press(OtdRow::Apply);
    otd.refresh(&mut screen);
    assert!(screen.is_disabled(OtdRow::Clipping.index()));
    assert!(screen.is_disabled(OtdRow::Filter(4).index()));
    assert!(screen.is_disabled(OtdRow::Apply.index()));
    assert!(!screen.is_disabled(OtdRow::Download.index()));
    // Given up after the timeout: the editing comes back, with the error note.
    assert!(!otd.poll(1));
    assert!(otd.poll(1 + WORKER_TIMEOUT_NS + 1));
    assert!(otd.service.is_none());
    otd.refresh(&mut screen);
    assert!(shown(&screen, OtdRow::Failed));
    assert!(!screen.is_disabled(OtdRow::Clipping.index()));
    assert!(!screen.is_disabled(OtdRow::Apply.index()));
}

#[test]
fn unchanged_names_keep_their_generation() {
    let (otd, mut screen) = ready();
    let generation = screen.dynamic_generation();
    otd.refresh(&mut screen);
    assert_eq!(screen.dynamic_generation(), generation);
}

#[test]
fn switching_to_relative_mode_swaps_the_rows() {
    let (mut otd, mut screen) = ready();
    let relative = mode_options()
        .iter()
        .position(|m| *m == RELATIVE_MODE)
        .unwrap();
    otd.apply(OtdRow::OutputMode, RowValue::Choice(relative));
    otd.refresh(&mut screen);
    assert!(!shown(&screen, OtdRow::Preview));
    assert!(!shown(&screen, OtdRow::TabletWidth));
    assert!(shown(&screen, OtdRow::RelativeX));
    assert!((slider(&screen, OtdRow::RelativeX) - 15.0).abs() < 1e-6);
    // "Other" keeps the mode.
    otd.apply(OtdRow::OutputMode, RowValue::Choice(mode_options().len()));
    assert_eq!(
        otd.draft.as_ref().unwrap().profile(0).unwrap().output_mode,
        RELATIVE_MODE
    );
}

#[test]
fn a_plugin_output_mode_shows_as_other() {
    let mut snap = snapshot();
    snap.settings
        .set_output_mode(0, "SomePlugin.Output.Mode")
        .unwrap();
    let mut otd = OtdSection::default();
    otd.take(Event::Snapshot(snap));
    let mut screen = screen();
    otd.refresh(&mut screen);
    match screen.widget(OtdRow::OutputMode.index()) {
        Some(RowWidget::Dropdown(d)) => {
            assert_eq!(d.count(), mode_options().len() + 1);
            assert_eq!(d.selected(), mode_options().len());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_tablet_without_a_profile_says_so() {
    let mut snap = snapshot();
    snap.tablets[0].name = "Unknown Tablet".into();
    let mut otd = OtdSection::default();
    otd.take(Event::Snapshot(snap));
    let mut screen = screen();
    otd.refresh(&mut screen);
    assert!(shown(&screen, OtdRow::NoTablet));
    assert!(!shown(&screen, OtdRow::TabletWidth));
    assert!(!shown(&screen, OtdRow::Apply));
}

#[test]
fn a_failed_apply_keeps_the_draft() {
    let (mut otd, mut screen) = ready();
    otd.apply(OtdRow::Clipping, RowValue::Toggle(false));
    otd.take(Event::Failed("pipe closed".into()));
    otd.refresh(&mut screen);
    assert!(shown(&screen, OtdRow::Failed));
    assert!(shown(&screen, OtdRow::Clipping));
    assert!(!toggle(&screen, OtdRow::Clipping));
    assert!(!screen.is_disabled(OtdRow::Apply.index()));
    // A daemon gone hides the editing.
    otd.take(Event::NotRunning);
    otd.refresh(&mut screen);
    assert!(!shown(&screen, OtdRow::Clipping));
}

#[test]
fn param_ranges_cover_the_value() {
    assert_eq!(param_range(&SettingValue::Float(0.5)), (0.0, 1.0, 0.01));
    assert_eq!(param_range(&SettingValue::Int(10)), (0.0, 1000.0, 1.0));
    assert_eq!(param_range(&SettingValue::Int(3)), (0.0, 1000.0, 1.0));
    assert_eq!(param_range(&SettingValue::Float(5.0)), (0.0, 1000.0, 0.1));
    assert_eq!(
        param_range(&SettingValue::Float(-2.0)),
        (-1000.0, 1000.0, 0.1)
    );
    assert_eq!(
        param_range(&SettingValue::Float(150.0)),
        (0.0, 10000.0, 1.0)
    );
}

/// A daemon connection that takes every write and never answers a read.
struct Stuck;

impl std::io::Read for Stuck {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        loop {
            std::thread::park();
        }
    }
}

impl std::io::Write for Stuck {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The latency setting of the Antichatter filter (filter 4, setting 0) is 18.0.
fn latency() -> OtdRow {
    OtdRow::ParamNumber(4, 0)
}

#[test]
fn typed_filter_values_stay_exact_and_may_exceed_the_range() {
    let (mut otd, mut screen) = ready();
    // Antichatter is on, so its settings show.
    otd.refresh(&mut screen);
    assert!((slider(&screen, latency()) - 18.0).abs() < 1e-9);
    let row = latency().index();
    for typed in [18.0, 1000.0, 2500.5] {
        otd.apply(latency(), RowValue::Slider(typed));
        otd.refresh(&mut screen);
        assert!((slider(&screen, latency()) - typed).abs() < 1e-9, "{typed}");
        let p = otd.draft.as_ref().unwrap().profile(0).unwrap();
        assert_eq!(p.filters[4].settings[0].value, SettingValue::Float(typed));
    }
    assert!(matches!(screen.widget(row), Some(RowWidget::Slider(s)) if s.is_free()));
}

#[test]
fn a_preset_round_trips_through_its_file() {
    let dir = std::env::temp_dir().join(format!("slam-otd-presets-{}", std::process::id()));
    let path = presets::file_in(&dir);
    assert!(presets::read(&path).is_empty());
    let snap = snapshot();
    let list = vec![
        Preset {
            name: "Preset 1".into(),
            profile: snap.settings.profile_preset(0).unwrap(),
        },
        Preset {
            name: "Smooth".into(),
            profile: snap.settings.profile_preset(1).unwrap(),
        },
    ];
    presets::write(&path, &list).unwrap();
    assert_eq!(presets::read(&path), list);
    std::fs::write(&path, b"not json").unwrap();
    assert!(presets::read(&path).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_names_skip_taken_ones() {
    let mk = |n: &str| Preset {
        name: n.into(),
        profile: serde_json::Value::Null,
    };
    assert_eq!(presets::auto_name(&[]), "Preset 1");
    assert_eq!(presets::auto_name(&[mk("Preset 1"), mk("x")]), "Preset 2");
    assert_eq!(presets::auto_name(&[mk("Preset 2")]), "Preset 1");
}

#[test]
fn presets_save_apply_and_delete_through_the_draft() {
    let (mut otd, mut screen) = ready();
    assert!(!shown(&screen, OtdRow::PresetApply));
    assert!(shown(&screen, OtdRow::PresetSave));
    otd.press(OtdRow::PresetSave);
    otd.refresh(&mut screen);
    assert_eq!(otd.presets.len(), 1);
    assert_eq!(otd.presets[0].name, "Preset 1");
    assert!(shown(&screen, OtdRow::PresetApply));
    assert!(otd.names[OtdRow::PresetName.index()].starts_with("Preset 1 (1/1)"));
    // Saving a preset does not touch the draft.
    assert!(!otd.dirty);

    otd.apply(OtdRow::TabletWidth, RowValue::Slider(50.0));
    otd.apply(latency(), RowValue::Slider(33.0));
    otd.press(OtdRow::PresetApply);
    otd.refresh(&mut screen);
    let p = otd.draft.as_ref().unwrap().profile(0).unwrap();
    assert_eq!(p.absolute.unwrap().tablet.width, 60.0);
    assert_eq!(p.filters[4].settings[0].value, SettingValue::Float(18.0));
    // Loaded into the draft, not sent: it waits for Save.
    assert!(otd.dirty);
    assert!(shown(&screen, OtdRow::Unsaved));
    assert!(!screen.is_disabled(OtdRow::Apply.index()));

    otd.press(OtdRow::PresetDelete);
    otd.refresh(&mut screen);
    assert!(otd.presets.is_empty());
    assert!(!shown(&screen, OtdRow::PresetDelete));
}

#[test]
fn a_preset_with_filters_the_profile_lacks_applies_the_rest() {
    let (mut otd, mut screen) = ready();
    // A preset with a filter the profile does not have, and a new area.
    let mut profile = otd
        .snapshot
        .as_ref()
        .unwrap()
        .settings
        .profile_preset(0)
        .unwrap();
    profile["Filters"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"Path": "Gone.Filter", "Settings": [], "Enable": true}));
    profile["AbsoluteModeSettings"]["Tablet"]["Width"] = serde_json::json!(40.0);
    otd.presets.push(Preset {
        name: "Odd".into(),
        profile,
    });
    otd.press(OtdRow::PresetApply);
    otd.refresh(&mut screen);
    let p = otd.draft.as_ref().unwrap().profile(0).unwrap();
    assert_eq!(p.filters.len(), 5);
    assert_eq!(p.absolute.unwrap().tablet.width, 40.0);
    assert!(otd.dirty);
}
