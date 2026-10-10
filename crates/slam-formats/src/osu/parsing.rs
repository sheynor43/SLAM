//! Number parsing that mirrors osu!lazer's `Parsing` helpers on top of .NET's
//! `float.Parse` / `double.Parse` / `int.Parse` with `CultureInfo.InvariantCulture`.
//!
//! The emulated grammar (default `NumberStyles`):
//! * floats/doubles: `Float | AllowThousands` -- leading/trailing white space, optional sign,
//!   digits with `,` group separators in the integer part, optional single `.`, optional
//!   exponent; additionally `Infinity`, `+Infinity`, `-Infinity` and `NaN` (case-insensitive);
//! * integers: `Integer` -- leading/trailing white space and an optional sign followed by digits.
//!
//! White space accepted around numbers is the .NET numeric set (`\t \n \v \f \r` and space);
//! trailing `\0` characters are accepted after the number, as .NET does.

use thiserror::Error;

/// Largest absolute value accepted by the default parse limit (`int.MaxValue`).
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Parsing.cs
pub const MAX_PARSE_VALUE: f64 = i32::MAX as f64;

/// Largest coordinate value used by hit object parsing.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Parsing.cs
pub const MAX_COORDINATE_VALUE: i32 = 131_072;

/// Why a number failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NumberError {
    /// Not a valid number for the target type (`FormatException`).
    #[error("invalid number format")]
    Format,
    /// Value does not fit the type or exceeds the parse limit (`OverflowException`).
    #[error("number out of range")]
    Overflow,
    /// The value is NaN and NaN is not allowed.
    #[error("not a number")]
    NotANumber,
}

fn is_net_white(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ')
}

/// Strips the white space and trailing NULs .NET permits around a number.
fn strip_net(s: &str) -> &str {
    s.trim_start_matches(is_net_white)
        .trim_end_matches('\0')
        .trim_end_matches(is_net_white)
}

/// Parses a .NET-style floating point literal into a string that Rust's parser accepts.
/// Returns `None` when it is not a plain number (special values are handled separately).
fn sanitize_float(s: &str) -> Option<String> {
    let t = strip_net(s);
    let mut out = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();

    if let Some(&c) = chars.peek()
        && (c == '+' || c == '-')
    {
        out.push(c);
        chars.next();
    }

    let mut seen_digit = false;
    let mut seen_dot = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            seen_digit = true;
            out.push(c);
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            out.push(c);
        } else if c == ',' && seen_digit && !seen_dot {
            // AllowThousands: group separators are skipped.
        } else {
            break;
        }
        chars.next();
    }
    if !seen_digit {
        return None;
    }

    if matches!(chars.peek(), Some('e' | 'E')) {
        out.push('e');
        chars.next();
        if let Some(&c) = chars.peek()
            && (c == '+' || c == '-')
        {
            out.push(c);
            chars.next();
        }
        let mut exp_digits = false;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                exp_digits = true;
                out.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if !exp_digits {
            return None;
        }
    }

    if chars.next().is_some() {
        return None;
    }
    Some(out)
}

/// Special values accepted by .NET with the invariant culture.
fn special_value(s: &str) -> Option<f64> {
    // The Infinity/NaN fallback trims `char.IsWhiteSpace` (Unicode) white space, but not NULs.
    let t = s.trim();
    let (negative, body) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if body.eq_ignore_ascii_case("infinity") {
        Some(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        })
    } else if body.eq_ignore_ascii_case("nan") {
        Some(f64::NAN)
    } else {
        None
    }
}

/// Emulates `double.Parse(s, CultureInfo.InvariantCulture)`.
pub fn net_parse_f64(s: &str) -> Result<f64, NumberError> {
    if let Some(clean) = sanitize_float(s) {
        return clean.parse::<f64>().map_err(|_| NumberError::Format);
    }
    special_value(s).ok_or(NumberError::Format)
}

/// Emulates `float.Parse(s, CultureInfo.InvariantCulture)`; rounds directly to `f32`.
pub fn net_parse_f32(s: &str) -> Result<f32, NumberError> {
    if let Some(clean) = sanitize_float(s) {
        return clean.parse::<f32>().map_err(|_| NumberError::Format);
    }
    special_value(s)
        .map(|v| v as f32)
        .ok_or(NumberError::Format)
}

/// Emulates .NET integer parsing with `NumberStyles.Integer`, widened to `i128`
/// (saturating, so callers can range-check for their own type).
fn net_parse_integer(s: &str) -> Result<i128, NumberError> {
    let t = strip_net(s);
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NumberError::Format);
    }
    let mut value: i128 = 0;
    for b in digits.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i128::from(b - b'0'));
    }
    Ok(if negative { -value } else { value })
}

/// Emulates `int.Parse(s, CultureInfo.InvariantCulture)`.
pub fn net_parse_i32(s: &str) -> Result<i32, NumberError> {
    let v = net_parse_integer(s)?;
    i32::try_from(v).map_err(|_| NumberError::Overflow)
}

/// Emulates `byte.Parse(s)` (`-0` is accepted, other negatives overflow).
pub fn net_parse_u8(s: &str) -> Result<u8, NumberError> {
    let v = net_parse_integer(s)?;
    u8::try_from(v).map_err(|_| NumberError::Overflow)
}

/// Port of `Parsing.ParseFloat`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Parsing.cs
pub fn parse_float(input: &str, parse_limit: f32, allow_nan: bool) -> Result<f32, NumberError> {
    let output = net_parse_f32(input)?;
    if output < -parse_limit || output > parse_limit {
        return Err(NumberError::Overflow);
    }
    if !allow_nan && output.is_nan() {
        return Err(NumberError::NotANumber);
    }
    Ok(output)
}

/// Port of `Parsing.ParseDouble`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Parsing.cs
pub fn parse_double(input: &str, parse_limit: f64, allow_nan: bool) -> Result<f64, NumberError> {
    let output = net_parse_f64(input)?;
    if output < -parse_limit || output > parse_limit {
        return Err(NumberError::Overflow);
    }
    if !allow_nan && output.is_nan() {
        return Err(NumberError::NotANumber);
    }
    Ok(output)
}

/// Port of `Parsing.ParseInt`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/Parsing.cs
pub fn parse_int(input: &str, parse_limit: i32) -> Result<i32, NumberError> {
    let output = net_parse_i32(input)?;
    // C# negation is unchecked, so `-int.MinValue` wraps.
    if output < parse_limit.wrapping_neg() || output > parse_limit {
        return Err(NumberError::Overflow);
    }
    Ok(output)
}

/// `Parsing.ParseFloat(input)` with the default limit and NaN rejected.
pub fn float(input: &str) -> Result<f32, NumberError> {
    parse_float(input, MAX_PARSE_VALUE as f32, false)
}

/// `Parsing.ParseDouble(input)` with the default limit and NaN rejected.
pub fn double(input: &str) -> Result<f64, NumberError> {
    parse_double(input, MAX_PARSE_VALUE, false)
}

/// `Parsing.ParseInt(input)` with the default limit.
pub fn int(input: &str) -> Result<i32, NumberError> {
    parse_int(input, i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_table() {
        let ok: &[(&str, f64)] = &[
            ("1", 1.0),
            ("  12.5  ", 12.5), // white space allowed
            ("\t-3\r\n", -3.0),
            ("+4", 4.0),
            (".5", 0.5), // .NET accepts leading '.'
            ("5.", 5.0), // and trailing '.'
            ("-.5", -0.5),
            ("1e3", 1000.0),
            ("1E+3", 1000.0),
            ("1.5e-2", 0.015),
            ("1,000", 1000.0), // AllowThousands
            ("1,,0", 10.0),    // consecutive separators are skipped
            ("1,000.5", 1000.5),
            ("0005", 5.0),
            ("5\0", 5.0), // trailing NULs accepted
            ("5 \0\0", 5.0),
        ];
        for (s, v) in ok {
            assert_eq!(net_parse_f64(s), Ok(*v), "input {s:?}");
        }
        let bad = [
            "", " ", ".", "-", "+", "e5", ".e5", "1e", "1e+", "1e5.5", "1.2.3", ",1", "1.5,5",
            "1e5,0", "- 5", "5-", "0x10", "1_0", "inf", "+inf", "nan(1)", "1 2", "5\0 5", "\0 5",
            "１２", "∞", "--5", "1d", "1f",
        ];
        for s in bad {
            assert_eq!(net_parse_f64(s), Err(NumberError::Format), "input {s:?}");
        }
    }

    #[test]
    fn double_special_values() {
        assert_eq!(net_parse_f64("Infinity"), Ok(f64::INFINITY));
        assert_eq!(net_parse_f64("infinity"), Ok(f64::INFINITY)); // case-insensitive
        assert_eq!(net_parse_f64("+INFINITY"), Ok(f64::INFINITY));
        assert_eq!(net_parse_f64("-Infinity"), Ok(f64::NEG_INFINITY));
        assert!(net_parse_f64("NaN").unwrap().is_nan());
        assert!(net_parse_f64(" nan ").unwrap().is_nan());
        assert!(net_parse_f64("-NaN").unwrap().is_nan());
        // Overflow becomes infinity (.NET Core 3.0+), later rejected by the limit.
        assert_eq!(net_parse_f64("1e999"), Ok(f64::INFINITY));
        assert_eq!(net_parse_f64("-1e999"), Ok(f64::NEG_INFINITY));
        assert_eq!(net_parse_f64("1e-999"), Ok(0.0));
        assert_eq!(net_parse_f64("1e99999999999999999999"), Ok(f64::INFINITY));
    }

    #[test]
    fn special_values_do_not_accept_trailing_nul() {
        // .NET's Infinity/NaN fallback trims white space only, unlike plain numbers.
        assert_eq!(net_parse_f64("NaN\0"), Err(NumberError::Format));
        assert_eq!(net_parse_f64("Infinity\0"), Err(NumberError::Format));
        assert!(net_parse_f64(" \tNaN\n").unwrap().is_nan());
        assert_eq!(net_parse_f64("5\0"), Ok(5.0));
    }

    #[test]
    fn parse_int_with_min_value_limit_does_not_panic() {
        // -i32::MIN wraps to i32::MIN in C#, so the range collapses to exactly i32::MIN.
        assert_eq!(parse_int("-2147483648", i32::MIN), Ok(i32::MIN));
        assert_eq!(parse_int("-5", i32::MIN), Err(NumberError::Overflow));
    }

    #[test]
    fn negative_zero_preserved() {
        assert!(net_parse_f64("-0").unwrap().is_sign_negative());
    }

    #[test]
    fn float_rounds_directly_to_f32() {
        // A decimal just above the midpoint of two f32 values: double rounding via f64 would
        // land on the midpoint and round to even, the direct parse rounds up.
        let s = "1.00000005960464477539062500000001";
        let direct = net_parse_f32(s).unwrap();
        assert_eq!(direct, f32::from_bits(0x3f80_0001));
        assert_ne!(s.parse::<f64>().unwrap() as f32, direct);
        assert_eq!(net_parse_f32("0.7"), Ok(0.7f32));
        assert_eq!(net_parse_f32("1e39"), Ok(f32::INFINITY));
    }

    #[test]
    fn int_table() {
        let ok: &[(&str, i32)] = &[
            ("0", 0),
            ("-0", 0),
            ("+7", 7),
            ("  42 ", 42),
            ("\t42\n", 42),
            ("007", 7),
            ("2147483647", i32::MAX),
            ("-2147483648", i32::MIN),
            ("5\0", 5),
        ];
        for (s, v) in ok {
            assert_eq!(net_parse_i32(s), Ok(*v), "input {s:?}");
        }
        for s in [
            "2147483648",
            "-2147483649",
            "99999999999999999999999999999999999999999999",
        ] {
            assert_eq!(net_parse_i32(s), Err(NumberError::Overflow), "input {s:?}");
        }
        for s in [
            "", " ", "+", "-", "1.0", "1e3", "1,000", "0x1", "1 2", "- 1", "٣",
        ] {
            assert_eq!(net_parse_i32(s), Err(NumberError::Format), "input {s:?}");
        }
    }

    #[test]
    fn byte_table() {
        assert_eq!(net_parse_u8(" 255 "), Ok(255));
        assert_eq!(net_parse_u8("0"), Ok(0));
        assert_eq!(net_parse_u8("-0"), Ok(0));
        assert_eq!(net_parse_u8("256"), Err(NumberError::Overflow));
        assert_eq!(net_parse_u8("-1"), Err(NumberError::Overflow));
        assert_eq!(net_parse_u8("1.0"), Err(NumberError::Format));
    }

    #[test]
    fn limits_and_nan() {
        assert_eq!(double("2147483647"), Ok(2147483647.0));
        assert_eq!(double("2147483648"), Err(NumberError::Overflow));
        assert_eq!(double("-2147483648"), Err(NumberError::Overflow));
        assert_eq!(double("Infinity"), Err(NumberError::Overflow));
        assert_eq!(double("-Infinity"), Err(NumberError::Overflow));
        assert_eq!(double("NaN"), Err(NumberError::NotANumber));
        assert!(parse_double("NaN", MAX_PARSE_VALUE, true).unwrap().is_nan());
        // The float limit is (float)int.MaxValue == 2^31.
        assert_eq!(float("2147483648"), Ok(2147483648.0));
        assert_eq!(float("2147483904"), Err(NumberError::Overflow));
        assert_eq!(float("NaN"), Err(NumberError::NotANumber));
        assert_eq!(int("2147483647"), Ok(i32::MAX));
        // int.MinValue is below -int.MaxValue.
        assert_eq!(int("-2147483648"), Err(NumberError::Overflow));
        assert_eq!(int("-2147483647"), Ok(-i32::MAX));
        assert_eq!(parse_int("11", 10), Err(NumberError::Overflow));
        assert_eq!(MAX_COORDINATE_VALUE, 131072);
    }
}
