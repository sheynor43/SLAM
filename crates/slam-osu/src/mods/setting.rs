//! Mod setting values as lazer reads them: `Bindable<T>.Parse` (through `Convert.ChangeType`
//! with the invariant culture) on the value Newtonsoft deserialised from the score JSON, then
//! the bindable's own clamping and precision rounding.
//!
//! Every function returns `None` where lazer throws; `APIMod.ToMod` catches the exception and
//! keeps the setting's default.

use slam_formats::osr::SettingValue;

/// .NET's `IsWhite` (space and `\t`..=`\r`), the white space that number parsing skips.
fn is_white(c: char) -> bool {
    c == ' ' || ('\t'..='\r').contains(&c)
}

/// The trimming of .NET's number parsing: leading white space, then at the end white space
/// followed by NULs (a NUL before trailing white space is not skipped).
fn trim_number(s: &str) -> &str {
    s.trim_start_matches(is_white)
        .trim_end_matches('\0')
        .trim_end_matches(is_white)
}

/// `Convert.ChangeType(input, typeof(double), InvariantCulture)`.
///
/// Strings go through `double.Parse` with `NumberStyles.Float | AllowThousands`: surrounding
/// white space, a sign, group separators in the integer part, an exponent, and the symbols
/// `Infinity`, `-Infinity` and `NaN` (case-insensitive). Unusual spellings that .NET also
/// accepts (`∞`, separators in odd places) are rejected; lazer never writes numbers as strings.
pub(crate) fn to_f64(value: &SettingValue) -> Option<f64> {
    match value {
        SettingValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        SettingValue::Int(i) => Some(*i as f64),
        SettingValue::Float(f) => Some(*f),
        SettingValue::String(s) => parse_f64(s),
        // `null` throws for a non-nullable bindable; arrays, objects and integers beyond `long`
        // (`BigInteger`) are not `IConvertible`.
        SettingValue::Null | SettingValue::Raw(_) => None,
    }
}

fn split_sign(s: &str) -> (f64, &str) {
    match s.as_bytes().first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    }
}

fn parse_f64(s: &str) -> Option<f64> {
    // The fallback for the symbols trims with `string.Trim()`: white space, but no NULs.
    let (sign, symbol) = split_sign(s.trim());
    if symbol.eq_ignore_ascii_case("infinity") {
        return Some(sign * f64::INFINITY);
    }
    // A signed NaN symbol is NaN as well.
    if symbol.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN);
    }

    let (sign, body) = split_sign(trim_number(s));
    if body.starts_with(['+', '-']) {
        return None;
    }

    // Group separators are allowed between the digits of the integer part only.
    let int_end = body.find(['.', 'e', 'E']).unwrap_or(body.len());
    let (int_part, rest) = body.split_at(int_end);
    if int_part.starts_with(',') {
        return None;
    }
    let mut text = String::with_capacity(body.len());
    text.extend(int_part.chars().filter(|&c| c != ','));
    text.push_str(rest);

    let digits_ok = text
        .bytes()
        .all(|b| b.is_ascii_digit() || b"eE.+-".contains(&b));
    let has_digit = text
        .bytes()
        .take_while(|&b| b != b'e' && b != b'E')
        .any(|b| b.is_ascii_digit());
    if !digits_ok || !has_digit {
        return None;
    }
    text.parse::<f64>().ok().map(|v| sign * v)
}

/// `Convert.ChangeType(input, typeof(bool), InvariantCulture)`. A `BindableBool` additionally
/// accepts the strings `"1"` and `"0"` (`digit_strings`); a plain `Bindable<bool>` does not.
pub(crate) fn to_bool(value: &SettingValue, digit_strings: bool) -> Option<bool> {
    match value {
        SettingValue::Bool(b) => Some(*b),
        SettingValue::Int(i) => Some(*i != 0),
        // NaN is not zero.
        SettingValue::Float(f) => Some(*f != 0.0),
        SettingValue::String(s) => {
            if digit_strings && s == "1" {
                return Some(true);
            }
            if digit_strings && s == "0" {
                return Some(false);
            }
            // `bool.Parse`: case-insensitive, white space and NULs around it ignored.
            let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
            if t.eq_ignore_ascii_case("true") {
                Some(true)
            } else if t.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        }
        SettingValue::Null | SettingValue::Raw(_) => None,
    }
}

/// `Convert.ChangeType(input, typeof(int), InvariantCulture)`: an out-of-range integer throws,
/// a double is rounded half to even (and throws outside the `int` range or when NaN), a string
/// goes through `int.Parse` with `NumberStyles.Integer`.
pub(crate) fn to_i32(value: &SettingValue) -> Option<i32> {
    match value {
        SettingValue::Bool(b) => Some(i32::from(*b)),
        SettingValue::Int(i) => i32::try_from(*i).ok(),
        SettingValue::Float(f) => {
            // Convert.ToInt32(double)
            if *f >= -2_147_483_648.5 && *f < 2_147_483_647.5 {
                Some(f.round_ties_even() as i32)
            } else {
                None
            }
        }
        SettingValue::String(s) => trim_number(s).parse::<i32>().ok(),
        SettingValue::Null | SettingValue::Raw(_) => None,
    }
}

/// .NET's `s_doublePowers10` (the exact doubles `1e0..=1e28` used here).
const DOUBLE_POWERS_10: [f64; 29] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28,
];

/// `DEC_SCALE_MAX`: the largest scale of a `decimal`.
const DEC_SCALE_MAX: i32 = 28;

// Ported from dotnet/runtime (.NET 10, the runtime of lazer 2026.1005.0-lazer): src/libraries/System.Private.CoreLib/src/System/Decimal.DecCalc.cs (VarDecFromR8)
// Written without a local copy of the runtime sources; `tests/mods_lazer.rs` checks the
// rounding it leads to against .NET 10 on every thousandth of the setting ranges, their
// neighbouring doubles and random values.
/// The explicit `double` → `decimal` conversion: the magnitude is rounded to 15 significant
/// digits through a double multiplication, which is not always the exactly rounded value.
/// Returns `(negative, mantissa, scale)` with `value = mantissa * 10^-scale` (a negative scale
/// multiplies), or `None` for NaN, infinities and magnitudes from about 2^96, which
/// `decimal.CreateTruncating` (the caller) handles itself.
///
/// For the clamped speed settings only the branches with `power` 14 and 15 run; the fixture
/// of `tests/mods_lazer.rs` covers nothing else.
fn decimal_from_f64(input: f64) -> Option<(bool, u64, i32)> {
    const DBLBIAS: i32 = 1022;
    if !input.is_finite() {
        return None;
    }
    let exp = ((input.to_bits() >> 52) & 0x7ff) as i32 - DBLBIAS;
    if exp < -94 {
        return Some((false, 0, 0));
    }
    if exp > 96 {
        return None;
    }

    let negative = input < 0.0;
    let mut dbl = input.abs();

    // Calculate max power of 10 input value could have by multiplying the exponent by
    // log10(2), using scaled integer multiplication.
    let mut power = 14 - ((exp * 19728) >> 16);
    if power >= 0 {
        if power > DEC_SCALE_MAX {
            power = DEC_SCALE_MAX;
        }
        dbl *= DOUBLE_POWERS_10[power as usize];
    } else if power != -1 || dbl >= 1e15 {
        dbl /= DOUBLE_POWERS_10[(-power) as usize];
    } else {
        power = 0;
    }

    if dbl < 1e14 && power < DEC_SCALE_MAX {
        dbl *= 10.0;
        power += 1;
    }

    // Round to an integer, half to even (both code paths of the runtime do this).
    let mant = dbl.round_ties_even() as u64;
    if mant == 0 {
        return Some((false, 0, 0));
    }
    Some((negative, mant, power))
}

// Ported from osu-framework 2026.921.1: osu.Framework/Bindables/BindableNumber.cs (setValue)
// Ported from osu-framework 2026.921.1: osu.Framework/Bindables/RangeConstrainedBindable.cs (setValue)
/// `BindableDouble.Value = value` with `MinValue = min`, `MaxValue = max` and
/// `Precision = 10^-decimals`: clamp, convert to `decimal`, `Math.Round(value / precision)`
/// (half to even) and multiply back, convert to `double`, and clamp again (the setter of
/// `RangeConstrainedBindable` that stores the result).
///
/// NaN survives the first clamp, `decimal.CreateTruncating` turns it into 0, and the second
/// clamp makes that `min`. `None` where lazer throws and keeps the old value: a magnitude
/// beyond `decimal` saturates there and the division by the precision overflows (only
/// reachable with bounds far beyond lazer's speed settings).
pub(crate) fn set_bindable_double(value: f64, min: f64, max: f64, decimals: u32) -> Option<f64> {
    let clamped = crate::dotnet::clamp(value, min, max);
    if clamped.is_nan() {
        return Some(crate::dotnet::clamp(0.0, min, max));
    }
    let (negative, mant, scale) = decimal_from_f64(clamped)?;

    // value / 10^-decimals = mant * 10^(decimals - scale), exact in `decimal`.
    let shift = decimals as i32 - scale;
    let rounded: u128 = if shift >= 0 {
        let quotient = u128::from(mant) * 10u128.pow(shift as u32);
        // The 96-bit mantissa of `decimal` overflows: the division throws.
        if quotient >= 1u128 << 96 {
            return None;
        }
        quotient
    } else {
        let div = 10u128.pow((-shift) as u32);
        let q = u128::from(mant) / div;
        let r = u128::from(mant) % div;
        // Math.Round(decimal): MidpointRounding.ToEven
        if r * 2 > div || (r * 2 == div && q % 2 == 1) {
            q + 1
        } else {
            q
        }
    };

    // `rounded * precision` is the decimal `rounded` with scale `decimals`. `VarR8FromDec`
    // converts it as `(double)Low64 (+ High * 2^64) / s_doublePowers10[scale]`.
    let low = rounded as u64;
    let high = (rounded >> 64) as u64;
    let mut magnitude = low as f64;
    if high != 0 {
        magnitude += high as f64 * 18_446_744_073_709_551_616.0;
    }
    let result = magnitude / DOUBLE_POWERS_10[decimals as usize];
    let result = if negative { -result } else { result };
    Some(crate::dotnet::clamp(result, min, max))
}

// Ported from osu-framework 2026.921.1: osu.Framework/Bindables/BindableNumber.cs (setValue)
/// `BindableInt.Value = value` with the given bounds (the default precision 1 does not change
/// an integer).
pub(crate) fn set_bindable_int(value: i32, min: i32, max: i32) -> i32 {
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> SettingValue {
        SettingValue::String(text.to_owned())
    }

    #[test]
    fn doubles_convert_like_dotnet() {
        assert_eq!(to_f64(&SettingValue::Int(2)), Some(2.0));
        assert_eq!(to_f64(&SettingValue::Bool(true)), Some(1.0));
        assert_eq!(to_f64(&SettingValue::Float(1.25)), Some(1.25));
        assert_eq!(to_f64(&s(" 1.5 ")), Some(1.5));
        assert_eq!(to_f64(&s("-1,000.5e-3")), Some(-1.0005));
        assert_eq!(to_f64(&s("Infinity")), Some(f64::INFINITY));
        assert_eq!(to_f64(&s("-infinity")), Some(f64::NEG_INFINITY));
        assert!(to_f64(&s("NaN")).unwrap().is_nan());
        assert_eq!(to_f64(&s("")), None);
        assert_eq!(to_f64(&s("1.5x")), None);
        assert_eq!(to_f64(&s(",5")), None);
        assert_eq!(to_f64(&s("--1")), None);
        assert_eq!(to_f64(&s("-+1.5")), None);
        assert!(to_f64(&s("-NaN")).unwrap().is_nan());
        assert_eq!(to_f64(&s("1.5 \0")), Some(1.5));
        assert_eq!(to_f64(&s("1.5\0 ")), None);
        assert_eq!(to_f64(&s("NaN\0")), None);
        assert_eq!(to_f64(&s(" Infinity ")), Some(f64::INFINITY));
        assert_eq!(to_f64(&SettingValue::Null), None);
    }

    #[test]
    fn bools_convert_like_dotnet() {
        assert_eq!(to_bool(&s("1"), true), Some(true));
        assert_eq!(to_bool(&s("0"), true), Some(false));
        assert_eq!(to_bool(&s("1"), false), None);
        assert_eq!(to_bool(&s(" TRUE "), false), Some(true));
        assert_eq!(to_bool(&SettingValue::Int(-3), false), Some(true));
        assert_eq!(to_bool(&SettingValue::Float(f64::NAN), false), Some(true));
        assert_eq!(to_bool(&SettingValue::Float(0.0), false), Some(false));
        assert_eq!(to_bool(&SettingValue::Null, true), None);
    }

    #[test]
    fn ints_convert_like_dotnet() {
        assert_eq!(to_i32(&SettingValue::Float(2.5)), Some(2));
        assert_eq!(to_i32(&SettingValue::Float(3.5)), Some(4));
        assert_eq!(
            to_i32(&SettingValue::Float(-2_147_483_648.5)),
            Some(i32::MIN)
        );
        assert_eq!(to_i32(&SettingValue::Float(2_147_483_647.5)), None);
        assert_eq!(to_i32(&SettingValue::Float(f64::NAN)), None);
        assert_eq!(to_i32(&SettingValue::Int(1 << 40)), None);
        assert_eq!(to_i32(&s(" +7 ")), Some(7));
        assert_eq!(to_i32(&s("7.0")), None);
        assert_eq!(set_bindable_int(11, 0, 10), 10);
    }

    #[test]
    fn precision_rounds_through_decimal() {
        // 1.015 is 1.01499999999999990... as a double, but becomes the decimal 1.015 and then
        // rounds half to even.
        assert_eq!(set_bindable_double(1.015, 1.01, 2.0, 2), Some(1.02));
        assert_eq!(set_bindable_double(1.025, 1.01, 2.0, 2), Some(1.02));
        assert_eq!(set_bindable_double(1.5, 1.01, 2.0, 2), Some(1.5));
        assert_eq!(set_bindable_double(5.0, 1.01, 2.0, 2), Some(2.0));
        assert_eq!(set_bindable_double(f64::INFINITY, 0.5, 0.99, 2), Some(0.99));
        assert_eq!(
            set_bindable_double(f64::NEG_INFINITY, 0.5, 0.99, 2),
            Some(0.5)
        );
        // NaN becomes the decimal 0, which the second clamp raises to the minimum.
        assert_eq!(set_bindable_double(f64::NAN, 0.5, 0.99, 2), Some(0.5));
        // Beyond `decimal`: lazer throws and keeps the old value.
        assert_eq!(set_bindable_double(1e300, f64::MIN, f64::MAX, 2), None);
        assert_eq!(set_bindable_double(1e27, f64::MIN, f64::MAX, 2), None);
    }
}
