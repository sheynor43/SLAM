use super::*;

const MS: u64 = 1_000_000;

fn layout() -> MenuLayout {
    MenuLayout::new(1280.0, 720.0, &Theme::default())
}

fn menu(style: MenuStyle) -> MainMenu {
    let mut menu = MainMenu::new();
    menu.set_style(style);
    menu
}

fn press(menu: &mut MainMenu, input: MenuInput, now_ms: u64) -> Option<MenuAction> {
    menu.input(input, &layout(), now_ms * MS)
}

/// Opens the buttons by clicking the logo alone at time 0.
fn open(menu: &mut MainMenu) {
    let [x, y] = menu.snapshot(&layout(), 0).logo_centre;
    press(menu, MenuInput::PointerPress { x, y }, 0);
    assert!(menu.is_open());
}

fn centre_of(menu: &MainMenu, item: MenuItem, now_ms: u64) -> (f32, f32) {
    let index = MenuItem::ALL.iter().position(|i| *i == item).unwrap();
    let [x, y] = menu.snapshot(&layout(), now_ms * MS).buttons[index]
        .rect
        .centre();
    (x, y)
}

#[test]
fn enter_plays_and_keys_open_the_buttons() {
    for style in [MenuStyle::Lazer, MenuStyle::Stable] {
        let mut menu = menu(style);
        assert!(!menu.is_open());
        assert_eq!(
            press(&mut menu, MenuInput::Activate, 0),
            Some(MenuAction::Play)
        );
        assert!(!menu.is_open());
        press(&mut menu, MenuInput::Up, 10);
        assert!(menu.is_open());
        assert_eq!(menu.take_cue(), Some(MenuCue::Open));
    }
}

#[test]
fn hotkeys_open_then_act() {
    let mut menu = menu(MenuStyle::Lazer);
    let key = |item| MenuInput::Hotkey(item);
    assert_eq!(press(&mut menu, key(MenuItem::Exit), 0), None);
    assert!(!menu.is_open(), "only play opens the buttons");
    assert_eq!(press(&mut menu, key(MenuItem::Play), 10), None);
    assert!(menu.is_open());
    assert_eq!(
        press(&mut menu, key(MenuItem::Play), 20),
        Some(MenuAction::Play)
    );
    assert_eq!(
        press(&mut menu, key(MenuItem::Settings), 30),
        Some(MenuAction::Settings)
    );
}

#[test]
fn back_closes_then_exits() {
    let mut menu = menu(MenuStyle::Lazer);
    press(&mut menu, MenuInput::Down, 0);
    menu.take_cue();
    assert_eq!(press(&mut menu, MenuInput::Back, 100), None);
    assert!(!menu.is_open());
    assert_eq!(menu.take_cue(), Some(MenuCue::Close));
    assert_eq!(
        press(&mut menu, MenuInput::Back, 200),
        Some(MenuAction::Exit)
    );
}

#[test]
fn keys_open_the_buttons_and_walk_them() {
    for style in [MenuStyle::Lazer, MenuStyle::Stable] {
        let mut menu = menu(style);
        press(&mut menu, MenuInput::Down, 0);
        assert!(menu.is_open(), "the first key opens the buttons");
        assert_eq!(menu.selected(), MenuItem::Settings);
        press(&mut menu, MenuInput::Down, 10);
        assert_eq!(menu.selected(), MenuItem::Exit);
        press(&mut menu, MenuInput::Down, 20);
        assert_eq!(menu.selected(), MenuItem::Play);
        press(&mut menu, MenuInput::Up, 30);
        assert_eq!(menu.selected(), MenuItem::Exit);
        assert_eq!(
            press(&mut menu, MenuInput::Activate, 40),
            Some(MenuAction::Exit)
        );
    }
}

#[test]
fn clicking_the_logo_opens_then_plays() {
    for style in [MenuStyle::Lazer, MenuStyle::Stable] {
        let mut menu = menu(style);
        let [x, y] = menu.snapshot(&layout(), 0).logo_centre;
        assert_eq!(press(&mut menu, MenuInput::PointerPress { x, y }, 0), None);
        assert!(menu.is_open());
        let [x, y] = menu.snapshot(&layout(), 2000 * MS).logo_centre;
        assert_eq!(
            press(&mut menu, MenuInput::PointerPress { x, y }, 2000),
            Some(MenuAction::Play)
        );
    }
}

#[test]
fn the_logo_moves_aside_and_the_buttons_come_out() {
    let mut lazer = menu(MenuStyle::Lazer);
    let alone = lazer.snapshot(&layout(), 0);

    assert_eq!(alone.logo_centre, [640.0, 360.0]);
    assert!(alone.buttons.iter().all(|b| b.alpha == 0.0));
    open(&mut lazer);
    let out = lazer.snapshot(&layout(), 2000 * MS);
    assert!(out.logo_size < alone.logo_size * 0.5);
    assert!(out.bar_alpha == 1.0 && out.buttons.iter().all(|b| b.alpha == 1.0));
    let [settings, play, exit] = [1, 0, 2].map(|i| out.buttons[i].rect);
    assert!(settings.x + settings.width < out.logo_centre[0]);
    assert!(play.x > out.logo_centre[0] && exit.x >= play.x + play.width - 0.01);

    let mut stable = menu(MenuStyle::Stable);
    open(&mut stable);
    let out = stable.snapshot(&layout(), 2000 * MS);
    assert!(out.logo_centre[0] < 640.0);
    assert_eq!(out.logo_size, layout().logo_alone_in(MenuStyle::Stable).1);
    let rects = out.buttons.map(|b| b.rect);
    assert!(
        rects[0].y < rects[1].y && rects[1].y < rects[2].y,
        "a column"
    );
    assert!(
        rects
            .iter()
            .all(|r| r.x + r.width > out.logo_centre[0] + out.logo_size / 2.0)
    );
}

#[test]
fn hovering_a_button_selects_it_and_pressing_activates() {
    for style in [MenuStyle::Lazer, MenuStyle::Stable] {
        let mut menu = menu(style);
        open(&mut menu);
        let (x, y) = centre_of(&menu, MenuItem::Exit, 2000);
        press(&mut menu, MenuInput::PointerMove { x, y }, 2000);
        assert_eq!(menu.selected(), MenuItem::Exit, "{style:?}");
        let (x, y) = centre_of(&menu, MenuItem::Settings, 3000);
        assert_eq!(
            press(&mut menu, MenuInput::PointerPress { x, y }, 3000),
            Some(MenuAction::Settings),
            "{style:?}"
        );
    }
}

#[test]
fn a_hovered_lazer_button_widens() {
    let mut menu = menu(MenuStyle::Lazer);
    open(&mut menu);
    let before = menu.snapshot(&layout(), 2000 * MS).buttons[0].rect.width;
    let (x, y) = centre_of(&menu, MenuItem::Play, 2000);
    press(&mut menu, MenuInput::PointerMove { x, y }, 2000);
    let after = menu.snapshot(&layout(), 3000 * MS).buttons[0].rect.width;
    assert!((after / before - HOVER_WIDTH).abs() < 1e-3);
}

#[test]
fn the_stable_column_closes_when_left_alone() {
    let mut menu = menu(MenuStyle::Stable);
    open(&mut menu);
    menu.tick(5000 * MS, &LogoAudio::default(), true);
    assert!(menu.is_open());
    menu.tick(6100 * MS, &LogoAudio::default(), true);
    assert!(!menu.is_open());
    let mut lazer = self::menu(MenuStyle::Lazer);
    open(&mut lazer);
    lazer.tick(60_000 * MS, &LogoAudio::default(), true);
    assert!(lazer.is_open(), "lazer's bar stays");
}

#[test]
fn closed_buttons_do_not_take_the_pointer() {
    let mut menu = menu(MenuStyle::Lazer);
    open(&mut menu);
    let (x, y) = centre_of(&menu, MenuItem::Exit, 2000);
    press(&mut menu, MenuInput::Back, 2000);
    assert_eq!(
        press(&mut menu, MenuInput::PointerPress { x, y }, 4000),
        None
    );
}
