//! The text of a number being typed into a slider's value field (ADR-0173): a fixed buffer
//! (the update tick never allocates) that accepts what a number is made of, and parses to
//! the value to commit.

use std::fmt;

/// The most characters of an edit.
pub const EDIT_CAPACITY: usize = 24;

/// A number being typed. Starts "fresh": the first key replaces what was shown (as a
/// selected text would), later ones append.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueEdit {
    buf: [u8; EDIT_CAPACITY],
    len: u8,
    fresh: bool,
}

impl ValueEdit {
    /// An edit that shows `text` (cut to the capacity); the first key replaces it.
    pub fn new(text: &str) -> ValueEdit {
        let mut edit = ValueEdit {
            buf: [0; EDIT_CAPACITY],
            len: 0,
            fresh: true,
        };
        for c in text.chars() {
            if !edit.push(c) {
                break;
            }
        }
        edit
    }

    /// An edit of the number `value` with `decimals` digits after the point, trailing zeros
    /// dropped. Does not allocate.
    pub fn of_value(value: f64, decimals: usize) -> ValueEdit {
        let mut edit = ValueEdit::new("");
        let _ = fmt::write(&mut edit, format_args!("{value:.decimals$}"));
        if edit.as_str().contains('.') {
            while edit.last() == Some(b'0') {
                edit.len -= 1;
            }
            if edit.last() == Some(b'.') {
                edit.len -= 1;
            }
        }
        edit.fresh = true;
        edit
    }

    fn last(&self) -> Option<u8> {
        self.len.checked_sub(1).map(|i| self.buf[usize::from(i)])
    }

    /// The text as typed (ASCII).
    pub fn as_str(&self) -> &str {
        // Only ASCII is ever pushed, so the bytes are valid UTF-8.
        std::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }

    /// Nothing has been typed yet: the next key replaces the text.
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    fn push(&mut self, c: char) -> bool {
        if usize::from(self.len) >= EDIT_CAPACITY || !c.is_ascii() {
            return false;
        }
        self.buf[usize::from(self.len)] = c as u8;
        self.len += 1;
        true
    }

    /// Types `c`: a digit; `.` or `,` (one, as a point); `-` (only first). Anything else is
    /// refused. Returns whether the text changed.
    pub fn type_char(&mut self, c: char) -> bool {
        let c = if c == ',' { '.' } else { c };
        let ok = match c {
            '0'..='9' => true,
            '.' => self.fresh || !self.as_str().contains('.'),
            '-' => self.fresh || self.len == 0,
            _ => false,
        };
        if !ok {
            return false;
        }
        if self.fresh {
            self.len = 0;
            self.fresh = false;
        }
        self.push(c)
    }

    /// Backspace: erases the last character (all of a fresh text). Returns whether the
    /// text changed.
    pub fn erase(&mut self) -> bool {
        if self.fresh {
            return self.erase_all();
        }
        if self.len == 0 {
            return false;
        }
        self.len -= 1;
        true
    }

    /// Ctrl+Backspace: a number is one word, so it all goes.
    pub fn erase_all(&mut self) -> bool {
        self.fresh = false;
        let changed = self.len != 0;
        self.len = 0;
        changed
    }

    /// The typed number, `None` for text that is no number yet (`""`, `"-"`, `"."`).
    pub fn parse(&self) -> Option<f64> {
        let value: f64 = self.as_str().parse().ok()?;
        value.is_finite().then_some(value)
    }
}

impl fmt::Write for ValueEdit {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            if !self.push(c) {
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> ValueEdit {
        let mut edit = ValueEdit::new("");
        for c in text.chars() {
            edit.type_char(c);
        }
        edit
    }

    #[test]
    fn accepts_digits_one_point_and_a_leading_minus() {
        assert_eq!(typed("12.5").as_str(), "12.5");
        assert_eq!(typed("1,5").as_str(), "1.5");
        assert_eq!(typed("1.2.3").as_str(), "1.23");
        assert_eq!(typed("-4").as_str(), "-4");
        assert_eq!(typed("4-2").as_str(), "42");
        assert_eq!(typed("a1b").as_str(), "1");
    }

    #[test]
    fn the_first_key_replaces_the_shown_value() {
        let mut edit = ValueEdit::new("50");
        assert!(edit.is_fresh());
        edit.type_char('7');
        assert_eq!(edit.as_str(), "7");
        edit.type_char('5');
        assert_eq!(edit.as_str(), "75");
        // A refused key leaves a fresh text alone.
        let mut fresh = ValueEdit::new("50");
        assert!(!fresh.type_char('x'));
        assert_eq!(fresh.as_str(), "50");
    }

    #[test]
    fn erase_goes_back_one_char_and_a_fresh_text_goes_whole() {
        let mut edit = typed("123");
        assert!(edit.erase());
        assert_eq!(edit.as_str(), "12");
        assert!(edit.erase_all());
        assert_eq!(edit.as_str(), "");
        assert!(!edit.erase());
        let mut fresh = ValueEdit::new("50");
        assert!(fresh.erase());
        assert_eq!(fresh.as_str(), "");
    }

    #[test]
    fn parses_numbers_and_refuses_the_rest() {
        assert_eq!(typed("12.5").parse(), Some(12.5));
        assert_eq!(typed("-3").parse(), Some(-3.0));
        assert_eq!(typed(".5").parse(), Some(0.5));
        assert_eq!(typed("5.").parse(), Some(5.0));
        assert_eq!(typed("").parse(), None);
        assert_eq!(typed("-").parse(), None);
        assert_eq!(typed(".").parse(), None);
    }

    #[test]
    fn the_buffer_is_bounded() {
        let long = "1".repeat(100);
        assert_eq!(typed(&long).as_str().len(), EDIT_CAPACITY);
    }

    #[test]
    fn a_value_shows_without_trailing_zeros() {
        assert_eq!(ValueEdit::of_value(1.5, 2).as_str(), "1.5");
        assert_eq!(ValueEdit::of_value(2.0, 2).as_str(), "2");
        assert_eq!(ValueEdit::of_value(-0.25, 2).as_str(), "-0.25");
        assert_eq!(ValueEdit::of_value(100.0, 0).as_str(), "100");
    }
}
