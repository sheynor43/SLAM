use super::*;

fn count(layout: &HudLayout, kind: &ComponentKind) -> usize {
    layout
        .gameplay
        .iter()
        .filter(|e| &e.component == kind)
        .count()
}

#[test]
fn builtins_round_trip() {
    for b in Builtin::ALL {
        let layout = b.layout();
        assert_eq!(from_json(&to_json(&layout)).unwrap(), layout);
    }
}

#[test]
fn builtin_ids_are_unique_and_round_trip() {
    let mut ids: Vec<_> = Builtin::ALL.iter().map(|b| b.id()).collect();
    for b in Builtin::ALL {
        assert_eq!(Builtin::from_id(b.id()), Some(b));
    }
    assert_eq!(Builtin::from_id("builtin:nope"), None);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

#[test]
fn lazer_has_two_hit_error_meters() {
    assert_eq!(
        count(&Builtin::Lazer.layout(), &ComponentKind::HitErrorMeter),
        2
    );
    assert_eq!(
        count(&Builtin::Stable.layout(), &ComponentKind::HitErrorMeter),
        1
    );
}

#[test]
fn unknown_data_survives_a_round_trip() {
    let text = r#"{
        "version": 1, "name": "n", "future": {"a": [1, 2]},
        "gameplay": [{
            "component": "hologram", "anchor": "centre", "origin": "top_left",
            "position": [1.5, 2.0], "glow": 3,
            "properties": {"mystery": "x"}
        }]
    }"#;
    let layout = from_json(text).unwrap();
    assert_eq!(
        layout.gameplay[0].component,
        ComponentKind::Unknown("hologram".into())
    );
    let again = from_json(&to_json(&layout)).unwrap();
    assert_eq!(again, layout);
    assert_eq!(again.extra["future"], serde_json::json!({"a": [1, 2]}));
    assert_eq!(again.gameplay[0].extra["glow"], 3);
    assert_eq!(again.gameplay[0].properties["mystery"], "x");
    assert!(to_json(&again).contains("\"hologram\""));
}

#[test]
fn newer_version_is_rejected() {
    let err = from_json(r#"{"version": 2, "name": "n"}"#).unwrap_err();
    assert!(matches!(err, LayoutError::UnsupportedVersion(2)));
}

#[test]
fn missing_version_is_rejected() {
    assert!(matches!(
        from_json(r#"{"name": "n"}"#).unwrap_err(),
        LayoutError::MissingVersion
    ));
    assert!(matches!(
        from_json(r#"{"version": "1", "name": "n"}"#).unwrap_err(),
        LayoutError::MissingVersion
    ));
}

#[test]
fn broken_json_is_an_error() {
    assert!(matches!(from_json("{").unwrap_err(), LayoutError::Json(_)));
}

#[test]
fn bad_sprite_names_are_rejected() {
    for bad in ["../x.png", "a/b.png", "a\\b.png", "C:x.png", "", "..", "."] {
        let text = serde_json::json!({
            "version": 1, "name": "n",
            "results": [{"component": "score", "anchor": "centre", "origin": "centre",
                         "position": [0, 0], "sprite": bad}]
        })
        .to_string();
        match from_json(&text) {
            Err(LayoutError::InvalidSprite(s)) => assert_eq!(s, bad),
            other => panic!("{bad:?}: {other:?}"),
        }
    }
}

#[test]
fn a_plain_sprite_name_is_accepted() {
    let text = r#"{"version": 1, "name": "n", "pause": [{"component": "score",
        "anchor": "centre", "origin": "centre", "position": [0, 0], "sprite": "my.sprite.png"}]}"#;
    assert_eq!(
        from_json(text).unwrap().pause[0].sprite.as_deref(),
        Some("my.sprite.png")
    );
}

#[test]
fn defaults_are_applied() {
    let text = r#"{"version": 1, "name": "n", "gameplay": [{"component": "combo",
        "anchor": "bottom_left", "origin": "bottom_left", "position": [5, -5]}]}"#;
    let layout = from_json(text).unwrap();
    assert!(layout.pause.is_empty() && layout.results.is_empty());
    let e = &layout.gameplay[0];
    assert_eq!(e.scale, [1.0, 1.0]);
    assert_eq!(e.rotation, 0.0);
    assert_eq!(e.alpha, 1.0);
    assert!(e.visible);
    assert_eq!(e.perspective, [0.0, 0.0]);
    assert_eq!(e.sprite, None);
    assert!(e.properties.is_empty());
    assert!(e.extra.is_empty());
    let out = to_json(&layout);
    assert!(!out.contains("sprite") && !out.contains("properties"));
    assert!(out.ends_with("}\n"));
}
