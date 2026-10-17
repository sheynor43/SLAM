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
