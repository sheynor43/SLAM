//! The wall clock as the player reads it: the results' "played on" date (ADR-0161).

use std::time::{SystemTime, UNIX_EPOCH};

/// `2026-11-20 21:55:12` in the local time zone, `2026-11-20 18:55:12 UTC` where the zone
/// is not known.
pub fn now_text() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    match utc_offset(seconds) {
        Some(offset) => format_seconds(seconds + offset),
        None => format!("{} UTC", format_seconds(seconds)),
    }
}

/// `seconds` since the epoch as `YYYY-MM-DD hh:mm:ss`.
pub fn format_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let in_day = seconds.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        in_day / 3600,
        in_day / 60 % 60,
        in_day % 60
    )
}

/// The local zone's offset from UTC at `seconds`, in seconds.
#[cfg(target_os = "linux")]
fn utc_offset(seconds: i64) -> Option<i64> {
    let time: libc::time_t = seconds;
    // SAFETY: `tm` is plain data that `localtime_r` fills; both pointers are valid for the
    // call, and `localtime_r` is the thread-safe form.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }
        Some(tm.tm_gmtoff)
    }
}

/// The local zone's offset from UTC now, in seconds.
#[cfg(windows)]
fn utc_offset(_seconds: i64) -> Option<i64> {
    use windows_sys::Win32::System::Time::{
        GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
    };
    /// `TIME_ZONE_ID_DAYLIGHT` of `winnt.h`: daylight saving time is in effect.
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    // SAFETY: `info` is plain data that the call fills; the pointer is valid for it.
    unsafe {
        let mut info: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let id = GetTimeZoneInformation(&mut info);
        if id == TIME_ZONE_ID_INVALID {
            return None;
        }
        let mut bias = info.Bias;
        if id == TIME_ZONE_ID_DAYLIGHT {
            bias += info.DaylightBias;
        }
        // The bias is UTC minus local, in minutes.
        Some(-i64::from(bias) * 60)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn utc_offset(_seconds: i64) -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_on_a_calendar() {
        assert_eq!(format_seconds(0), "1970-01-01 00:00:00");
        // 2026-11-20 21:55:12 UTC.
        assert_eq!(format_seconds(1_795_211_712), "2026-11-20 21:55:12");
        // A leap day.
        assert_eq!(format_seconds(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn now_has_a_date_and_a_time() {
        let text = now_text();
        assert!(text.len() >= 19, "{text}");
        assert_eq!(&text[4..5], "-");
    }
}
