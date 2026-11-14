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
    // Headers are lines of the one list: header, three rows, header, one row.
    assert_eq!((s.header(0), s.header(1)), (Some(0), Some(4)));
    assert_eq!((s.slot(0), s.slot(2), s.slot(3)), (1, 3, 5));
    assert_eq!(s.lines(), 6);
}

#[test]
fn up_and_down_walk_the_one_list_across_sections() {
    let mut s = screen();
    assert_eq!(input(&mut s, SettingsInput::Up), None);
    assert_eq!(s.focus(), 0);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    // The second section's row follows the first's, past its header.
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 3);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 3);
    input(&mut s, SettingsInput::Up);
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
    let (x, y) = centre(layout().row(s.slot(1)));
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
    let widget = SliderLayout::new(l.widget(s.slot(0)), &theme(), l.scale);
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
    let slot = s.slot(2);
    let widget = l.widget(slot);
    let (x, y) = centre(widget);
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if d.is_open()));
    let option = l.dropdown(slot, 4).option(3);
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
    let (x, y) = centre(layout().row(s.slot(2)));
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

#[test]
fn a_hidden_row_is_not_hit_and_the_rows_below_move_up() {
    let mut s = screen();
    let l = layout();
    let old_toggle = centre(l.row(s.slot(1)));
    s.set_hidden(0, true);
    assert!(s.is_hidden(0) && !s.is_hidden(1));
    assert_eq!(s.slot(1), 1);
    assert_eq!(s.slot(2), 2);
    // The toggle now sits where the slider was; the old place holds the dropdown row.
    let first = centre(l.row(1));
    assert!(matches!(
        input(
            &mut s,
            SettingsInput::PointerPress {
                x: first.0,
                y: first.1
            }
        ),
        Some(SettingsAction::Changed {
            row: 1,
            value: RowValue::Toggle(true)
        })
    ));
    assert_eq!(s.focus(), 1);
    input(
        &mut s,
        SettingsInput::PointerPress {
            x: old_toggle.0,
            y: old_toggle.1,
        },
    );
    assert_eq!(s.focus(), 2);
    // The line below the section's last row is the next header, which takes no press.
    let past = centre(l.row(3));
    input(
        &mut s,
        SettingsInput::PointerPress {
            x: past.0,
            y: past.1,
        },
    );
    assert_eq!(s.focus(), 2);
    let snapshot = s.snapshot();
    assert!(snapshot.hidden[0] && !snapshot.hidden[1]);
    assert_eq!(snapshot.slots[1], 1);
}

#[test]
fn navigation_skips_hidden_rows() {
    let mut s = screen();
    s.set_hidden(1, true);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    input(&mut s, SettingsInput::Up);
    assert_eq!(s.focus(), 0);
    // Out of range rows are ignored.
    s.set_hidden(99, true);
    assert!(!s.is_hidden(99));
}

#[test]
fn the_focus_leaves_a_row_that_becomes_hidden() {
    let mut s = screen();
    input(&mut s, SettingsInput::Down);
    s.set_hidden(1, true);
    assert_eq!(s.focus(), 2);
    // Nothing below in the section: the next row of the list is the second section's.
    s.set_hidden(2, true);
    assert_eq!(s.focus(), 3);
    s.set_hidden(3, true);
    assert_eq!(s.focus(), 0);
}

#[test]
fn hiding_an_open_dropdown_closes_it_without_an_action() {
    let mut s = screen();
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Activate);
    assert!(matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if d.is_open()));
    input(&mut s, SettingsInput::Down);
    s.set_hidden(2, true);
    assert!(
        matches!(s.widget(2), Some(RowWidget::Dropdown(d)) if !d.is_open() && d.selected() == 1)
    );
    assert_eq!(s.focus(), 3);
    // Back now closes the screen, not a dropdown.
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn hiding_a_listening_key_row_stops_listening() {
    let mut s = keys_screen();
    listening(&mut s, 1);
    assert!(s.is_listening());
    s.set_hidden(1, true);
    assert!(!s.is_listening());
}

#[test]
fn showing_a_row_again_restores_its_state() {
    let mut s = screen();
    input(&mut s, SettingsInput::Right);
    s.set_disabled(0, true);
    s.set_hint(0, true);
    s.set_hidden(0, true);
    assert_eq!(input(&mut s, SettingsInput::Right), None);
    s.set_hidden(0, false);
    assert!(s.is_disabled(0) && s.is_hinted(0) && !s.is_hidden(0));
    assert_eq!(s.slot(0), 1);
    assert_eq!(s.slot(1), 2);
    assert!(matches!(s.widget(0), Some(RowWidget::Slider(w)) if w.value() > 50.0));
}

#[test]
fn hidden_rows_shrink_the_section() {
    let mut s = screen();
    s.set_hidden(0, true);
    s.set_hidden(1, true);
    // One visible row in section 0 is left, under its header; section 1 follows it.
    assert_eq!(s.slot(2), 1);
    assert_eq!(s.header(1), Some(2));
    assert_eq!(s.slot(3), 3);
    let visible = s.snapshot().hidden[..3].iter().filter(|h| !**h).count();
    assert_eq!(visible, 1);
}

fn note(section: usize) -> RowSpec {
    RowSpec {
        section,
        kind: RowKind::Note,
        restart_required: false,
    }
}

/// Section 0: toggle, note, toggle. Section 1: note only. Section 2: toggle.
fn notes_screen() -> SettingsScreen {
    SettingsScreen::new(
        &[
            toggle(0, false),
            note(0),
            toggle(0, false),
            note(1),
            toggle(2, false),
        ],
        3,
    )
}

#[test]
fn a_note_row_is_skipped_by_navigation_and_never_focused() {
    let mut s = notes_screen();
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    input(&mut s, SettingsInput::Up);
    assert_eq!(s.focus(), 0);
    // A section of notes only leaves the focus where it was; nothing is emitted.
    input(&mut s, SettingsInput::NextSection);
    assert_eq!(s.section(), 1);
    assert_eq!(s.focus(), 0);
}

#[test]
fn a_note_row_is_not_hit_and_gives_no_action() {
    let mut s = notes_screen();
    let l = layout();
    let (x, y) = centre(l.row(s.slot(1)));
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert_eq!(s.focus(), 0);
    input(&mut s, SettingsInput::PointerMove { x, y });
    assert_eq!(s.focus(), 0);
    assert!(matches!(s.widget(1), Some(RowWidget::Note)));
    let mut s = notes_screen();
    s.focus = 1;
    for i in [
        SettingsInput::Activate,
        SettingsInput::Left,
        SettingsInput::Right,
    ] {
        assert_eq!(input(&mut s, i), None);
    }
}

#[test]
fn hiding_the_focused_row_never_moves_the_focus_to_a_note() {
    let mut s = notes_screen();
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 2);
    // Nothing below in the section: the focus goes on to the next row of the list, passing
    // the note of section 1.
    s.set_hidden(2, true);
    assert_eq!(s.focus(), 4);
}

#[test]
fn a_note_row_can_be_hidden_and_shown() {
    let mut s = notes_screen();
    s.set_hidden(1, true);
    assert!(s.is_hidden(1));
    assert_eq!(s.slot(2), 2);
    s.set_hidden(1, false);
    assert!(!s.is_hidden(1));
    assert_eq!(s.slot(2), 3);
}

#[test]
fn hiding_a_slider_while_dragging_ends_the_drag() {
    let mut s = screen();
    let l = layout();
    let widget = SliderLayout::new(l.widget(s.slot(0)), &theme(), l.scale);
    let y = widget.hit.y + 2.0;
    let at = |fraction: f32| widget.track.x + widget.track.width * fraction;
    input(&mut s, SettingsInput::PointerPress { x: at(0.25), y });
    let Some(RowWidget::Slider(sl)) = s.widget(0) else {
        panic!()
    };
    assert!(sl.is_dragging());
    s.set_hidden(0, true);
    let Some(RowWidget::Slider(sl)) = s.widget(0) else {
        panic!()
    };
    assert!(!sl.is_dragging());
    assert_eq!(
        input(&mut s, SettingsInput::PointerMove { x: at(0.75), y }),
        None
    );
    assert_eq!(input(&mut s, SettingsInput::PointerRelease), None);
    s.set_hidden(0, false);
    let Some(RowWidget::Slider(sl)) = s.widget(0) else {
        panic!()
    };
    assert_eq!(sl.value(), 25.0);
}

#[test]
fn a_section_without_rows_has_no_header_and_is_skipped() {
    let mut s = screen();
    // Hide the only row of section 1.
    s.set_hidden(3, true);
    assert_eq!((s.header(0), s.header(1)), (Some(0), None));
    assert_eq!(s.lines(), 4);
    input(&mut s, SettingsInput::NextSection);
    assert_eq!(s.section(), 0);
    assert_eq!(s.focus(), 0);
    // Clicking its tab does nothing either.
    let (x, y) = centre(layout().tabs[1]);
    assert_eq!(input(&mut s, SettingsInput::PointerPress { x, y }), None);
    assert_eq!(s.section(), 0);
    // Back still closes.
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn an_open_list_covers_the_rows_on_the_side_it_opens() {
    let l = layout();
    let count = 6;
    // The first row's list opens below; a row low enough opens it above.
    assert!(l.dropdown_at(0, count).list().y > l.row(0).y);
    let low = (1..64)
        .find(|slot| l.dropdown_at(*slot, count).list().y < l.row(*slot).y)
        .expect("a low row opens its list above");
    assert!(l.covered_by_list(0, count, 1));
    assert!(!l.covered_by_list(0, count, 0), "not its own row");
    assert!(l.covered_by_list(low, count, low - 1));
    assert!(!l.covered_by_list(low, count, low + 1));
    assert!(!l.covered_by_list(low, count, low));
    // Rows past the list stay drawn.
    assert!(!l.covered_by_list(0, count, 40));
}

fn texts(list: &[&str]) -> Arc<[Box<str>]> {
    list.iter().map(|t| Box::<str>::from(*t)).collect()
}

fn type_str(s: &mut SettingsScreen, text: &str) {
    for c in text.chars() {
        input(s, SettingsInput::Type(c));
    }
}

/// Section 0: master volume, music volume, hit lighting. Section 1: effect volume, skin.
fn searchable() -> SettingsScreen {
    let mut s = SettingsScreen::new(
        &[
            slider(0, 50.0),
            slider(0, 60.0),
            toggle(0, false),
            slider(1, 70.0),
            slider(1, 80.0),
        ],
        2,
    );
    s.set_search_texts(texts(&[
        "master volume audio",
        "music volume audio",
        "hit lighting gameplay",
        "effect volume sound",
        "skin sound",
    ]));
    s
}

#[test]
fn search_lists_matches_of_all_sections_with_global_slots() {
    let mut s = searchable();
    assert!(!s.searching());
    type_str(&mut s, "volume");
    assert!(s.searching());
    assert_eq!(s.search().as_str(), "volume");
    let snap = s.snapshot();
    assert_eq!(snap.hidden[..5], [false, false, true, false, true]);
    // Header, two results, the header of the second section, its result.
    assert_eq!((s.slot(0), s.slot(1), s.slot(3)), (1, 2, 4));
    assert_eq!(snap.headers[..2], [Some(0), Some(3)]);
    assert!(snap.search.as_str() == "volume");
    // The focus is on the first result and moves over the results of both sections.
    assert_eq!(s.focus(), 0);
    input(&mut s, SettingsInput::Down);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 3);
    input(&mut s, SettingsInput::Down);
    assert_eq!(s.focus(), 3);
    // A second word narrows it: every word must match.
    type_str(&mut s, " music");
    let snap = s.snapshot();
    assert_eq!(snap.hidden[..5], [true, false, true, true, true]);
    assert_eq!(s.focus(), 1);
    // Erasing widens it again.
    for _ in 0..6 {
        input(&mut s, SettingsInput::Erase);
    }
    assert_eq!(s.search().as_str(), "volume");
    assert_eq!(s.snapshot().hidden[..5], [false, false, true, false, true]);
}

#[test]
fn search_is_by_substring_and_a_leading_space_is_ignored() {
    let mut s = searchable();
    input(&mut s, SettingsInput::Type(' '));
    assert!(!s.searching());
    type_str(&mut s, "ght");
    assert_eq!(s.snapshot().hidden[..5], [true, true, false, true, true]);
    type_str(&mut s, "zzz");
    assert_eq!(s.snapshot().hidden[..5], [true; 5]);
}

#[test]
fn back_clears_the_search_before_it_closes() {
    let mut s = searchable();
    type_str(&mut s, "sk");
    assert_eq!(input(&mut s, SettingsInput::Back), None);
    assert!(!s.searching());
    assert!(s.snapshot().hidden[..5].iter().all(|h| !h));
    assert_eq!(
        input(&mut s, SettingsInput::Back),
        Some(SettingsAction::Close)
    );
}

#[test]
fn switching_section_clears_the_search() {
    let mut s = searchable();
    type_str(&mut s, "volume");
    input(&mut s, SettingsInput::NextSection);
    assert!(!s.searching());
    assert_eq!(s.search().as_str(), "");
    assert_eq!(s.section(), 1);
}

/// 30 sliders in one section, in a window that fits only some of them.
fn long_screen() -> (SettingsScreen, SettingsLayout) {
    let rows: Vec<RowSpec> = (0..30).map(|i| slider(0, f64::from(i))).collect();
    let layout = SettingsLayout::new(1280.0, 500.0, &theme());
    assert!(
        layout.visible_rows() >= 3.0 && layout.visible_rows() < 20.0,
        "{}",
        layout.visible_rows()
    );
    (SettingsScreen::new(&rows, 1), layout)
}

#[test]
fn moving_the_focus_past_the_bottom_scrolls_to_keep_it_visible() {
    let (mut s, l) = long_screen();
    let visible = l.visible_rows().floor() as usize;
    assert_eq!(s.scroll(), 0.0);
    // The header takes the first line: the rows under it fit `visible - 1` lines.
    for _ in 0..visible - 2 {
        s.input(SettingsInput::Down, &l);
    }
    assert_eq!(s.scroll(), 0.0);
    s.input(SettingsInput::Down, &l);
    assert_eq!(s.focus(), visible - 1);
    assert_eq!(s.scroll(), 1.0);
    s.tick(1_000_000_000, &l);
    assert!(l.scrolled(s.shown_scroll()).shows(s.slot(s.focus())));
    for _ in 0..30 {
        s.input(SettingsInput::Down, &l);
    }
    assert_eq!(s.focus(), 29);
    s.tick(2_000_000_000, &l);
    assert!(l.scrolled(s.shown_scroll()).shows(s.slot(29)));
    // And back up, to the header.
    for _ in 0..30 {
        s.input(SettingsInput::Up, &l);
    }
    assert_eq!((s.focus(), s.scroll()), (0, 0.0));
}

#[test]
fn the_wheel_scrolls_the_rows_and_is_clamped() {
    let (mut s, l) = long_screen();
    s.input(SettingsInput::Scroll { lines: 3.0 }, &l);
    assert_eq!(s.scroll(), 3.0);
    s.input(SettingsInput::Scroll { lines: -10.0 }, &l);
    assert_eq!(s.scroll(), 0.0);
    s.input(SettingsInput::Scroll { lines: 1000.0 }, &l);
    // 30 rows and the header.
    let max = 31.0 - l.visible_rows();
    assert_eq!(s.scroll(), max);
    // The last row is shown whole at the end.
    assert!(l.scrolled(s.scroll()).shows(30));
    // A short list does not scroll.
    let mut short = screen();
    short.input(SettingsInput::Scroll { lines: 5.0 }, &layout());
    assert_eq!(short.scroll(), 0.0);
}

#[test]
fn the_wheel_scrolls_an_open_dropdown_not_the_rows() {
    let rows: Vec<RowSpec> = std::iter::once(dropdown(0, 40, 0))
        .chain((0..19).map(|i| slider(0, f64::from(i))))
        .collect();
    let l = SettingsLayout::new(1280.0, 500.0, &theme());
    let mut s = SettingsScreen::new(&rows, 1);
    s.input(SettingsInput::Activate, &l);
    s.input(SettingsInput::Scroll { lines: 4.0 }, &l);
    assert_eq!(s.scroll(), 0.0);
}

#[test]
fn pointing_respects_the_scroll() {
    let (mut s, l) = long_screen();
    s.input(SettingsInput::Scroll { lines: 5.0 }, &l);
    s.tick(1_000_000_000, &l);
    let scrolled = l.scrolled(s.shown_scroll());
    // Row 2 (line 3) is above the content now: not found, wherever it lies.
    assert_eq!(s.row_at(&scrolled, 0.0, 0.0), None);
    let (x, y) = centre(scrolled.row(s.slot(2)));
    assert_eq!(s.row_at(&scrolled, x, y), None);
    let (x, y) = centre(scrolled.row(s.slot(5)));
    assert_eq!(s.row_at(&scrolled, x, y), Some(5));
    let (x, y) = centre(scrolled.row(s.slot(6)));
    assert_eq!(s.row_at(&scrolled, x, y), Some(6));
    // A line past the content area is not pointed at either.
    let below = (6..31).find(|slot| !scrolled.shows(*slot)).unwrap();
    let (x, y) = centre(scrolled.row(below));
    assert_eq!(s.row_at(&scrolled, x, y), None);
    // A press at line 0's place focuses the line there now (5, row 4), not row 0.
    let (x, y) = centre(l.row(0));
    s.input(SettingsInput::PointerPress { x, y }, &l);
    assert_eq!(s.focus(), 4);
}

/// Three sections of 20 sliders each in a window that fits only some of them: 63 lines.
fn sectioned_screen() -> (SettingsScreen, SettingsLayout) {
    let rows: Vec<RowSpec> = (0..60_u32)
        .map(|i| slider((i / 20) as usize, f64::from(i)))
        .collect();
    let layout = SettingsLayout::new(1280.0, 500.0, &theme());
    let mut s = SettingsScreen::new(&rows, 3);
    s.tick(0, &layout);
    s.set_search_texts(texts(
        &(0..60)
            .map(|i| if i == 45 { "needle" } else { "hay" })
            .collect::<Vec<_>>(),
    ));
    (s, layout)
}

/// Lets the eased scroll settle.
fn settle(s: &mut SettingsScreen, l: &SettingsLayout) {
    for step in 1..=200_u64 {
        s.tick(step * 16_000_000, l);
    }
}

#[test]
fn all_sections_are_one_list_with_a_header_each() {
    let (s, _) = sectioned_screen();
    assert_eq!(s.lines(), 63);
    assert_eq!(
        (s.header(0), s.header(1), s.header(2)),
        (Some(0), Some(21), Some(42))
    );
    assert_eq!(s.slot(0), 1);
    assert_eq!(s.slot(20), 22);
    assert_eq!(s.slot(59), 62);
    let snap = s.snapshot();
    assert_eq!(snap.headers[..3], [Some(0), Some(21), Some(42)]);
}

#[test]
fn a_click_on_a_category_scrolls_to_its_header_and_the_sidebar_follows() {
    let (mut s, l) = sectioned_screen();
    let (x, y) = centre(l.tabs[1]);
    assert_eq!(s.input(SettingsInput::PointerPress { x, y }, &l), None);
    // The target is the header; nothing has moved yet.
    assert_eq!(s.scroll(), 21.0);
    assert_eq!(s.shown_scroll(), 0.0);
    assert_eq!(s.section(), 1);
    assert_eq!(s.focus(), 20);
    settle(&mut s, &l);
    assert_eq!(s.shown_scroll(), 21.0);
    assert_eq!(s.section(), 1);
    // The last section cannot reach the top of a short window: it is the one highlighted.
    let (x, y) = centre(l.tabs[2]);
    s.input(SettingsInput::PointerPress { x, y }, &l);
    assert_eq!(s.section(), 2);
    assert!(s.scroll() <= 42.0);
    settle(&mut s, &l);
    assert_eq!(s.section(), 2);
}

#[test]
fn the_sidebar_highlights_the_section_at_the_top_of_a_scrolled_list() {
    let (mut s, l) = sectioned_screen();
    assert_eq!(s.section(), 0);
    s.input(SettingsInput::Scroll { lines: 22.0 }, &l);
    settle(&mut s, &l);
    assert_eq!(s.section(), 1);
    s.input(SettingsInput::Scroll { lines: 20.0 }, &l);
    settle(&mut s, &l);
    assert_eq!(s.section(), 2);
    s.input(SettingsInput::Scroll { lines: -1000.0 }, &l);
    settle(&mut s, &l);
    assert_eq!(s.section(), 0);
    // Halfway through the eased scroll the highlight is where the list is.
    s.input(SettingsInput::Scroll { lines: 30.0 }, &l);
    assert_eq!(s.section(), 0);
    settle(&mut s, &l);
    assert_eq!(s.section(), 1);
}

#[test]
fn a_picked_section_stays_highlighted_until_the_list_is_scrolled() {
    let (mut s, l) = sectioned_screen();
    let (x, y) = centre(l.tabs[2]);
    s.input(SettingsInput::PointerPress { x, y }, &l);
    settle(&mut s, &l);
    s.input(SettingsInput::Scroll { lines: -15.0 }, &l);
    settle(&mut s, &l);
    assert_eq!(s.section(), 1);
}

#[test]
fn the_scroll_eases_to_its_target_and_lands() {
    let (mut s, l) = sectioned_screen();
    s.input(SettingsInput::Scroll { lines: 10.0 }, &l);
    assert_eq!((s.scroll(), s.shown_scroll()), (10.0, 0.0));
    // One frame moves it part of the way, never past the target.
    s.tick(16_000_000, &l);
    assert!(s.shown_scroll() > 0.0 && s.shown_scroll() < 10.0);
    let mut last = s.shown_scroll();
    for step in 2..=100_u64 {
        s.tick(step * 16_000_000, &l);
        assert!(s.shown_scroll() >= last && s.shown_scroll() <= 10.0);
        last = s.shown_scroll();
    }
    assert_eq!(s.shown_scroll(), 10.0);
    // The snapshot draws the eased scroll.
    assert_eq!(s.snapshot().scroll, 10.0);
}

#[test]
fn the_keyboard_scrolls_the_list_and_shows_a_sections_header_with_its_first_row() {
    let (mut s, l) = sectioned_screen();
    // Down to the last row of section 0, then the first of section 1.
    for _ in 0..20 {
        s.input(SettingsInput::Down, &l);
    }
    assert_eq!(s.focus(), 20);
    // Its header is the line above the first row: it scrolls in with it.
    let visible = l.visible_rows().floor();
    assert!(s.scroll() <= 21.0 && s.scroll() + visible > 21.0);
    // Page down goes to the header of the section after the one at the top of the list.
    s.input(SettingsInput::NextSection, &l);
    assert_eq!((s.scroll(), s.section()), (21.0, 1));
    s.input(SettingsInput::NextSection, &l);
    assert_eq!(s.scroll(), 42.0_f32.min(s.max_scroll()));
    assert_eq!(s.section(), 2);
}

#[test]
fn search_scrolls_to_the_first_match_hides_empty_headers_and_clearing_returns() {
    let (mut s, l) = sectioned_screen();
    s.input(SettingsInput::Scroll { lines: 30.0 }, &l);
    settle(&mut s, &l);
    type_str(&mut s, "needle");
    // One match, in section 2: only that section's header remains.
    let snap = s.snapshot();
    assert_eq!(snap.headers[..3], [None, None, Some(0)]);
    assert_eq!(s.lines(), 2);
    assert_eq!(s.slot(45), 1);
    assert_eq!(s.focus(), 45);
    // The target is the top of the results; the list glides there.
    assert_eq!(s.scroll(), 0.0);
    assert!(s.shown_scroll() > 0.0);
    settle(&mut s, &l);
    assert_eq!(s.shown_scroll(), 0.0);
    assert_eq!(s.section(), 2);
    // Clearing goes back to where the list was.
    s.input(SettingsInput::Back, &l);
    assert!(!s.searching());
    assert_eq!(s.scroll(), 30.0);
    assert_eq!(s.lines(), 63);
    settle(&mut s, &l);
    assert_eq!(s.shown_scroll(), 30.0);
    assert_eq!(s.section(), 1);
    // A match in a later section than the one at the top is reached from anywhere.
    s.input(SettingsInput::Scroll { lines: -30.0 }, &l);
    type_str(&mut s, "hay");
    assert_eq!(s.scroll(), 0.0);
    assert_eq!(s.snapshot().headers[..3], [Some(0), Some(21), Some(42)]);
}

#[test]
fn only_the_lines_touching_the_content_are_visible_slots() {
    let (mut s, l) = sectioned_screen();
    let range = l.visible_slots();
    assert_eq!(range.start, 0);
    assert!((range.end as f32) < l.visible_rows() + 3.0);
    s.input(SettingsInput::Scroll { lines: 10.5 }, &l);
    settle(&mut s, &l);
    let scrolled = l.scrolled(s.shown_scroll());
    let range = scrolled.visible_slots();
    assert_eq!(range.start, 10);
    assert!(range.end < 10 + 1 + l.visible_rows().ceil() as usize + 2);
    // A line partly out of the content shows in part; one far out, nothing.
    let half = scrolled.fraction(10);
    assert!(half > 0.0 && half < 1.0, "{half}");
    assert_eq!(scrolled.fraction(0), 0.0);
    assert_eq!(scrolled.fraction(62), 0.0);
    assert!((scrolled.fraction(12) - 1.0).abs() < 1e-4);
}
