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

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

#[test]
fn unit_scale_fits_the_screen_in_the_window() {
    assert_eq!(unit_scale(640.0, 480.0), 1.0);
    assert_eq!(unit_scale(1920.0, 1080.0), 2.25);
    assert_eq!(unit_scale(1280.0, 1024.0), 2.0);
}

#[test]
fn origin_lands_on_anchor_plus_offset() {
    let size = [100.0, 20.0];
    for anchor in [
        Anchor::TopLeft,
        Anchor::TopCentre,
        Anchor::TopRight,
        Anchor::CentreLeft,
        Anchor::Centre,
        Anchor::CentreRight,
        Anchor::BottomLeft,
        Anchor::BottomCentre,
        Anchor::BottomRight,
    ] {
        for (width, height) in [(1920.0, 1080.0), (800.0, 600.0), (1080.0, 1920.0)] {
            let element = LayoutElement::new(ComponentKind::Score, anchor, [10.0, -4.0]);
            let map = element.transform(width, height, size);
            let unit = unit_scale(width, height);
            let f = anchor.fraction();
            let expected = [f[0] * width + 10.0 * unit, f[1] * height - 4.0 * unit];
            let origin = [f[0] * size[0], f[1] * size[1]];
            assert!(
                close(map.apply(origin), expected),
                "{anchor:?} {width}x{height}"
            );
        }
    }
}

#[test]
fn top_right_element_ends_at_the_right_edge() {
    let element = LayoutElement::new(ComponentKind::Score, Anchor::TopRight, [0.0, 0.0]);
    let map = element.transform(1280.0, 720.0, [200.0, 40.0]);
    assert!(close(map.apply([0.0, 0.0]), [1080.0, 0.0]));
    assert!(close(map.apply([200.0, 40.0]), [1280.0, 40.0]));
}

#[test]
fn scale_and_rotation_turn_about_the_origin() {
    let mut element = LayoutElement::new(ComponentKind::Health, Anchor::Centre, [0.0, 0.0]);
    element.origin = Anchor::TopLeft;
    element.scale = [2.0, 3.0];
    element.rotation = 90.0;
    let map = element.transform(640.0, 480.0, [10.0, 10.0]);
    // The origin stays at the centre of the window.
    assert!(close(map.apply([0.0, 0.0]), [320.0, 240.0]));
    // Clockwise: the element's right (x) points down, its down (y) points left.
    assert!(close(map.apply([1.0, 0.0]), [320.0, 242.0]));
    assert!(close(map.apply([0.0, 1.0]), [317.0, 240.0]));
}

#[test]
fn negative_scale_mirrors_about_the_origin() {
    let mut element = LayoutElement::new(
        ComponentKind::HitErrorMeter,
        Anchor::CentreRight,
        [0.0, 0.0],
    );
    element.origin = Anchor::CentreLeft;
    element.scale = [-1.0, 1.0];
    let map = element.transform(640.0, 480.0, [20.0, 100.0]);
    assert!(close(map.apply([0.0, 50.0]), [640.0, 240.0]));
    assert!(close(map.apply([20.0, 50.0]), [620.0, 240.0]));
}

#[test]
fn attach_is_read_and_written() {
    let json = r#"{"version": 1, "name": "x", "gameplay": [
        {"component": "score", "anchor": "top_right", "origin": "top_right", "position": [0, 0]},
        {"component": "accuracy", "anchor": "top_right", "origin": "top_right", "position": [-1, 2],
         "attach": {"to": "score", "point": "bottom_right"}}
    ]}"#;
    let layout = from_json(json).unwrap();
    assert_eq!(layout.gameplay[0].attach, None);
    assert_eq!(
        layout.gameplay[1].attach,
        Some(Attach {
            to: ComponentKind::Score,
            point: Anchor::BottomRight,
        })
    );
    assert!(layout.gameplay[1].extra.is_empty());
    assert_eq!(from_json(&to_json(&layout)).unwrap(), layout);
    // Elements without it do not write it.
    assert!(!to_json(&layout).contains("\"attach\": null"));
}

#[test]
fn stable_hangs_the_accuracy_and_the_progress_off_the_score() {
    for b in [Builtin::Stable, Builtin::Slam] {
        let layout = b.layout();
        let attach = |kind: ComponentKind| {
            layout
                .gameplay
                .iter()
                .find(|e| e.component == kind)
                .and_then(|e| e.attach.clone())
        };
        assert_eq!(
            attach(ComponentKind::Accuracy).map(|a| (a.to, a.point)),
            Some((ComponentKind::Score, Anchor::BottomRight))
        );
        assert_eq!(
            attach(ComponentKind::SongProgress).map(|a| (a.to, a.point)),
            Some((ComponentKind::Accuracy, Anchor::CentreLeft))
        );
    }
}

#[test]
fn transform_from_places_the_origin_at_the_point_plus_the_offset() {
    let mut element =
        LayoutElement::new(ComponentKind::SongProgress, Anchor::TopRight, [-10.0, 0.0]);
    element.origin = Anchor::CentreRight;
    // 1280 × 960: 2 pixels per unit.
    let map = element.transform_from([500.0, 100.0], 1280.0, 960.0, [40.0, 40.0]);
    assert!(close(map.apply([40.0, 20.0]), [480.0, 100.0]));
}

/// Migrated copies of either preset are brought up to the stable one's gameplay section
/// (`slam-app`'s `refresh_migrated_copies`): the two must stay the same until the slam
/// preset gets its own components (ADR-0109).
#[test]
fn stable_and_slam_share_their_gameplay_section() {
    assert_eq!(
        Builtin::Stable.layout().gameplay,
        Builtin::Slam.layout().gameplay
    );
}

#[test]
fn the_game_scale_grows_an_element_about_its_anchor() {
    let element = LayoutElement {
        origin: Anchor::TopRight,
        scale: [1.0, 0.5],
        ..LayoutElement::new(ComponentKind::Score, Anchor::TopRight, [-10.0, 5.0])
    };
    let scaled = element.scaled_by(1.5);
    assert_eq!(scaled.scale, [1.5, 0.75]);
    assert_eq!(scaled.position, [-15.0, 7.5]);
    // The corner placed on the anchor stays on it, further in by the scaled offset.
    let map = scaled.transform(1280.0, 960.0, [100.0, 40.0]);
    let corner = map.apply([100.0, 0.0]);
    let unit = super::place::unit_scale(1280.0, 960.0);
    assert!((corner[0] - (1280.0 - 15.0 * unit)).abs() < 1e-3);
    assert!((corner[1] - 7.5 * unit).abs() < 1e-3);
}
