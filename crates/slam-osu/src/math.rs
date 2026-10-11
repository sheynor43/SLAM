//! Transcendental functions that gameplay results depend on (ADR-0031).
//!
//! Every `sin`, `cos`, `atan2`, `acos` and `pow` of the rules goes through this module. The
//! platform libm behind `f64::sin` and friends differs between systems in the last bits, so the
//! results here are correctly rounded instead (`pxfm`): unique, the same on every platform and
//! CPU, and what each platform's libm (which lazer calls through .NET's `Math`) returns for
//! nearly every input. Special values follow C99 Annex F, as .NET's `Math` does through the C
//! runtime; the wrappers fix the few cases where `pxfm` 0.1.30 does not.

/// .NET's `Math.Sin(double)`, correctly rounded.
pub(crate) fn sin(x: f64) -> f64 {
    pxfm::f_sin(x)
}

/// .NET's `Math.Cos(double)`, correctly rounded.
pub(crate) fn cos(x: f64) -> f64 {
    pxfm::f_cos(x)
}

/// .NET's `Math.Atan2(double, double)`, correctly rounded.
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    let r = pxfm::f_atan2(y, x);
    // pxfm returns +0 when a negative result underflows (y = -1e-300, x = 1e300); the result
    // takes the sign of y.
    if r == 0.0 { r.copysign(y) } else { r }
}

/// .NET's `Math.Acos(double)`, correctly rounded.
pub(crate) fn acos(x: f64) -> f64 {
    pxfm::f_acos(x)
}

/// .NET's `MathF.Atan2(float, float)`, correctly rounded.
pub(crate) fn atan2f(y: f32, x: f32) -> f32 {
    pxfm::f_atan2f(y, x)
}

/// .NET's `Math.Pow(double, double)`, correctly rounded.
pub(crate) fn pow(x: f64, y: f64) -> f64 {
    // pow(1, y) and pow(x, ±0) are 1 even for a NaN argument; pxfm returns NaN.
    if x == 1.0 || y == 0.0 {
        return 1.0;
    }
    // pxfm's x^-0.5 overflows into NaN for x above ~1e300 without FMA (Dekker's split of x),
    // while its FMA path is correct. Scaling by an even power of two is exact on both sides:
    // x^-0.5 = (x * 2^-128)^-0.5 * 2^-64, and the result stays normal. The rules only raise to
    // 0.5 and 5 for now; this keeps the function correct for any later caller.
    if y == -0.5 && x >= TWO_POW_960 && x.is_finite() {
        return pxfm::f_pow(x * TWO_POW_MINUS_128, -0.5) * TWO_POW_MINUS_64;
    }
    pxfm::f_pow(x, y)
}

const TWO_POW_960: f64 = f64::from_bits((1023 + 960) << 52);
const TWO_POW_MINUS_128: f64 = f64::from_bits((1023 - 128) << 52);
const TWO_POW_MINUS_64: f64 = f64::from_bits((1023 - 64) << 52);

#[cfg(test)]
mod tests {
    use super::*;

    /// Correctly rounded results computed with MPFR: every input from this crate's tests where
    /// glibc 2.44 is not correctly rounded, and samples of test and random inputs.
    const VECTORS: &str = include_str!("../tests/data/cr-math-vectors.txt");

    fn eval(name: &str, a: u64, b: u64) -> u64 {
        let (a, b) = (f64::from_bits(a), f64::from_bits(b));
        match name {
            "sin" => sin(a).to_bits(),
            "cos" => cos(a).to_bits(),
            "acos" => acos(a).to_bits(),
            // atan2(y, x), pow(x, y): arguments in .NET order.
            "atan2" => atan2(a, b).to_bits(),
            "pow" => pow(a, b).to_bits(),
            "atan2f" => {
                let (y, x) = (
                    f32::from_bits(a.to_bits() as u32),
                    f32::from_bits(b.to_bits() as u32),
                );
                atan2f(y, x).to_bits().into()
            }
            _ => panic!("unknown function {name}"),
        }
    }

    #[test]
    fn results_are_correctly_rounded() {
        let mut cases = 0;
        let mut failures = Vec::new();
        for line in VECTORS.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split(' ').collect();
            let hex = |s: &str| u64::from_str_radix(s, 16).unwrap();
            let (a, b, expected) = (hex(f[1]), hex(f[2]), hex(f[3]));
            let actual = eval(f[0], a, b);
            if actual != expected {
                failures.push(format!("{} {a:x} {b:x}: {actual:x} != {expected:x}", f[0]));
            }
            cases += 1;
        }
        assert!(cases > 3000, "{cases} vectors");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    /// C99 Annex F special values, which .NET's `Math` returns through the C runtime.
    #[test]
    fn special_values_follow_c99() {
        let (inf, nan) = (f64::INFINITY, f64::NAN);
        let pi = std::f64::consts::PI;
        let half_pi = std::f64::consts::FRAC_PI_2;
        let cases = [
            (sin(0.0), 0.0),
            (sin(-0.0), -0.0),
            (sin(inf), nan),
            (sin(nan), nan),
            (cos(0.0), 1.0),
            (cos(-inf), nan),
            (cos(nan), nan),
            (acos(nan), nan),
            (acos(1.0), 0.0),
            (acos(-1.0), pi),
            (acos(1.0000000000000002), nan),
            (atan2(0.0, 0.0), 0.0),
            (atan2(-0.0, 0.0), -0.0),
            (atan2(0.0, -0.0), pi),
            (atan2(-0.0, -0.0), -pi),
            (atan2(1.0, 0.0), half_pi),
            (atan2(-1.0, inf), -0.0),
            (atan2(1.0, -inf), pi),
            (atan2(-1e-300, 1e300), -0.0),
            (atan2(1e-300, 1e300), 0.0),
            (atan2(nan, 1.0), nan),
            (pow(1.0, nan), 1.0),
            (pow(nan, 0.0), 1.0),
            (pow(nan, -0.0), 1.0),
            (pow(-1.0, inf), 1.0),
            (pow(-1.0, -inf), 1.0),
            (pow(0.0, -1.0), inf),
            (pow(-0.0, -1.0), -inf),
            (pow(-0.0, 0.5), 0.0),
            (pow(-8.0, 1.0 / 3.0), nan),
            (pow(-2.0, 3.0), -8.0),
            (pow(inf, -0.5), 0.0),
            (pow(-inf, -0.5), 0.0),
            (pow(-0.0, -0.5), inf),
            (pow(-1e300, -0.5), nan),
            (pow(2.0, nan), nan),
        ];
        for (i, (actual, expected)) in cases.into_iter().enumerate() {
            assert!(
                same(actual, expected),
                "case {i}: {actual:e} != {expected:e}"
            );
        }
        assert_eq!(atan2f(-0.0, 0.0).to_bits(), (-0.0f32).to_bits());
        assert_eq!(atan2f(0.0, -1.0), std::f32::consts::PI);
        assert!(atan2f(f32::NAN, 1.0).is_nan());
    }

    /// pxfm's `pow(x, -0.5)` without FMA turns NaN for x above ~1e300.
    #[test]
    fn pow_of_huge_numbers_to_minus_one_half() {
        // Results from MPFR.
        let cases = [
            (5.258431155957536e+301, 0x209c_e3e0_7dbb_3c5d_u64),
            (5.227607469587421e+306, 0x2017_74e8_124b_f34f),
            (f64::MAX, 0x1ff0_0000_0000_0000),
        ];
        for (x, expected) in cases {
            let r = pow(x, -0.5);
            assert_eq!(r.to_bits(), expected, "pow({x:e}, -0.5) = {r:e}");
        }
        assert_eq!(pow(TWO_POW_960, -0.5), f64::from_bits((1023 - 480) << 52));
        // Just below the threshold the wrapper leaves the call to pxfm.
        let below = f64::from_bits(TWO_POW_960.to_bits() - 1);
        let r = pow(below, -0.5);
        assert_eq!(r.to_bits(), pxfm::f_pow(below, -0.5).to_bits());
        // 2^-480 * (1 + 2^-54 + ...), below half an ulp: rounds to 2^-480.
        assert_eq!(r, f64::from_bits((1023 - 480) << 52));
    }

    /// The correctly rounded square root and `pow(x, 0.5)` agree; glibc's `pow` does not at
    /// 2921 (lazer's `Math.Pow` on Linux).
    #[test]
    fn pow_one_half_is_the_square_root() {
        for x in [2.0, 2921.0, 12345.0, 1e-300, 1e300] {
            assert_eq!(pow(x, 0.5).to_bits(), x.sqrt().to_bits(), "{x}");
        }
    }
}
