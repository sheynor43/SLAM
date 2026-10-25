use super::*;

fn theme() -> Theme {
    Theme::default()
}

fn layout() -> SettingsLayout {
    SettingsLayout::new(1280.0, 720.0, &theme())
}

fn slider(section: usize, value: f64) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Slider {
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value,
        },
        restart_required: false,
    }
}

fn toggle(section: usize, value: bool) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Toggle { value },
        restart_required: false,
    }
}

fn dropdown(section: usize, options: usize, selected: usize) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Dropdown { options, selected },
        restart_required: false,
    }
}

/// Section 0: slider, toggle, dropdown. Section 1: slider.
fn screen() -> SettingsScreen {
    SettingsScreen::new(
        &[
            slider(0, 50.0),
            toggle(0, false),
            dropdown(0, 4, 1),
            slider(1, 10.0),
        ],
        2,
    )
}

fn input(screen: &mut SettingsScreen, input: SettingsInput) -> Option<SettingsAction> {
    screen.input(input, &layout())
}

#[test]
fn starts_on_the_first_row_of_the_first_section() {
    let s = screen();
    assert_eq!((s.section(), s.focus(), s.sections()), (0, 0, 2));
    assert_eq!(s.slot(2), 2);
    assert_eq!(s.slot(3), 0);
}

#[test]
fn up_and_down_move_the_focus_within_the_section() {
    let mut s = screen();
    assert_eq!(input(&mut s, SettingsInput::Up), None);
    assert_eq!(s.focus(), 0);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    // The second section's row is not reachable from here.
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    input(&mut s, SettingsInput::Up);
    assert_eq!(s.focus(), 1);
}

#[test]
fn sections_switch_and_wrap_and_focus_their_first_row() {
    let mut s = screen();
    input(&mut s, SettingsInput::NextSection);
    assert_eq!((s.section(), s.focus()), (1, 3));
    input(&mut s, SettingsInput::NextSection);
    assert_eq!((s.section(), s.focus()), (0, 0));
    input(&mut s, SettingsInput::PrevSection);
    assert_eq!((s.section(), s.focus()), (1, 3));
}

#[test]
fn keys_change_a_slider_and_a_toggle() {
    let mut s = screen();
    assert_eq!(
        input(&mut s, SettingsInput::Right),
        Some(SettingsAction::Changed {
            row: 0,
            value: RowValue::Slider(51.0)
        })
    );
    assert_eq!(
        input(&mut s, SettingsInput::Left),
        Some(SettingsAction::Changed {
            row: 0,
            value: RowValue::Slider(50.0)
        })
    );
    input(&mut s, SettingsInput::Down);
    assert_eq!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::Changed {
            row: 1,
            value: RowValue::Toggle(true)
        })
    );
    // Arrows leave a toggle alone.
    assert_eq!(input(&mut s, SettingsInput::Left), None);
}

#[test]
fn a_closed_dropdown_steps_with_the_arrows_and_an_open_one_chooses() {
    let mut s = screen();
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    assert_eq!(
        input(&mut s, SettingsInput::Right),
        Some(SettingsAction::Changed {
            row: 2,
            value: RowValue::Choice(2)
        })
    );
    assert_eq!(input(&mut s, SettingsInput::Activate), None);
    assert_eq!(s.snapshot().widgets[2], {
        let mut d = Dropdown::new(4, 2);
        d.input(
            DropdownInput::Activate,
            &DropdownLayout::new(Rect::default(), 4, 1.0),
        );
        RowWidget::Dropdown(d)
    });
    // Open: Up/Down move the highlight, not the focus; Enter chooses.
    assert_eq!(input(&mut s, SettingsInput::Up), None);
    assert_eq!(s.focus(), 2);
    assert_eq!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::Changed {
            row: 2,
            value: RowValue::Choice(1)
        })
    );
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if !d.is_open()));
}

#[test]
fn back_closes_an_open_dropdown_first_then_the_screen() {
    let mut s = screen();
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Activate);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if d.is_open()));
    assert_eq!(input(&mut s, SettingsInput::Back), None);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if !d.is_open()));
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn switching_section_closes_an_open_dropdown() {
    let mut s = screen();
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Activate);
    input(&mut s, SettingsInput::NextSection);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if !d.is_open()));
}

fn centre(rect: Rect) -> (f32, f32) {
    let [x, y] = rect.centre();
    (x, y)
}

#[test]
fn pointer_press_on_a_tab_switches_the_section() {
    let mut s = screen();
    let (x, y) = centre(layout().tabs[1]);
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert_eq!((s.section(), s.focus()), (1, 3));
}

#[test]
fn pointer_press_on_a_toggle_row_flips_it_and_focuses_it() {
    let mut s = screen();
    let (x, y) = centre(layout().row(1));
    assert_eq!(
        input(&mut s, SettingsInput::PointerPress { x, y }),
        Some(SettingsAction::Changed {
            row: 1,
            value: RowValue::Toggle(true)
        })
    );
    assert_eq!(s.focus(), 1);
}

#[test]
fn dragging_a_slider_follows_the_pointer_until_release() {
    let mut s = screen();
    let l = layout();
    let widget = SliderLayout::new(l.widget(0), &theme(), l.scale);
    let y = widget.hit.y + 2.0;
    let at = |fraction: f32| widget.track.x + widget.track.width * fraction;
    assert_eq!(
        input(&mut s, SettingsInput::PointerPress { x: at(0.25), y }),
        Some(SettingsAction::Changed {
            row: 0,
            value: RowValue::Slider(25.0)
        })
    );
    // The pointer may leave the row while dragging.
    assert_eq!(
        input(
            &mut s,
            SettingsInput::PointerMove {
                x: at(0.75),
                y: y + 300.0
            }
        ),
        Some(SettingsAction::Changed {
            row: 0,
            value: RowValue::Slider(75.0)
        })
    );
    input(&mut s, SettingsInput::PointerRelease);
    assert_eq!(
        input(&mut s, SettingsInput::PointerMove { x: at(0.1), y }),
        None
    );
}

#[test]
fn pointer_chooses_from_an_open_list_and_a_press_elsewhere_closes_it() {
    let mut s = screen();
    let l = layout();
    let widget = l.widget(2);
    let (x, y) = centre(widget);
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if d.is_open()));
    let option = l.dropdown(2, 4).option(3);
    let (ox, oy) = centre(option);
    assert_eq!(
        input(&mut s, SettingsInput::PointerPress { x: ox, y: oy }),
        Some(SettingsAction::Changed {
            row: 2,
            value: RowValue::Choice(3)
        })
    );
    input(&mut s, SettingsInput::PointerPress { x, y });
    assert_eq!(
        input(&mut s, SettingsInput::PointerPress { x: 1.0, y: 1.0 }),
        None
    );
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if !d.is_open()));
}

#[test]
fn hover_focuses_the_row_under_the_pointer() {
    let mut s = screen();
    let (x, y) = centre(layout().row(2));
    input(&mut s, SettingsInput::PointerMove { x, y });
    assert_eq!(s.focus(), 2);
}

#[test]
fn an_empty_screen_only_closes() {
    let mut s = SettingsScreen::new(&[], 1);
    assert_eq!(input(&mut s, SettingsInput::Down), None);
    assert_eq!(input(&mut s, SettingsInput::Activate), None);
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn rows_past_the_capacity_are_ignored() {
    let rows = vec![toggle(0, false); MAX_ROWS + 5];
    let s = SettingsScreen::new(&rows, MAX_SECTIONS + 3);
    assert_eq!(s.snapshot().len, MAX_ROWS);
    assert_eq!(s.sections(), MAX_SECTIONS);
}

#[test]
fn the_layout_keeps_rows_and_tabs_inside_the_window() {
    for (w, h) in [(1280.0, 720.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let l = SettingsLayout::new(w, h, &theme());
        for slot in 0..8 {
            let row = l.row(slot);
            assert!(row.x >= 0.0 && row.x + row.width <= w, "{w}x{h}");
            assert!(row.y + row.height <= h, "{w}x{h} slot {slot}");
            let (label, widget) = (l.label(slot), l.widget(slot));
            assert!(label.x + label.width <= widget.x);
            assert!(widget.x + widget.width <= row.x + row.width + 0.01);
        }
        for tab in &l.tabs {
            assert!(tab.x + tab.width <= l.row(0).x);
        }
        let moved = l.shifted(10.0);
        assert_eq!(moved.row(1).x, l.row(1).x + 10.0);
    }
}

fn key_row(section: usize, value: KeyValue) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Key { value },
        restart_required: false,
    }
}

fn button(section: usize) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Button,
        restart_required: false,
    }
}

const Z: KeyValue = KeyValue::Key(29);
const X: KeyValue = KeyValue::Key(27);

/// Section 0: toggle, two key rows, button.
fn keys_screen() -> SettingsScreen {
    SettingsScreen::new(
        &[
            toggle(0, false),
            key_row(0, Z),
            key_row(0, KeyValue::Empty),
            button(0),
        ],
        1,
    )
}

fn listening(s: &mut SettingsScreen, row: usize) {
    while s.focus() < row {
        input(s, SettingsInput::Down);
    }
    assert_eq!(input(s, SettingsInput::Activate), None);
    assert!(s.is_listening());
}

#[test]
fn activating_a_key_row_starts_listening() {
    let mut s = keys_screen();
    input(&mut s, SettingsInput::Down);
    assert!(!s.is_listening());
    assert_eq!(input(&mut s, SettingsInput::Activate), None);
    assert!(s.is_listening());
    assert!(s.snapshot().widgets[1] != s.snapshot().widgets[2]);
}

#[test]
fn a_press_on_a_key_row_starts_listening() {
    let mut s = keys_screen();
    let l = layout();
    let r = l.widget(s.slot(2));
    let (x, y) = (r.x + 4.0, r.y + 4.0);
    assert_eq!(s.input(SettingsInput::PointerPress { x, y }, &l), None);
    assert!(s.is_listening());
    assert_eq!(s.focus(), 2);
    // The next press arrives as a capture, whatever button it is.
    assert_eq!(
        input(
            &mut s,
            SettingsInput::Captured(KeyValue::Mouse(MouseKey::Left))
        ),
        Some(SettingsAction::KeyCaptured {
            row: 2,
            key: KeyValue::Mouse(MouseKey::Left)
        })
    );
    assert!(!s.is_listening());
}

#[test]
fn a_captured_key_is_reported_and_listening_ends() {
    let mut s = keys_screen();
    listening(&mut s, 2);
    assert_eq!(
        input(&mut s, SettingsInput::Captured(X)),
        Some(SettingsAction::KeyCaptured { row: 2, key: X })
    );
    assert!(!s.is_listening());
    // The application decides the value.
    assert_eq!(
        s.widget(2),
        Some(&RowWidget::Key(KeyRow {
            bind: KeyBind::new(),
            value: KeyValue::Empty
        }))
    );
    s.set_key(2, X);
    assert!(matches!(s.widget(2), Some(RowWidget::Key(k)) if k.value() == X));
}

#[test]
fn escape_stops_listening_without_closing() {
    let mut s = keys_screen();
    listening(&mut s, 1);
    assert_eq!(input(&mut s, SettingsInput::Back), None);
    assert!(!s.is_listening());
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn delete_clears_the_listening_slot() {
    let mut s = keys_screen();
    listening(&mut s, 1);
    assert_eq!(
        input(&mut s, SettingsInput::Clear),
        Some(SettingsAction::KeyCleared { row: 1 })
    );
    assert!(!s.is_listening());
    // Not listening: nothing to clear.
    assert_eq!(input(&mut s, SettingsInput::Clear), None);
}

#[test]
fn an_empty_capture_and_other_input_while_listening_do_nothing() {
    let mut s = keys_screen();
    listening(&mut s, 1);
    for i in [
        SettingsInput::Captured(KeyValue::Empty),
        SettingsInput::Down,
        SettingsInput::NextSection,
        SettingsInput::Activate,
    ] {
        assert_eq!(input(&mut s, i), None);
        assert!(s.is_listening());
        assert_eq!(s.focus(), 1);
    }
}

#[test]
fn a_button_row_asks_for_its_action() {
    let mut s = keys_screen();
    while s.focus() < 3 {
        input(&mut s, SettingsInput::Down);
    }
    assert_eq!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::Pressed { row: 3 })
    );
    let l = layout();
    let r = l.row(s.slot(3));
    assert_eq!(
        s.input(
            SettingsInput::PointerPress {
                x: r.x + 5.0,
                y: r.y + 5.0
            },
            &l
        ),
        Some(SettingsAction::Pressed { row: 3 })
    );
}

#[test]
fn navigation_passes_over_the_new_rows() {
    let mut s = keys_screen();
    for expected in [1, 2, 3, 3] {
        input(&mut s, SettingsInput::Down);
        assert_eq!(s.focus(), expected);
    }
    // Left and right do nothing on a key row or a button.
    assert_eq!(input(&mut s, SettingsInput::Left), None);
    input(&mut s, SettingsInput::Up);
    assert_eq!(input(&mut s, SettingsInput::Right), None);
}

fn asked() -> SettingsScreen {
    let mut s = keys_screen();
    listening(&mut s, 2);
    input(&mut s, SettingsInput::Captured(Z));
    s.ask_conflict(2, Z, Some(1));
    s
}

#[test]
fn the_conflict_is_in_the_snapshot_with_keep_chosen() {
    let s = asked();
    let c = s.snapshot().conflict.unwrap();
    assert_eq!((c.row, c.key, c.other, c.apply), (2, Z, Some(1), false));
}

#[test]
fn activating_the_conflict_keeps_by_default() {
    let mut s = asked();
    assert_eq!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::ConflictResolved {
            row: 2,
            key: Z,
            apply: false
        })
    );
    assert!(s.conflict().is_none());
}

#[test]
fn right_chooses_apply_and_left_goes_back() {
    let mut s = asked();
    assert_eq!(input(&mut s, SettingsInput::Right), None);
    assert!(s.conflict().unwrap().apply);
    assert_eq!(input(&mut s, SettingsInput::Left), None);
    assert!(!s.conflict().unwrap().apply);
    input(&mut s, SettingsInput::Right);
    assert_eq!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::ConflictResolved {
            row: 2,
            key: Z,
            apply: true
        })
    );
}

#[test]
fn escape_keeps_the_old_binding_and_does_not_close() {
    let mut s = asked();
    input(&mut s, SettingsInput::Right);
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::ConflictResolved {
            row: 2,
            key: Z,
            apply: false
        })
    );
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn the_conflict_buttons_answer_to_the_pointer() {
    let l = layout();
    let [keep, apply] = l.conflict_buttons();
    let mut s = asked();
    s.input(
        SettingsInput::PointerMove {
            x: apply.x + 2.0,
            y: apply.y + 2.0,
        },
        &l,
    );
    assert!(s.conflict().unwrap().apply);
    // A press elsewhere does nothing, the screen stays modal.
    assert_eq!(
        s.input(SettingsInput::PointerPress { x: 0.0, y: 0.0 }, &l),
        None
    );
    assert!(s.conflict().is_some());
    assert_eq!(
        s.input(
            SettingsInput::PointerPress {
                x: keep.x + 2.0,
                y: keep.y + 2.0
            },
            &l
        ),
        Some(SettingsAction::ConflictResolved {
            row: 2,
            key: Z,
            apply: false
        })
    );
    let mut s = asked();
    assert_eq!(
        s.input(
            SettingsInput::PointerPress {
                x: apply.x + 2.0,
                y: apply.y + 2.0
            },
            &l
        ),
        Some(SettingsAction::ConflictResolved {
            row: 2,
            key: Z,
            apply: true
        })
    );
}

#[test]
fn the_conflict_blocks_navigation() {
    let mut s = asked();
    for i in [
        SettingsInput::Down,
        SettingsInput::NextSection,
        SettingsInput::Captured(X),
        SettingsInput::Clear,
    ] {
        assert_eq!(input(&mut s, i), None);
    }
    assert_eq!(s.focus(), 2);
    assert!(s.conflict().is_some());
}

#[test]
fn the_wheel_scrolls_an_open_long_dropdown_and_is_ignored_otherwise() {
    let mut s = SettingsScreen::new(&[slider(0, 1.0), dropdown(0, 30, 0)], 1);
    assert_eq!(input(&mut s, SettingsInput::Scroll { lines: 2.0 }), None);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Activate);
    input(&mut s, SettingsInput::Scroll { lines: 4.0 });
    let RowWidget::Dropdown(d) = s.snapshot().widgets[1] else {
        panic!("a dropdown");
    };
    assert_eq!(d.scroll(), 4);
    // Keys keep the highlight in view.
    for _ in 0..20 {
        input(&mut s, SettingsInput::Down);
    }
    let RowWidget::Dropdown(d) = s.snapshot().widgets[1] else {
        panic!("a dropdown");
    };
    assert_eq!(d.highlight(), 20);
    assert!(d.scroll() <= 20 && 20 < d.scroll() + crate::widgets::MAX_VISIBLE_OPTIONS);
}

#[test]
fn a_toggle_changed_from_outside_shows_without_an_action() {
    let mut s = screen();
    let row = (0..s.len)
        .find(|&r| matches!(s.widget(r), Some(RowWidget::Toggle(_))))
        .unwrap();
    let before = matches!(s.widget(row), Some(RowWidget::Toggle(t)) if t.value());
    s.set_toggle(row, !before);
    assert!(matches!(s.widget(row), Some(RowWidget::Toggle(t)) if t.value() == !before));
    // Other rows are left alone.
    let other = (0..s.len)
        .find(|&r| !matches!(s.widget(r), Some(RowWidget::Toggle(_))))
        .unwrap();
    let widget = *s.widget(other).unwrap();
    s.set_toggle(other, true);
    assert_eq!(*s.widget(other).unwrap(), widget);
}

#[test]
fn the_value_text_is_stored_and_snapshotted() {
    let mut s = SettingsScreen::new(&[button(0)], 1);
    assert_eq!(s.value_text(), "");
    let text: Arc<str> = Arc::from("/songs");
    s.set_value_text(Arc::clone(&text));
    assert_eq!(s.value_text(), "/songs");
    assert!(Arc::ptr_eq(&s.snapshot().value_text, &text));
}

#[test]
fn set_dropdown_changes_only_a_dropdown_row() {
    let mut s = screen();
    s.set_dropdown(2, 7, 5);
    let Some(RowWidget::Dropdown(d)) = s.widget(2) else {
        panic!("dropdown");
    };
    assert_eq!((d.count(), d.selected()), (7, 5));
    // A selection past the end is clamped; other rows are left alone.
    s.set_dropdown(2, 2, 9);
    let Some(RowWidget::Dropdown(d)) = s.widget(2) else {
        panic!("dropdown");
    };
    assert_eq!((d.count(), d.selected()), (2, 1));
    s.set_dropdown(1, 3, 0);
    assert!(matches!(s.widget(1), Some(RowWidget::Toggle(_))));
}

#[test]
fn swap_dropdown_swaps_only_a_dropdown_row_view() {
    use crate::widgets::DropdownView;
    use slam_text::FontRegistry;
    let registry = Arc::new(FontRegistry::with_builtin());
    let rows = [toggle(0, false), dropdown(0, 2, 0)];
    let labels = SettingsLabels {
        sections: vec!["A".to_owned()],
        rows: vec![
            RowLabels::default(),
            RowLabels {
                options: vec!["a".to_owned(), "b".to_owned()],
                ..RowLabels::default()
            },
        ],
        ..SettingsLabels::default()
    };
    let mut view = SettingsView::new(Arc::clone(&registry), theme(), &rows, &labels);
    let mut new = DropdownView::new(Arc::clone(&registry), theme(), &["x", "y", "z"]);
    assert!(view.swap_dropdown(1, &mut new));
    // `new` now holds the view built from the labels: two options.
    assert!(format!("{new:?}").contains("options: 2"));
    assert!(!view.swap_dropdown(0, &mut new));
    assert!(format!("{new:?}").contains("options: 2"));
}

#[test]
fn a_disabled_slider_or_toggle_ignores_input() {
    let mut s = screen();
    // Row 0 is the slider, row 1 the toggle.
    s.set_disabled(0, true);
    assert!(s.is_disabled(0));
    assert_eq!(input(&mut s, SettingsInput::Right), None);
    let l = layout();
    let slot = s.slot(0);
    let (x, y) = (
        l.widget(slot).x + 5.0,
        l.widget(slot).y + l.widget(slot).height / 2.0,
    );
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert_eq!(input(&mut s, SettingsInput::PointerRelease), None);
    assert!(
        matches!(s.widget(0), Some(RowWidget::Slider(w)) if w.value() == 50.0 && !w.is_dragging())
    );
    // The focus still moves over it, and it works again once enabled.
    input(&mut s, SettingsInput::Down);
    s.set_disabled(1, true);
    assert_eq!(input(&mut s, SettingsInput::Activate), None);
    s.set_disabled(1, false);
    assert!(matches!(
        input(&mut s, SettingsInput::Activate),
        Some(SettingsAction::Changed {
            row: 1,
            value: RowValue::Toggle(true)
        })
    ));
    s.set_disabled(0, false);
    input(&mut s, SettingsInput::Up);
    assert!(matches!(
        input(&mut s, SettingsInput::Right),
        Some(SettingsAction::Changed { row: 0, .. })
    ));
}

#[test]
fn disabled_and_hint_flags_reach_the_snapshot_and_ignore_unknown_rows() {
    let mut s = screen();
    s.set_disabled(2, true);
    s.set_hint(2, true);
    s.set_disabled(99, true);
    s.set_hint(99, true);
    let snapshot = s.snapshot();
    assert!(snapshot.disabled[2] && snapshot.hinted[2]);
    assert!(!snapshot.disabled[0] && !snapshot.hinted[0]);
    assert!(!s.is_disabled(99) && !s.is_hinted(99));
    assert!(s.is_hinted(2));
    s.set_hint(2, false);
    assert!(!s.snapshot().hinted[2]);
}
