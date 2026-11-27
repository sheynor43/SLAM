//! The boxes inside one row never overlap (ADR-0180).

use std::sync::Arc;

use super::*;
use crate::theme::ColourScheme;

fn spec(kind: RowKind) -> RowSpec {
    RowSpec {
        section: 0,
        kind,
        restart_required: true,
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.width > 0.0
        && a.height > 0.0
        && b.width > 0.0
        && b.height > 0.0
        && a.x < b.x + b.width - 0.01
        && b.x < a.x + a.width - 0.01
        && a.y < b.y + b.height - 0.01
        && b.y < a.y + a.height - 0.01
}

#[test]
fn name_marker_and_hint_boxes_of_every_row_kind_never_overlap() {
    use slam_text::FontRegistry;
    let registry = Arc::new(FontRegistry::with_builtin());
    let kinds = [
        RowKind::Slider {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: 5.0,
        },
        RowKind::Toggle { value: false },
        RowKind::Dropdown {
            options: 3,
            selected: 0,
        },
        RowKind::Key {
            value: KeyValue::Key(29),
        },
        RowKind::Button,
    ];
    let languages = [
        (
            "A long setting name",
            "after restart",
            "A note on the row\nand a second line",
        ),
        (
            "Очень длинное название настройки",
            "после перезапуска",
            "Заметка к строке\nи вторая строка",
        ),
    ];
    for kind in kinds {
        for (label, marker, hint) in languages {
            for ui_scale in [0.85, 1.0] {
                for (width, height) in [(1920.0, 1080.0), (1280.0, 720.0), (600.0, 720.0)] {
                    let theme = Theme::new(ColourScheme::default(), ui_scale);
                    let rows = [spec(kind)];
                    let labels = SettingsLabels {
                        sections: vec!["A".to_owned()],
                        rows: vec![RowLabels {
                            label: label.to_owned(),
                            hint: hint.to_owned(),
                            ..RowLabels::default()
                        }],
                        restart: marker.to_owned(),
                        ..SettingsLabels::default()
                    };
                    let mut view = SettingsView::new(registry.clone(), theme, &rows, &labels);
                    view.lay_out_for(width, height);
                    let layout = SettingsLayout::new(width, height, &theme);
                    let boxes = view.row_boxes(&layout, 0, 0);
                    let row = layout.row(0);
                    let marker_box = boxes.marker.expect("a restart row has a marker box");
                    let mut parts = vec![("label", boxes.label), ("marker", marker_box)];
                    for (i, hint) in boxes.hints.iter().enumerate() {
                        if hint.height > 0.0 {
                            assert!(
                                hint.y >= row.y - 0.01
                                    && hint.y + hint.height <= row.y + row.height + 0.01,
                                "hint line {i} leaves the row at {width}x{height}"
                            );
                            parts.push(("hint", *hint));
                        }
                    }
                    for (i, (na, a)) in parts.iter().enumerate() {
                        for (nb, b) in &parts[i + 1..] {
                            assert!(
                                !overlaps(a, b),
                                "{na} {a:?} overlaps {nb} {b:?} ({kind:?}, {label}, {ui_scale}, {width}x{height})"
                            );
                        }
                    }
                    if matches!(kind, RowKind::Toggle { .. }) {
                        let track = ToggleLayout::new(row, &theme, layout.scale).track;
                        for (_, part) in &parts {
                            assert!(
                                !overlaps(part, &track),
                                "{part:?} over the toggle {track:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
