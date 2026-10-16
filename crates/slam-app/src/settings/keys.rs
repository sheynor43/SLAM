//! Names of keys and mouse buttons in the settings file.
//!
//! Keys are SDL scancodes (USB HID usage ids). Names are case-insensitive on read and
//! canonical on write; a scancode without a name is written as `Scancode<n>`.

use slam_input::MouseButton;
use slam_osu::replay::Actions;
use slam_play::input::Input;

/// Named keys outside the letter, digit and function-key runs.
const NAMED: &[(&str, u16)] = &[
    ("Enter", 40),
    ("Escape", 41),
    ("Backspace", 42),
    ("Tab", 43),
    ("Space", 44),
    ("-", 45),
    ("=", 46),
    ("[", 47),
    ("]", 48),
    ("\\", 49),
    (";", 51),
    ("'", 52),
    ("`", 53),
    (",", 54),
    (".", 55),
    ("/", 56),
    ("CapsLock", 57),
    ("Right", 79),
    ("Left", 80),
    ("Down", 81),
    ("Up", 82),
    ("LCtrl", 224),
    ("LShift", 225),
    ("LAlt", 226),
    ("RCtrl", 228),
    ("RShift", 229),
    ("RAlt", 230),
];

const MOUSE: &[(&str, MouseButton)] = &[
    ("MouseLeft", MouseButton::Left),
    ("MouseMiddle", MouseButton::Middle),
    ("MouseRight", MouseButton::Right),
    ("MouseX1", MouseButton::X1),
    ("MouseX2", MouseButton::X2),
];

/// Canonical name of a keyboard scancode.
pub fn key_name(scancode: u16) -> String {
    match scancode {
        4..=29 => ((b'A' + (scancode - 4) as u8) as char).to_string(),
        30..=38 => ((b'1' + (scancode - 30) as u8) as char).to_string(),
        39 => "0".to_string(),
        58..=69 => format!("F{}", scancode - 57),
        _ => match NAMED.iter().find(|(_, code)| *code == scancode) {
            Some((name, _)) => (*name).to_string(),
            None => format!("Scancode{scancode}"),
        },
    }
}

/// The scancode called `name`, case-insensitive.
pub fn key_from_name(name: &str) -> Option<u16> {
    let name = name.trim();
    if let Some(&(_, code)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(code);
    }
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0].to_ascii_uppercase();
        match c {
            b'A'..=b'Z' => return Some(4 + u16::from(c - b'A')),
            b'1'..=b'9' => return Some(30 + u16::from(c - b'1')),
            b'0' => return Some(39),
            _ => {}
        }
    }
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(n) = rest.parse::<u16>()
        && (1..=12).contains(&n)
        && !rest.starts_with('+')
    {
        return Some(57 + n);
    }
    if let Some(rest) = lower.strip_prefix("scancode")
        && !rest.starts_with('+')
    {
        return rest.parse::<u16>().ok();
    }
    None
}

/// Canonical name of an input.
pub fn input_name(input: Input) -> String {
    match input {
        Input::Key(code) => key_name(code),
        Input::Mouse(button) => MOUSE
            .iter()
            .find(|(_, b)| *b == button)
            .map_or_else(String::new, |(n, _)| (*n).to_string()),
    }
}

/// The input called `name`, case-insensitive.
pub fn input_from_name(name: &str) -> Option<Input> {
    let name = name.trim();
    if let Some(&(_, button)) = MOUSE.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(Input::Mouse(button));
    }
    key_from_name(name).map(Input::Key)
}

/// Canonical name of a single action, `None` for anything else.
pub fn action_name(action: Actions) -> Option<&'static str> {
    if action == Actions::LEFT {
        Some("left")
    } else if action == Actions::RIGHT {
        Some("right")
    } else if action == Actions::SMOKE {
        Some("smoke")
    } else {
        None
    }
}

/// The action called `name`, case-insensitive.
pub fn action_from_name(name: &str) -> Option<Actions> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Actions::LEFT),
        "right" => Some(Actions::RIGHT),
        "smoke" => Some(Actions::SMOKE),
        _ => None,
    }
}
