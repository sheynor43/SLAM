//! .NET runtime semantics that lazer's results depend on: NaN handling of `Math.Min`, `Math.Max`
//! and `Math.Clamp`, `double.CompareTo`, and the unstable introsort behind `List<T>.Sort`.
//!
//! Rust's `f64::min`/`max` drop NaNs and `f64::clamp` panics on a NaN bound, while .NET
//! propagates the NaN; ties of an unstable sort land in an order a stable sort does not
//! reproduce.

use std::cmp::Ordering;

/// .NET's `Math.Min(double, double)`: NaN if either argument is NaN, and -0 below 0.
pub(crate) fn min(a: f64, b: f64) -> f64 {
    if a != b {
        if !a.is_nan() {
            return if a < b { a } else { b };
        }
        return a;
    }
    if a.is_sign_negative() { a } else { b }
}

/// .NET's `Math.Max(double, double)`: NaN if either argument is NaN, and 0 above -0.
pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a != b {
        if !a.is_nan() {
            return if b < a { a } else { b };
        }
        return a;
    }
    if b.is_sign_negative() { a } else { b }
}

/// .NET's `Math.Clamp(double, double, double)` without its `min > max` exception: a NaN value
/// stays NaN, and a NaN bound is ignored.
pub(crate) fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// .NET's `Math.Clamp(float, float, float)`, see [`clamp`].
pub(crate) fn clamp_f32(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// .NET's `double.CompareTo`: NaN sorts before everything, and -0 equals 0.
pub(crate) fn compare_f64(a: f64, b: f64) -> Ordering {
    match a.partial_cmp(&b) {
        Some(ordering) => ordering,
        None => a.is_nan().cmp(&b.is_nan()).reverse(),
    }
}

// Ported from dotnet/runtime (.NET 10, the runtime of lazer 2026.1005.0-lazer): src/libraries/System.Private.CoreLib/src/System/Collections/Generic/ArraySortHelper.cs (IntrospectiveSort)
/// `List<T>.Sort(Comparison<T>)`: introsort, which does not keep the order of equal elements.
pub(crate) fn list_sort<T>(keys: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    if keys.len() > 1 {
        // A slice length fits in u32 for any realistic list; .NET lists are int-indexed.
        let depth_limit = 2 * (usize::BITS - 1 - keys.len().leading_zeros() + 1);
        intro_sort(keys, depth_limit, &mut compare);
    }
}

const INTROSORT_SIZE_THRESHOLD: usize = 16;

fn intro_sort<T>(
    mut keys: &mut [T],
    mut depth_limit: u32,
    compare: &mut impl FnMut(&T, &T) -> Ordering,
) {
    while keys.len() > 1 {
        let partition_size = keys.len();
        if partition_size <= INTROSORT_SIZE_THRESHOLD {
            if partition_size == 2 {
                swap_if_greater(keys, compare, 0, 1);
                return;
            }
            if partition_size == 3 {
                swap_if_greater(keys, compare, 0, 1);
                swap_if_greater(keys, compare, 0, 2);
                swap_if_greater(keys, compare, 1, 2);
                return;
            }
            insertion_sort(keys, compare);
            return;
        }

        if depth_limit == 0 {
            heap_sort(keys, compare);
            return;
        }
        depth_limit -= 1;

        let p = pick_pivot_and_partition(keys, compare);
        let (left, right) = keys.split_at_mut(p);
        intro_sort(&mut right[1..], depth_limit, compare);
        keys = left;
    }
}

fn swap_if_greater<T>(
    keys: &mut [T],
    compare: &mut impl FnMut(&T, &T) -> Ordering,
    i: usize,
    j: usize,
) {
    if compare(&keys[i], &keys[j]) == Ordering::Greater {
        keys.swap(i, j);
    }
}

fn pick_pivot_and_partition<T>(
    keys: &mut [T],
    compare: &mut impl FnMut(&T, &T) -> Ordering,
) -> usize {
    let hi = keys.len() - 1;

    // Median-of-three; the pivot then sits at hi - 1.
    let middle = hi >> 1;
    swap_if_greater(keys, compare, 0, middle);
    swap_if_greater(keys, compare, 0, hi);
    swap_if_greater(keys, compare, middle, hi);

    keys.swap(middle, hi - 1);
    let pivot = hi - 1;
    let mut left = 0;
    let mut right = hi - 1;

    while left < right {
        loop {
            left += 1;
            if compare(&keys[left], &keys[pivot]) != Ordering::Less {
                break;
            }
        }
        loop {
            right -= 1;
            if compare(&keys[pivot], &keys[right]) != Ordering::Less {
                break;
            }
        }

        if left >= right {
            break;
        }
        keys.swap(left, right);
    }

    if left != hi - 1 {
        keys.swap(left, hi - 1);
    }
    left
}

fn heap_sort<T>(keys: &mut [T], compare: &mut impl FnMut(&T, &T) -> Ordering) {
    let n = keys.len();
    let mut i = n >> 1;
    while i >= 1 {
        down_heap(keys, i, n, compare);
        i -= 1;
    }

    let mut i = n;
    while i > 1 {
        keys.swap(0, i - 1);
        down_heap(keys, 1, i - 1, compare);
        i -= 1;
    }
}

/// Sifts the element at 1-based index `i` down a heap of `n` elements. .NET holds the element
/// aside and moves children up; swapping along the way puts every element in the same place.
fn down_heap<T>(
    keys: &mut [T],
    mut i: usize,
    n: usize,
    compare: &mut impl FnMut(&T, &T) -> Ordering,
) {
    while i <= n >> 1 {
        let mut child = 2 * i;
        if child < n && compare(&keys[child - 1], &keys[child]) == Ordering::Less {
            child += 1;
        }

        if compare(&keys[i - 1], &keys[child - 1]) != Ordering::Less {
            break;
        }

        keys.swap(i - 1, child - 1);
        i = child;
    }
}

fn insertion_sort<T>(keys: &mut [T], compare: &mut impl FnMut(&T, &T) -> Ordering) {
    for i in 0..keys.len() - 1 {
        // .NET holds keys[i + 1] aside and shifts greater elements right; swapping it down
        // compares the same pairs.
        let mut j = i + 1;
        while j > 0 && compare(&keys[j], &keys[j - 1]) == Ordering::Less {
            keys.swap(j, j - 1);
            j -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_max_propagate_nan_and_order_zeros() {
        assert!(min(f64::NAN, 1.0).is_nan());
        assert!(min(1.0, f64::NAN).is_nan());
        assert!(max(f64::NAN, 1.0).is_nan());
        assert!(max(1.0, f64::NAN).is_nan());
        assert!(min(0.0, -0.0).is_sign_negative());
        assert!(min(-0.0, 0.0).is_sign_negative());
        assert!(max(0.0, -0.0).is_sign_positive());
        assert!(max(-0.0, 0.0).is_sign_positive());
        assert_eq!(min(2.0, 3.0), 2.0);
        assert_eq!(max(2.0, 3.0), 3.0);
    }

    #[test]
    fn clamp_keeps_nan_value_and_ignores_nan_bound() {
        assert!(clamp(f64::NAN, 0.0, 1.0).is_nan());
        assert_eq!(clamp(5.0, 0.0, f64::NAN), 5.0);
        assert_eq!(clamp(f64::INFINITY, 0.0, 3.0), 3.0);
        assert_eq!(clamp(-1.0, 0.0, 3.0), 0.0);
    }

    /// `List<T>.Sort` runs recorded by .NET (see `tools/lazer-refgen`): keys with many ties
    /// and adversarial inputs that exhaust the depth limit (heapsort). Each case gives the
    /// original indices in sorted order and a hash of every compared pair, so the comparison
    /// path itself must match.
    #[test]
    fn list_sort_matches_dotnet() {
        const FIXTURE: &str = include_str!("../tests/data/lazer-slider-nested.txt");
        let numbers = |line: &str| -> Vec<u64> {
            line.split(' ')
                .skip(1)
                .filter(|f| !f.is_empty())
                .map(|f| f.parse().expect("number"))
                .collect()
        };

        let mut lines = FIXTURE.lines().filter(|l| {
            l.starts_with("sort ") || l.starts_with("sorted") || l.starts_with("compares ")
        });
        let mut cases = 0;
        while let (Some(keys), Some(sorted), Some(compares)) =
            (lines.next(), lines.next(), lines.next())
        {
            assert!(keys.starts_with("sort ") && sorted.starts_with("sorted"));
            let keys = numbers(keys);
            let expected: Vec<usize> = numbers(sorted).into_iter().map(|i| i as usize).collect();
            let compares: Vec<&str> = compares.split(' ').collect();

            let mut items: Vec<(f64, usize)> = keys
                .iter()
                .enumerate()
                .map(|(i, &k)| (k as f64, i))
                .collect();
            let mut compared = String::new();
            let mut count = 0;
            list_sort(&mut items, |a, b| {
                compared.push_str(&format!("{},{};", a.1, b.1));
                count += 1;
                compare_f64(a.0, b.0)
            });

            let actual: Vec<usize> = items.iter().map(|&(_, i)| i).collect();
            assert_eq!(actual, expected, "keys {keys:?}");
            assert_eq!(count.to_string(), compares[1], "comparisons for {keys:?}");
            assert_eq!(
                format!("{:016x}", fnv(&compared)),
                compares[2],
                "comparison path for {keys:?}"
            );
            cases += 1;
        }
        assert_eq!(cases, 203);
    }

    fn fnv(text: &str) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    #[test]
    fn list_sort_sorts() {
        let mut v: Vec<i32> = (0..200).map(|i| (i * 7919) % 101).collect();
        list_sort(&mut v, |a, b| a.cmp(b));
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }
}
