//! Entry name decoding, normalization and filtering.

/// Why a name cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rejected {
    /// Absolute path, `:`, `..`, control character, reserved device name or a component
    /// that ends in `.` or a space.
    Unsafe,
    /// Nothing left after normalization.
    Empty,
}

/// Decodes raw name bytes; the flag is true when the result is lossy.
pub(super) fn decode_name(raw: &[u8]) -> (String, bool) {
    if let Ok(s) = std::str::from_utf8(raw) {
        return (s.to_owned(), false);
    }
    let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(raw);
    (text.into_owned(), had_errors)
}

/// Normalizes a name for lookup: `\` to `/`, empty and `.` components removed.
/// `..` components are kept.
pub(super) fn clean(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

/// Device names Windows resolves in any directory, with or without an extension.
fn is_reserved_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let mut chars = upper.chars();
            let prefix: String = chars.by_ref().take(3).collect();
            let digit = chars.next();
            (prefix == "COM" || prefix == "LPT")
                && chars.next().is_none()
                && matches!(digit, Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'))
        }
    }
}

/// Normalizes and validates an entry name.
///
/// A returned name is safe to join onto a base directory on Windows and Unix: it has no
/// absolute path, drive or stream (`:`), `..`, control character, reserved device name or
/// component that Windows would silently alter (trailing `.` or space).
pub(super) fn normalize(name: &str) -> Result<String, Rejected> {
    if name.starts_with(['/', '\\']) || name.chars().any(char::is_control) {
        return Err(Rejected::Unsafe);
    }
    let cleaned = clean(name);
    if cleaned.is_empty() {
        return Err(Rejected::Empty);
    }
    for part in cleaned.split('/') {
        if part == ".."
            || part.contains(':')
            || part.ends_with(['.', ' '])
            || is_reserved_device(part)
        {
            return Err(Rejected::Unsafe);
        }
    }
    Ok(cleaned)
}

/// True for names that denote a directory.
pub(super) fn is_dir_name(name: &str) -> bool {
    name.ends_with(['/', '\\'])
}

/// True for operating-system litter that is never part of a beatmap or skin.
pub(super) fn is_system_file(normalized: &str) -> bool {
    let mut parts = normalized.split('/').peekable();
    while let Some(part) = parts.next() {
        if part.eq_ignore_ascii_case("__MACOSX") {
            return true;
        }
        if parts.peek().is_none() {
            return [".DS_Store", "Thumbs.db", "desktop.ini"]
                .iter()
                .any(|n| part.eq_ignore_ascii_case(n));
        }
    }
    false
}

/// Key for case-insensitive comparison.
pub(super) fn key(normalized: &str) -> String {
    normalized.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("a\\b//./c.osu").unwrap(), "a/b/c.osu");
        assert_eq!(normalize("./a.osu").unwrap(), "a.osu");
        assert_eq!(normalize("../a"), Err(Rejected::Unsafe));
        assert_eq!(normalize("a/../b"), Err(Rejected::Unsafe));
        assert_eq!(normalize("a\\..\\b"), Err(Rejected::Unsafe));
        assert_eq!(normalize("/a"), Err(Rejected::Unsafe));
        assert_eq!(normalize("\\a"), Err(Rejected::Unsafe));
        assert_eq!(normalize("C:/a"), Err(Rejected::Unsafe));
        assert_eq!(normalize("c:a"), Err(Rejected::Unsafe));
        assert_eq!(normalize("./"), Err(Rejected::Empty));
        assert_eq!(normalize(""), Err(Rejected::Empty));
        assert_eq!(normalize("ab:c"), Err(Rejected::Unsafe));
        for bad in [
            "./C:/evil.osu",
            ".\\C:\\x",
            "//C:x",
            "a/b:c",
            ".. /x",
            "a.osu.",
            "a /b",
            "a\0b",
            "a\tb",
            "x/...",
            "CON",
            "con.txt",
            "a/Nul.osu",
            "COM1",
            "lpt9.x",
            "aux /x",
        ] {
            assert_eq!(normalize(bad), Err(Rejected::Unsafe), "{bad:?}");
        }
        assert_eq!(normalize("COM0").unwrap(), "COM0");
        for bad in ["COM\u{b9}", "lpt\u{b2}.x", "CONIN$", "conout$.txt"] {
            assert_eq!(normalize(bad), Err(Rejected::Unsafe), "{bad:?}");
        }
        assert_eq!(normalize("console.osu").unwrap(), "console.osu");
        assert_eq!(normalize(".").unwrap_err(), Rejected::Empty);
        assert_eq!(normalize("..a").unwrap(), "..a");
    }

    #[test]
    fn system_files() {
        assert!(is_system_file("__MACOSX/._a.osu"));
        assert!(is_system_file("x/__macosx/a"));
        assert!(is_system_file(".DS_Store"));
        assert!(is_system_file("sb/THUMBS.DB"));
        assert!(is_system_file("Desktop.ini"));
        assert!(!is_system_file("Thumbs.db/a.png"));
        assert!(!is_system_file("a.osu"));
    }

    #[test]
    fn decodes_names() {
        assert_eq!(decode_name(b"a.osu"), ("a.osu".to_owned(), false));
        assert_eq!(
            decode_name("曲.osu".as_bytes()),
            ("曲.osu".to_owned(), false)
        );
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("曲.osu");
        assert_eq!(decode_name(&sjis), ("曲.osu".to_owned(), false));
        let (lossy, flag) = decode_name(&[0x81, 0x00]);
        assert!(flag && lossy.contains('\u{fffd}'));
    }
}
