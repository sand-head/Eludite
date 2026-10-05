//! RFC 3339 timestamps from `SystemTime`, with no date dependency.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Now, as `2026-10-05T12:34:56Z`.
pub fn now_rfc3339() -> String {
    rfc3339(SystemTime::now())
}

/// `t` as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` (what [`rfc3339`] writes and GitHub's `published_at`) to a `SystemTime`.
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let days = days_from_civil(y, m, d);
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

// Howard Hinnant's algorithms.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip() {
        let t = UNIX_EPOCH + Duration::from_secs(1_791_203_696);
        let s = rfc3339(t);
        assert_eq!(s, "2026-10-05T12:34:56Z");
        assert_eq!(parse_rfc3339(&s), Some(t));
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            parse_rfc3339("2000-02-29T00:00:00Z")
                .map(rfc3339)
                .as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(parse_rfc3339("garbage"), None);
    }
}
