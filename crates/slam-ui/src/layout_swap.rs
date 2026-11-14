//! Keyboard layout swapping for search: what a text typed on one layout reads as when the
//! same physical keys are pressed on the other (ADR-0137).
//!
//! The table is the standard Russian ЙЦУКЕН against the English QWERTY, key by key, including
//! the punctuation keys that carry letters (`[ ] ; ' , . \``). It is symmetric: a Latin
//! character maps to the Cyrillic one on its key, and back. Characters that are not on the
//! table (digits, spaces, other letters) stay as they are.

/// `(latin, cyrillic)` of each physical key, lowercase / unshifted form.
const KEYS: [(char, char); 33] = [
    ('q', 'й'),
    ('w', 'ц'),
    ('e', 'у'),
    ('r', 'к'),
    ('t', 'е'),
    ('y', 'н'),
    ('u', 'г'),
    ('i', 'ш'),
    ('o', 'щ'),
    ('p', 'з'),
    ('[', 'х'),
    (']', 'ъ'),
    ('a', 'ф'),
    ('s', 'ы'),
    ('d', 'в'),
    ('f', 'а'),
    ('g', 'п'),
    ('h', 'р'),
    ('j', 'о'),
    ('k', 'л'),
    ('l', 'д'),
    (';', 'ж'),
    ('\'', 'э'),
    ('z', 'я'),
    ('x', 'ч'),
    ('c', 'с'),
    ('v', 'м'),
    ('b', 'и'),
    ('n', 'т'),
    ('m', 'ь'),
    (',', 'б'),
    ('.', 'ю'),
    ('`', 'ё'),
];

/// The shifted punctuation keys whose Cyrillic form is an uppercase letter: `{ } : " < > ~`.
const SHIFTED: [(char, char); 7] = [
    ('{', 'Х'),
    ('}', 'Ъ'),
    (':', 'Ж'),
    ('"', 'Э'),
    ('<', 'Б'),
    ('>', 'Ю'),
    ('~', 'Ё'),
];

fn lookup(c: char) -> Option<char> {
    for &(latin, cyrillic) in KEYS.iter().chain(&SHIFTED) {
        if c == latin {
            return Some(cyrillic);
        }
        if c == cyrillic {
            return Some(latin);
        }
    }
    None
}

/// The character on the same key of the other layout; `c` itself when it is not on the
/// table. Case is kept (`Q` gives `Й`); a shifted Cyrillic punctuation letter (`Х`) gives
/// the shifted punctuation (`{`).
pub fn swap_char(c: char) -> char {
    if let Some(swapped) = lookup(c) {
        return swapped;
    }
    if c.is_uppercase() {
        // Uppercase letters: swap the lowercase form and raise the result.
        let mut lower = c.to_lowercase();
        if let (Some(l), None) = (lower.next(), lower.next())
            && let Some(swapped) = lookup(l)
        {
            let mut upper = swapped.to_uppercase();
            if let (Some(u), None) = (upper.next(), upper.next()) {
                return u;
            }
        }
    }
    c
}

/// Writes `s` with every character swapped into `buf` and returns it. What does not fit
/// is cut at a character. Does not allocate.
pub fn swap_into<'a>(s: &str, buf: &'a mut [u8]) -> &'a str {
    let mut len = 0;
    for c in s.chars() {
        let swapped = swap_char(c);
        let width = swapped.len_utf8();
        if len + width > buf.len() {
            break;
        }
        swapped.encode_utf8(&mut buf[len..]);
        len += width;
    }
    // Only whole characters were written.
    std::str::from_utf8(&buf[..len]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap(s: &str) -> String {
        s.chars().map(swap_char).collect()
    }

    #[test]
    fn letters_both_ways() {
        assert_eq!(swap("ghbdtn"), "привет");
        assert_eq!(swap("привет"), "ghbdtn");
        assert_eq!(
            swap("еркщгпр еру ашку фтв адфьуы"),
            "through the fire and flames"
        );
    }

    #[test]
    fn punctuation_keys_carry_letters() {
        assert_eq!(swap("[];',.`"), "хъжэбюё");
        assert_eq!(swap("хъжэбюё"), "[];',.`");
        assert_eq!(swap("{}:\"<>~"), "ХЪЖЭБЮЁ");
        assert_eq!(swap("ХЪЖЭБЮЁ"), "{}:\"<>~");
    }

    #[test]
    fn case_is_kept() {
        assert_eq!(swap("Ghbdtn"), "Привет");
        assert_eq!(swap("ПРИВЕТ"), "GHBDTN");
        assert_eq!(swap_char('Q'), 'Й');
        assert_eq!(swap_char('й'), 'q');
    }

    #[test]
    fn unmapped_characters_are_kept() {
        assert_eq!(swap("12 -=_ ü é 日本"), "12 -=_ ü é 日本");
    }

    #[test]
    fn round_trip_over_the_table() {
        for &(a, b) in KEYS.iter().chain(&SHIFTED) {
            assert_eq!(swap_char(swap_char(a)), a);
            assert_eq!(swap_char(swap_char(b)), b);
            assert_eq!(swap_char(a), b);
        }
        let text = "Through the Fire [and] Flames; привет, Мир.";
        assert_eq!(swap(&swap(text)), text);
    }

    #[test]
    fn table_has_no_duplicates() {
        let mut seen = Vec::new();
        for &(a, b) in KEYS.iter().chain(&SHIFTED) {
            assert!(!seen.contains(&a) && !seen.contains(&b));
            seen.push(a);
            seen.push(b);
        }
    }

    #[test]
    fn swap_into_cuts_at_a_character() {
        let mut buf = [0u8; 5];
        // Latin to Cyrillic: two bytes each; the third does not fit.
        assert_eq!(swap_into("ghb", &mut buf), "пр");
        let mut buf = [0u8; 16];
        assert_eq!(swap_into("ghbdtn", &mut buf), "привет");
        assert_eq!(swap_into("", &mut buf), "");
    }
}
