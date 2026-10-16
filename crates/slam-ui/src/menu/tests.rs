use super::*;

fn layout() -> MenuLayout {
    MenuLayout::new(1280.0, 720.0, &Theme::default())
}

fn press(menu: &mut MainMenu, input: MenuInput) -> Option<MenuAction> {
    menu.input(input, &layout())
}

#[test]
fn starts_on_play_and_activates_it() {
    let mut menu = MainMenu::new();
    assert_eq!(menu.selected(), MenuItem::Play);
    assert_eq!(
        press(&mut menu, MenuInput::Activate),
        Some(MenuAction::Play)
    );
}

#[test]
fn keyboard_wraps_and_skips_the_disabled_item() {
    let mut menu = MainMenu::new();
    assert_eq!(press(&mut menu, MenuInput::Down), None);
    assert_eq!(menu.selected(), MenuItem::Exit);
    assert_eq!(press(&mut menu, MenuInput::Down), None);
    assert_eq!(menu.selected(), MenuItem::Play);
    press(&mut menu, MenuInput::Up);
    assert_eq!(menu.selected(), MenuItem::Exit);
    assert_eq!(
        press(&mut menu, MenuInput::Activate),
        Some(MenuAction::Exit)
    );
    press(&mut menu, MenuInput::Up);
    assert_eq!(menu.selected(), MenuItem::Play);
}

#[test]
fn back_exits() {
    assert_eq!(
        press(&mut MainMenu::new(), MenuInput::Back),
        Some(MenuAction::Exit)
    );
}

fn centre(item: MenuItem) -> (f32, f32) {
    let index = MenuItem::ALL.iter().position(|i| *i == item).unwrap();
    let [x, y] = layout().buttons[index].centre();
    (x, y)
}

#[test]
fn pointer_hover_selects_and_press_activates() {
    let mut menu = MainMenu::new();
    let (x, y) = centre(MenuItem::Exit);
    assert_eq!(press(&mut menu, MenuInput::PointerMove { x, y }), None);
    assert_eq!(menu.selected(), MenuItem::Exit);
    assert_eq!(
        press(&mut menu, MenuInput::PointerPress { x, y }),
        Some(MenuAction::Exit)
    );
    let (x, y) = centre(MenuItem::Play);
    assert_eq!(
        press(&mut menu, MenuInput::PointerPress { x, y }),
        Some(MenuAction::Play)
    );
    assert_eq!(menu.selected(), MenuItem::Play);
}

#[test]
fn pointer_ignores_the_disabled_item_and_empty_space() {
    let mut menu = MainMenu::new();
    let (x, y) = centre(MenuItem::Settings);
    assert_eq!(press(&mut menu, MenuInput::PointerMove { x, y }), None);
    assert_eq!(menu.selected(), MenuItem::Play);
    assert_eq!(press(&mut menu, MenuInput::PointerPress { x, y }), None);
    assert_eq!(
        press(&mut menu, MenuInput::PointerPress { x: 1.0, y: 1.0 }),
        None
    );
    assert_eq!(menu.selected(), MenuItem::Play);
}

#[test]
fn layout_keeps_everything_inside_the_window() {
    for (w, h) in [(1280.0, 720.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let layout = MenuLayout::new(w, h, &Theme::default());
        for rect in layout.buttons.iter().chain([&layout.title]) {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{w}x{h}: {rect:?}");
            assert!(rect.x + rect.width <= w && rect.y + rect.height <= h);
        }
        // Buttons do not overlap and are in order.
        for pair in layout.buttons.windows(2) {
            assert!(pair[0].y + pair[0].height <= pair[1].y);
        }
        // Hit testing agrees with the rectangles.
        for (item, rect) in MenuItem::ALL.into_iter().zip(&layout.buttons) {
            let [x, y] = rect.centre();
            assert_eq!(layout.item_at(x, y), Some(item));
        }
        assert_eq!(layout.item_at(0.0, 0.0), None);
    }
}
