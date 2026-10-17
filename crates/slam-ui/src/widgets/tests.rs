use super::*;

const BOUNDS: Rect = Rect {
    x: 100.0,
    y: 100.0,
    width: 400.0,
    height: 40.0,
};

fn theme() -> Theme {
    Theme::default()
}

use crate::theme::Theme;

mod toggle_tests {
    use super::*;

    fn layout() -> ToggleLayout {
        ToggleLayout::new(BOUNDS, &theme(), 1.0)
    }

    #[test]
    fn activate_and_press_flip_the_value() {
        let mut t = Toggle::new(false);
        assert_eq!(
            t.input(ToggleInput::Activate, &layout()),
            Some(ToggleAction::Changed(true))
        );
        let press = ToggleInput::PointerPress { x: 120.0, y: 110.0 };
        assert_eq!(
            t.input(press, &layout()),
            Some(ToggleAction::Changed(false))
        );
        assert!(!t.value());
    }

    #[test]
    fn press_outside_does_nothing() {
        let mut t = Toggle::new(true);
        let press = ToggleInput::PointerPress { x: 1.0, y: 1.0 };
        assert_eq!(t.input(press, &layout()), None);
        assert!(t.value());
    }

    #[test]
    fn track_sits_inside_bounds() {
        let l = layout();
        assert!(l.track.x >= BOUNDS.x && l.track.x + l.track.width <= BOUNDS.x + BOUNDS.width);
        assert!(l.track.y >= BOUNDS.y && l.track.y + l.track.height <= BOUNDS.y + BOUNDS.height);
    }
}

mod slider_tests {
    use super::*;

    fn layout() -> SliderLayout {
        SliderLayout::new(BOUNDS, &theme(), 1.0)
    }

    fn at(fraction: f32) -> (f32, f32) {
        let l = layout();
        (l.track.x + l.track.width * fraction, l.bounds.y + 5.0)
    }

    #[test]
    fn new_clamps_and_snaps() {
        assert_eq!(Slider::new(0.0, 10.0, 1.0, 99.0).value(), 10.0);
        assert_eq!(Slider::new(0.0, 10.0, 1.0, -5.0).value(), 0.0);
        assert_eq!(Slider::new(0.0, 10.0, 2.0, 5.2).value(), 6.0);
        assert_eq!(Slider::new(1.0, 10.0, 3.0, 3.0).value(), 4.0);
    }

    #[test]
    fn invalid_arguments_are_sanitised() {
        let s = Slider::new(5.0, 1.0, 1.0, 3.0);
        assert_eq!((s.min(), s.max(), s.value()), (5.0, 5.0, 5.0));
        assert_eq!(Slider::new(0.0, 4.0, 0.0, 0.0).step(), 4.0);
        assert_eq!(Slider::new(0.0, 4.0, f64::NAN, 0.0).step(), 4.0);
    }

    #[test]
    fn steps_stop_at_the_edges_without_actions() {
        let mut s = Slider::new(0.0, 2.0, 1.0, 1.0);
        let l = layout();
        assert_eq!(
            s.input(SliderInput::Increase, &l),
            Some(SliderAction::Changed(2.0))
        );
        assert_eq!(s.input(SliderInput::Increase, &l), None);
        s.input(SliderInput::Decrease, &l);
        s.input(SliderInput::Decrease, &l);
        assert_eq!(s.input(SliderInput::Decrease, &l), None);
        assert_eq!(s.value(), 0.0);
    }

    #[test]
    fn coarse_steps_move_ten_steps() {
        let mut s = Slider::new(0.0, 100.0, 1.0, 50.0);
        let l = layout();
        assert_eq!(
            s.input(SliderInput::IncreaseCoarse, &l),
            Some(SliderAction::Changed(60.0))
        );
        s.input(SliderInput::DecreaseCoarse, &l);
        s.input(SliderInput::DecreaseCoarse, &l);
        assert_eq!(s.value(), 40.0);
    }

    #[test]
    fn float_steps_do_not_drift() {
        let mut s = Slider::new(0.0, 1.0, 0.1, 0.0);
        let l = layout();
        for i in 1..=10 {
            s.input(SliderInput::Increase, &l);
            assert!(
                (s.value() - f64::from(i) * 0.1).abs() < 1e-12,
                "{i}: {}",
                s.value()
            );
        }
        assert_eq!(s.value(), 1.0);
        for _ in 0..10 {
            s.input(SliderInput::Decrease, &l);
        }
        assert!(s.value().abs() < 1e-12);
        for _ in 0..1000 {
            s.input(SliderInput::Increase, &l);
            s.input(SliderInput::Decrease, &l);
        }
        assert!(s.value().abs() < 1e-12);
    }

    #[test]
    fn pressing_the_track_jumps_and_drags() {
        let mut s = Slider::new(0.0, 100.0, 1.0, 0.0);
        let l = layout();
        let (x, y) = at(0.5);
        assert_eq!(
            s.input(SliderInput::PointerPress { x, y }, &l),
            Some(SliderAction::Changed(50.0))
        );
        assert!(s.is_dragging());
        let (x, y) = at(0.25);
        assert_eq!(
            s.input(SliderInput::PointerMove { x, y }, &l),
            Some(SliderAction::Changed(25.0))
        );
        // Dragging past either end clamps.
        assert_eq!(
            s.input(SliderInput::PointerMove { x: 5000.0, y }, &l),
            Some(SliderAction::Changed(100.0))
        );
        assert_eq!(
            s.input(SliderInput::PointerMove { x: -50.0, y }, &l),
            Some(SliderAction::Changed(0.0))
        );
        s.input(SliderInput::PointerRelease, &l);
        assert!(!s.is_dragging());
        let (x, y) = at(0.5);
        assert_eq!(s.input(SliderInput::PointerMove { x, y }, &l), None);
        assert_eq!(s.value(), 0.0);
    }

    #[test]
    fn no_action_when_the_snapped_value_is_unchanged() {
        let mut s = Slider::new(0.0, 10.0, 1.0, 5.0);
        let l = layout();
        let (x, y) = at(0.5);
        assert_eq!(s.input(SliderInput::PointerPress { x, y }, &l), None);
        assert!(s.is_dragging());
        // A tiny move stays in the same step.
        assert_eq!(
            s.input(SliderInput::PointerMove { x: x + 1.0, y }, &l),
            None
        );
    }

    #[test]
    fn press_off_the_track_does_not_drag() {
        let mut s = Slider::new(0.0, 10.0, 1.0, 5.0);
        let l = layout();
        let x = l.value.x + 1.0;
        assert_eq!(s.input(SliderInput::PointerPress { x, y: 110.0 }, &l), None);
        assert!(!s.is_dragging());
        assert_eq!(
            s.input(SliderInput::PointerPress { x: 1.0, y: 1.0 }, &l),
            None
        );
    }

    #[test]
    fn layout_keeps_parts_inside_bounds() {
        let l = layout();
        assert!(l.track.x + l.track.width <= l.value.x);
        assert!(l.value.x + l.value.width <= BOUNDS.x + BOUNDS.width);
        assert_eq!(s_fraction(), 0.5);
    }

    fn s_fraction() -> f32 {
        Slider::new(0.0, 10.0, 1.0, 5.0).fraction()
    }
}

mod dropdown_tests {
    use super::*;

    fn layout() -> DropdownLayout {
        DropdownLayout::new(BOUNDS, 3, 1.0)
    }

    fn centre(i: usize) -> (f32, f32) {
        let [x, y] = layout().option(i).centre();
        (x, y)
    }

    #[test]
    fn activate_opens_then_confirms_the_highlight() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        assert_eq!(d.input(DropdownInput::Activate, &l), None);
        assert!(d.is_open());
        assert_eq!(d.input(DropdownInput::Down, &l), None);
        assert_eq!(d.input(DropdownInput::Down, &l), None);
        assert_eq!(d.input(DropdownInput::Down, &l), None);
        assert_eq!(d.highlight(), 2);
        assert_eq!(
            d.input(DropdownInput::Activate, &l),
            Some(DropdownAction::Selected(2))
        );
        assert!(!d.is_open());
        assert_eq!(d.selected(), 2);
    }

    #[test]
    fn confirming_the_same_option_is_not_an_action() {
        let mut d = Dropdown::new(3, 1);
        let l = layout();
        d.input(DropdownInput::Activate, &l);
        assert_eq!(d.input(DropdownInput::Activate, &l), None);
        assert!(!d.is_open());
    }

    #[test]
    fn back_closes_without_change() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        d.input(DropdownInput::Activate, &l);
        d.input(DropdownInput::Down, &l);
        assert_eq!(d.input(DropdownInput::Back, &l), None);
        assert!(!d.is_open());
        assert_eq!((d.selected(), d.highlight()), (0, 0));
    }

    #[test]
    fn arrows_change_the_selection_directly_while_closed() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        assert_eq!(d.input(DropdownInput::Up, &l), None);
        assert_eq!(
            d.input(DropdownInput::Down, &l),
            Some(DropdownAction::Selected(1))
        );
        d.input(DropdownInput::Down, &l);
        assert_eq!(d.input(DropdownInput::Down, &l), None);
        assert_eq!(d.selected(), 2);
    }

    #[test]
    fn pointer_opens_hovers_and_picks() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        let (bx, by) = (BOUNDS.x + 5.0, BOUNDS.y + 5.0);
        assert_eq!(
            d.input(DropdownInput::PointerPress { x: bx, y: by }, &l),
            None
        );
        assert!(d.is_open());
        let (x, y) = centre(1);
        d.input(DropdownInput::PointerMove { x, y }, &l);
        assert_eq!(d.highlight(), 1);
        assert_eq!(
            d.input(DropdownInput::PointerPress { x, y }, &l),
            Some(DropdownAction::Selected(1))
        );
        assert!(!d.is_open());
    }

    #[test]
    fn press_outside_closes_without_change() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        d.input(DropdownInput::Activate, &l);
        assert_eq!(
            d.input(DropdownInput::PointerPress { x: 1.0, y: 1.0 }, &l),
            None
        );
        assert!(!d.is_open());
        assert_eq!(d.selected(), 0);
        // Closed: a press outside does not open it, hover does nothing.
        d.input(DropdownInput::PointerPress { x: 1.0, y: 1.0 }, &l);
        let (x, y) = centre(2);
        d.input(DropdownInput::PointerMove { x, y }, &l);
        assert!(!d.is_open());
        assert_eq!(d.highlight(), 0);
    }

    #[test]
    fn press_on_the_box_while_open_closes() {
        let mut d = Dropdown::new(3, 0);
        let l = layout();
        d.input(DropdownInput::Activate, &l);
        let press = DropdownInput::PointerPress {
            x: BOUNDS.x + 5.0,
            y: BOUNDS.y + 5.0,
        };
        assert_eq!(d.input(press, &l), None);
        assert!(!d.is_open());
    }

    #[test]
    fn empty_dropdown_never_opens() {
        let mut d = Dropdown::new(0, 5);
        let l = DropdownLayout::new(BOUNDS, 0, 1.0);
        assert_eq!(d.input(DropdownInput::Activate, &l), None);
        assert!(!d.is_open());
        assert_eq!(d.input(DropdownInput::Down, &l), None);
    }

    #[test]
    fn layout_hit_testing_matches_option_rects() {
        let l = layout();
        for i in 0..3 {
            let (x, y) = centre(i);
            assert_eq!(l.option_at(x, y), Some(i));
        }
        assert_eq!(l.option_at(BOUNDS.x + 5.0, BOUNDS.y + 5.0), None);
        let below = l.option(2);
        assert_eq!(
            l.option_at(below.x + 1.0, below.y + below.height + 1.0),
            None
        );
    }
}

mod keybind_tests {
    use super::*;

    fn layout() -> KeyBindLayout {
        KeyBindLayout::new(BOUNDS, 1.0)
    }

    type Input = KeyBindInput<u32>;

    #[test]
    fn activate_listens_and_capture_binds() {
        let mut k = KeyBind::new();
        let l = layout();
        assert_eq!(
            k.input(Input::Activate, &l),
            Some(KeyBindAction::StartedListening)
        );
        assert!(k.is_listening());
        assert_eq!(k.input(Input::Activate, &l), None);
        assert_eq!(
            k.input(Input::Captured(7), &l),
            Some(KeyBindAction::Bound(7))
        );
        assert!(!k.is_listening());
    }

    #[test]
    fn back_cancels_only_while_listening() {
        let mut k = KeyBind::new();
        let l = layout();
        assert_eq!(k.input(Input::Back, &l), None);
        k.input(Input::Activate, &l);
        assert_eq!(k.input(Input::Back, &l), Some(KeyBindAction::Cancelled));
        assert!(!k.is_listening());
    }

    #[test]
    fn capture_while_idle_is_ignored() {
        let mut k = KeyBind::new();
        assert_eq!(k.input(Input::Captured(1), &layout()), None);
    }

    #[test]
    fn pointer_starts_inside_and_cancels_outside() {
        let mut k = KeyBind::new();
        let l = layout();
        let outside = Input::PointerPress { x: 1.0, y: 1.0 };
        assert_eq!(k.input(outside, &l), None);
        let inside = Input::PointerPress { x: 120.0, y: 110.0 };
        assert_eq!(k.input(inside, &l), Some(KeyBindAction::StartedListening));
        assert_eq!(k.input(inside, &l), None);
        assert!(k.is_listening());
        assert_eq!(k.input(outside, &l), Some(KeyBindAction::Cancelled));
        assert!(!k.is_listening());
    }

    const LONG: usize = 25;

    fn long() -> DropdownLayout {
        DropdownLayout::new(BOUNDS, LONG, 1.0).within(1000.0)
    }

    #[test]
    fn a_long_list_shows_a_capped_window() {
        let l = long();
        assert_eq!(l.visible(), MAX_VISIBLE_OPTIONS);
        assert_eq!(l.list().height, 40.0 * MAX_VISIBLE_OPTIONS as f32);
        let short = DropdownLayout::new(BOUNDS, 3, 1.0).within(1000.0);
        assert_eq!(short.visible(), 3);
    }

    #[test]
    fn the_list_opens_above_when_there_is_no_room_below() {
        // The box is at y 100..140; a 300 high window has room for 4 below and 2 above.
        let l = DropdownLayout::new(BOUNDS, LONG, 1.0).within(300.0);
        assert_eq!((l.visible(), l.list().y), (4, 140.0));
        // A box near the bottom: more room above.
        let low = Rect { y: 600.0, ..BOUNDS };
        let l = DropdownLayout::new(low, LONG, 1.0).within(700.0);
        assert_eq!(l.visible(), 10);
        assert_eq!(l.list().y, 200.0);
        assert!(l.list().y + l.list().height <= low.y);
        // Neither side fits everything: the larger side, shortened, inside the window.
        let l = DropdownLayout::new(low, LONG, 1.0).within(650.0);
        assert_eq!(l.visible(), 10);
        let l = DropdownLayout::new(Rect { y: 200.0, ..BOUNDS }, LONG, 1.0).within(400.0);
        assert!(l.list().y >= 0.0 && l.list().y + l.list().height <= 400.0);
    }

    #[test]
    fn opening_scrolls_the_selection_into_view() {
        let l = long();
        let mut d = Dropdown::new(LONG, 20);
        d.input(DropdownInput::Activate, &l);
        assert_eq!(d.scroll(), 20 + 1 - MAX_VISIBLE_OPTIONS);
        let mut d = Dropdown::new(LONG, 3);
        d.input(DropdownInput::Activate, &l);
        assert_eq!(d.scroll(), 0);
    }

    #[test]
    fn arrows_scroll_to_keep_the_highlight_visible() {
        let l = long();
        let mut d = Dropdown::new(LONG, 0);
        d.input(DropdownInput::Activate, &l);
        for _ in 0..12 {
            d.input(DropdownInput::Down, &l);
        }
        assert_eq!(d.highlight(), 12);
        assert_eq!(d.scroll(), 3);
        for _ in 0..12 {
            d.input(DropdownInput::Up, &l);
        }
        assert_eq!((d.highlight(), d.scroll()), (0, 0));
    }

    #[test]
    fn the_wheel_scrolls_within_bounds_and_only_while_open() {
        let l = long();
        let mut d = Dropdown::new(LONG, 0);
        d.input(DropdownInput::Scroll { lines: 3.0 }, &l);
        assert_eq!(d.scroll(), 0);
        d.input(DropdownInput::Activate, &l);
        d.input(DropdownInput::Scroll { lines: 3.0 }, &l);
        assert_eq!(d.scroll(), 3);
        d.input(DropdownInput::Scroll { lines: 0.1 }, &l);
        assert_eq!(d.scroll(), 4);
        d.input(DropdownInput::Scroll { lines: 100.0 }, &l);
        assert_eq!(d.scroll(), LONG - MAX_VISIBLE_OPTIONS);
        d.input(DropdownInput::Scroll { lines: -100.0 }, &l);
        assert_eq!(d.scroll(), 0);
        // The highlight and selection did not move.
        assert_eq!((d.highlight(), d.selected()), (0, 0));
    }

    #[test]
    fn hit_testing_accounts_for_the_scroll_offset() {
        let l = long().scrolled(5);
        let first = l.option(5);
        assert_eq!(first.y, BOUNDS.y + BOUNDS.height);
        let [x, y] = first.centre();
        assert_eq!(l.option_at(x, y), Some(5));
        let [x, y] = l.option(14).centre();
        assert_eq!(l.option_at(x, y), Some(14));
        // Pressing picks the option under the pointer, not the slot.
        let mut d = Dropdown::new(LONG, 0);
        d.input(DropdownInput::Activate, &long());
        d.input(DropdownInput::Scroll { lines: 5.0 }, &long());
        let [x, y] = long().scrolled(5).option(7).centre();
        assert_eq!(
            d.input(DropdownInput::PointerPress { x, y }, &long()),
            Some(DropdownAction::Selected(7))
        );
    }
}
