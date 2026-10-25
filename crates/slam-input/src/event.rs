use crate::WindowMode;

/// One input event, stamped in the engine clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputEvent {
    /// Time the OS (or SDL, where the OS gives no timestamp) observed the event, in
    /// nanoseconds of the engine's monotonic clock. Events from different devices may
    /// arrive slightly out of timestamp order.
    pub time_ns: u64,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputKind {
    /// Physical key (USB HID usage id, as `SDL_Scancode`). OS key repeats come as
    /// [`InputKind::KeyRepeat`] instead.
    Key {
        scancode: u16,
        down: bool,
    },
    /// An OS auto-repeat of a held key (ADR-0065).
    KeyRepeat {
        scancode: u16,
    },
    /// Text typed while text input is on (see `TextInput`), in pieces of at most
    /// [`TextChunk::CAPACITY`] bytes split at character boundaries.
    Text(TextChunk),
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Cursor position in window points (not pixels: the window is high-DPI aware,
    /// scale by `SDL_GetWindowPixelDensity` for pixels). Includes motion SDL
    /// synthesises from touch and pen, so tablets in absolute mode work as mice.
    MouseMove {
        x: f32,
        y: f32,
    },
    /// Vertical wheel scroll in notches, positive away from the user, as the OS reports it
    /// (natural scrolling already applied). Fractional on precise touchpads.
    MouseWheel {
        y: f32,
    },
    /// Alt+Enter or F11 switched the window to this mode (ADR-0070). The key press itself
    /// is not delivered; its release is.
    WindowMode(WindowMode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    X1,
    X2,
}

/// A piece of UTF-8 text small enough to keep [`InputEvent`] `Copy` (ADR-0065).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TextChunk {
    bytes: [u8; TextChunk::CAPACITY],
    len: u8,
}

impl TextChunk {
    /// The most bytes one chunk holds.
    pub const CAPACITY: usize = 15;

    /// The chunk holding `text`, or `None` if it is longer than [`TextChunk::CAPACITY`].
    pub fn new(text: &str) -> Option<Self> {
        if text.len() > Self::CAPACITY {
            return None;
        }
        let mut bytes = [0; Self::CAPACITY];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self {
            bytes,
            // Fits: at most CAPACITY bytes, checked above.
            len: text.len() as u8,
        })
    }

    /// Splits `text` into chunks at character boundaries, each as full as it can be.
    pub fn split(text: &str) -> impl Iterator<Item = TextChunk> + '_ {
        let mut rest = text;
        std::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let mut end = rest.len().min(Self::CAPACITY);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (head, tail) = rest.split_at(end);
            rest = tail;
            Self::new(head)
        })
    }

    pub fn as_str(&self) -> &str {
        let bytes = &self.bytes[..usize::from(self.len)];
        // SAFETY: `new` copies a whole `&str`, so the bytes are valid UTF-8.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

impl std::fmt::Debug for TextChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TextChunk").field(&self.as_str()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_holds_up_to_capacity() {
        assert_eq!(TextChunk::new("").unwrap().as_str(), "");
        assert_eq!(TextChunk::new("abc").unwrap().as_str(), "abc");
        let full = "x".repeat(TextChunk::CAPACITY);
        assert_eq!(TextChunk::new(&full).unwrap().as_str(), full);
        assert!(TextChunk::new(&format!("{full}x")).is_none());
    }

    #[test]
    fn split_fills_chunks_and_keeps_characters_whole() {
        let parts: Vec<_> = TextChunk::split("0123456789abcdefg").collect();
        let parts: Vec<_> = parts.iter().map(TextChunk::as_str).collect();
        assert_eq!(parts, ["0123456789abcde", "fg"]);
        // 4-byte characters: three fit in 15 bytes.
        let parts: Vec<_> = TextChunk::split("😀😀😀😀").collect();
        let parts: Vec<_> = parts.iter().map(TextChunk::as_str).collect();
        assert_eq!(parts, ["😀😀😀", "😀"]);
        assert_eq!(TextChunk::split("").count(), 0);
    }
}
