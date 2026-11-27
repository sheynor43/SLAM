//! Parsing and editing on responses recorded from a live OTD 0.6.8.1 daemon.

use serde_json::Value;
use slam_otd::model::{ABSOLUTE_MODE, RELATIVE_MODE, is_supported_version, parse_tablets};
use slam_otd::{
    AbsoluteFlag, AppInfo, Area, AreaKind, Error, RelativeField, SettingValue, Settings,
    short_type_name,
};

fn result(sample: &str) -> Value {
    let msg: Value = serde_json::from_str(sample).unwrap();
    msg["result"].clone()
}

fn settings() -> Settings {
    Settings::from_value(result(include_str!("data/get_settings.json"))).unwrap()
}

#[test]
fn application_info() {
    let app = AppInfo::from_value(&result(include_str!("data/get_application_info.json"))).unwrap();
    assert_eq!(app.version, "0.6.8.1");
    assert_eq!(
        app.settings_file,
        std::path::PathBuf::from("/home/user/.config/OpenTabletDriver/settings.json")
    );
    assert!(app.is_supported());
}

#[test]
fn supported_versions() {
    for v in ["0.6.8.1", "0.6.4.0", "0.6.6-beta"] {
        assert!(is_supported_version(v), "{v}");
    }
    for v in ["0.7.0.0", "0.5.3", "1.6.0", "", "garbage"] {
        assert!(!is_supported_version(v), "{v}");
    }
}

#[test]
fn tablets() {
    assert!(
        parse_tablets(&result(include_str!("data/get_tablets_empty.json")))
            .unwrap()
            .is_empty()
    );
    let tablets = parse_tablets(&result(include_str!("data/get_tablets_ctl470.json"))).unwrap();
    assert_eq!(tablets.len(), 1);
    assert_eq!(tablets[0].name, "Wacom CTL-470");
    assert_eq!(tablets[0].width_mm, 147.2);
    assert_eq!(tablets[0].height_mm, 92.0);
    assert!(parse_tablets(&serde_json::json!({})).is_err());
}

#[test]
fn profiles_are_read() {
    let s = settings();
    assert_eq!(s.profile_count(), 2);
    let i = s.profile_index("Wacom CTL-470").unwrap();
    let p = s.profile(i).unwrap();
    assert_eq!(p.output_mode, ABSOLUTE_MODE);
    assert!(p.is_absolute());
    let abs = p.absolute.unwrap();
    assert_eq!(
        abs.tablet,
        Area {
            width: 60.0,
            height: 43.0,
            x: 21.500002,
            y: 62.0,
            rotation: -90.0
        }
    );
    assert_eq!(abs.display.width, 1920.0);
    assert_eq!(abs.display.x, 2880.0);
    assert!(abs.clipping && !abs.area_limiting && !abs.lock_aspect_ratio);
    let rel = p.relative.unwrap();
    assert_eq!(rel.x_sensitivity, 15.0);
    assert_eq!(rel.rotation, -90.0);

    assert_eq!(p.filters.len(), 5);
    let noise = &p.filters[2];
    assert_eq!(short_type_name(&noise.path), "NoiseReduction");
    assert!(!noise.enabled);
    assert_eq!(noise.settings[0].property, "Samples");
    assert_eq!(noise.settings[0].value, SettingValue::Int(10));
    assert_eq!(noise.settings[1].value, SettingValue::Float(0.5));
    let antichatter = &p.filters[4];
    assert!(antichatter.enabled);
    assert!(
        antichatter
            .settings
            .iter()
            .any(|s| matches!(s.value, SettingValue::Bool(_)))
    );

    assert!(s.profile_index("No Such Tablet").is_none());
    assert!(s.profile(9).is_none());
}

#[test]
fn untouched_settings_round_trip_exactly() {
    let raw = result(include_str!("data/get_settings.json"));
    let s = Settings::from_value(raw.clone()).unwrap();
    assert_eq!(s.as_value(), &raw);
    let reparsed: Value = serde_json::from_str(&s.to_file_json()).unwrap();
    assert_eq!(reparsed, raw);
    assert!(
        s.to_file_json()
            .starts_with("{\n  \"Revision\": \"0.6.8.1\",\n  \"Profiles\"")
    );
}

#[test]
fn edits_touch_only_their_fields() {
    let raw = result(include_str!("data/get_settings.json"));
    let mut s = Settings::from_value(raw.clone()).unwrap();
    let area = Area {
        width: 50.1,
        height: 30.0,
        x: 25.0,
        y: 40.0,
        rotation: 0.0,
    };
    s.set_area(0, AreaKind::Tablet, area).unwrap();
    s.set_absolute_flag(0, AbsoluteFlag::LockAspectRatio, true)
        .unwrap();
    s.set_relative(0, RelativeField::YSensitivity, 12.5)
        .unwrap();
    s.set_filter_enabled(0, 2, true).unwrap();
    s.set_filter_value(0, 2, 0, SettingValue::Int(4)).unwrap();
    s.set_filter_value(0, 2, 1, SettingValue::Float(0.1))
        .unwrap();

    let p = s.profile(0).unwrap();
    assert_eq!(p.absolute.as_ref().unwrap().tablet, area);
    assert!(p.absolute.as_ref().unwrap().lock_aspect_ratio);
    assert_eq!(p.relative.unwrap().y_sensitivity, 12.5);
    assert!(p.filters[2].enabled);
    assert_eq!(p.filters[2].settings[0].value, SettingValue::Int(4));

    // Exact JSON written: shortest f32 decimals, ints stay ints.
    let v = s.as_value();
    let prof = &v["Profiles"][0];
    assert_eq!(
        prof["AbsoluteModeSettings"]["Tablet"]["Width"].to_string(),
        "50.1"
    );
    assert_eq!(prof["Filters"][2]["Settings"][0]["Value"].to_string(), "4");
    assert_eq!(
        prof["Filters"][2]["Settings"][1]["Value"].to_string(),
        "0.1"
    );

    // Everything else is unchanged: undo the edits on a copy and compare.
    let mut expected = raw.clone();
    let e = &mut expected["Profiles"][0];
    e["AbsoluteModeSettings"]["Tablet"] = prof["AbsoluteModeSettings"]["Tablet"].clone();
    e["AbsoluteModeSettings"]["LockAspectRatio"] = Value::Bool(true);
    e["RelativeModeSettings"]["YSensitivity"] =
        prof["RelativeModeSettings"]["YSensitivity"].clone();
    e["Filters"][2] = prof["Filters"][2].clone();
    assert_eq!(v, &expected);
    // Other profile, bindings and tools are untouched.
    assert_eq!(v["Profiles"][1], raw["Profiles"][1]);
    assert_eq!(prof["Bindings"], raw["Profiles"][0]["Bindings"]);
    assert_eq!(v["Tools"], raw["Tools"]);
}

#[test]
fn wrong_edits_are_rejected() {
    let mut s = settings();
    let before = s.clone();
    assert!(matches!(
        s.set_filter_value(0, 2, 0, SettingValue::Float(1.5)),
        Err(Error::WrongValueType)
    ));
    assert!(matches!(
        s.set_filter_value(0, 2, 0, SettingValue::Unset),
        Err(Error::WrongValueType)
    ));
    assert!(matches!(
        s.set_filter_enabled(0, 99, true),
        Err(Error::NoSuchItem)
    ));
    assert!(matches!(
        s.set_area(7, AreaKind::Display, Area::default()),
        Err(Error::NoSuchItem)
    ));
    assert_eq!(s, before);
}

#[test]
fn output_mode_switch_resets_mode_settings() {
    let mut s = settings();
    s.set_output_mode(0, ABSOLUTE_MODE).unwrap();
    assert_eq!(s, settings());
    s.set_output_mode(0, RELATIVE_MODE).unwrap();
    let p = s.profile(0).unwrap();
    assert_eq!(p.output_mode, RELATIVE_MODE);
    assert!(!p.is_absolute());
    assert_eq!(
        s.as_value()["Profiles"][0]["OutputMode"],
        serde_json::json!({"Path": RELATIVE_MODE, "Settings": [], "Enable": true})
    );
}

#[test]
fn preset_keeps_behaviour_but_not_the_tablet_or_bindings() {
    let s = settings();
    let preset = s.profile_preset(0).unwrap();
    let keys: Vec<&str> = preset
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(keys.contains(&"Filters") && keys.contains(&"AbsoluteModeSettings"));
    assert!(!keys.contains(&"Tablet") && !keys.contains(&"Bindings"));
}

#[test]
fn preset_restores_values_edited_since() {
    let mut s = settings();
    let preset = s.profile_preset(0).unwrap();
    let before = s.clone();
    s.set_filter_enabled(0, 4, false).unwrap();
    s.set_filter_value(0, 4, 0, SettingValue::Float(77.0))
        .unwrap();
    s.set_relative(0, RelativeField::XSensitivity, 3.0).unwrap();
    assert_eq!(s.apply_profile_preset(0, &preset).unwrap(), 0);
    assert_eq!(s, before);
}

#[test]
fn preset_filters_missing_in_the_profile_are_skipped() {
    let mut s = settings();
    let preset = s.profile_preset(0).unwrap();
    // Profile 1 has no filters: the area and mode come over, the five filters do not.
    assert_eq!(s.apply_profile_preset(1, &preset).unwrap(), 5);
    let p = s.profile(1).unwrap();
    assert!(p.filters.is_empty());
    assert_eq!(p.output_mode, ABSOLUTE_MODE);
    assert_eq!(p.absolute.unwrap().tablet.width, 60.0);
    assert_eq!(s.profile(1).unwrap().tablet, "Huion WH1409 V2");
}

#[test]
fn a_preset_that_is_no_object_is_refused() {
    let mut s = settings();
    assert!(s.apply_profile_preset(0, &Value::Null).is_err());
}

#[test]
fn malformed_settings_are_rejected() {
    assert!(Settings::from_value(serde_json::json!({"Revision": "0.6.8.1"})).is_err());
    assert!(Settings::from_value(serde_json::json!({"Profiles": [{"NoTablet": 1}]})).is_err());
}
