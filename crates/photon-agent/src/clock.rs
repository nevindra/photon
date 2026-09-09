//! Wall-clock helpers. Two concerns live here because both are "what time is it, really":
//!
//! - `Skew`: the offset between this host's clock and the server's (R9.3). Every sync response
//!   carries `server_time_ms`; the agent keeps `server − local` and applies it to every
//!   timestamp it SENDS (action/stack results, events, log lines), so a customer box whose clock
//!   is 40 minutes off still lands its results at the right place on the server's timeline.
//!   Nothing local is ever adjusted — Docker's own timestamps are re-based, not rewritten.
//! - `parse_rfc3339_ms`: Docker reports times as RFC 3339 strings with nanoseconds
//!   (`2025-09-07T10:11:12.123456789Z`); bollard hands them over as plain `String`s (its
//!   `time`/`chrono` features are off), and the log stream prefixes every line with the same
//!   shape. One small parser, no date crate.
use std::sync::atomic::{AtomicI64, Ordering};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `server_time − local_time`, in ms. Zero until the first sync answers.
#[derive(Debug, Default)]
pub struct Skew {
    offset_ms: AtomicI64,
}

impl Skew {
    /// Record a server timestamp against the local clock at the moment it arrived. Half the
    /// round trip is folded in by the caller passing the local time at mid-request.
    pub fn observe(&self, server_time_ms: i64, local_ms: i64) {
        self.offset_ms
            .store(server_time_ms - local_ms, Ordering::Relaxed);
    }
    pub fn offset_ms(&self) -> i64 {
        self.offset_ms.load(Ordering::Relaxed)
    }
    /// A local timestamp re-based onto the server's clock.
    pub fn adjust(&self, local_ms: i64) -> i64 {
        local_ms + self.offset_ms()
    }
    pub fn now(&self) -> i64 {
        self.adjust(now_ms())
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]` → epoch ms. Docker's zero time (`0001-01-01T…`,
/// what `FinishedAt` reads while a container is still running) comes back `None`, as does
/// anything that does not parse — a caller treating an unset time as 0 would draw a container
/// that finished in the year 1.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)? as u32, num(8, 10)? as u32);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if y <= 1 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60
    {
        return None;
    }
    let mut rest = &s[19..];
    let mut frac_ms = 0i64;
    if let Some(r) = rest.strip_prefix('.') {
        let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        let digits = &r[..end];
        // Only the first three fractional digits are milliseconds.
        let mut ms = 0i64;
        for (i, c) in digits.bytes().take(3).enumerate() {
            let _ = i;
            ms = ms * 10 + (c - b'0') as i64;
        }
        for _ in digits.len().min(3)..3 {
            ms *= 10;
        }
        frac_ms = ms;
        rest = &r[end..];
    }
    let offset_secs = match rest {
        "" | "Z" | "z" => 0,
        tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let th: i64 = tz[1..3].parse().ok()?;
            let tm: i64 = tz[4..6].parse().ok()?;
            sign * (th * 3600 + tm * 60)
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_secs;
    Some(secs * 1000 + frac_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_timestamps() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T00:00:00Z"),
            Some(1_704_067_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T00:00:00.123456789Z"),
            Some(1_704_067_200_123)
        );
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T00:00:00.5Z"),
            Some(1_704_067_200_500)
        );
        assert_eq!(
            parse_rfc3339_ms("2024-01-01T02:00:00+02:00"),
            Some(1_704_067_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2023-12-31T22:00:00-02:00"),
            Some(1_704_067_200_000)
        );
        // Docker's zero time is "not set", not "the year 1".
        assert_eq!(parse_rfc3339_ms("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_ms(""), None);
        assert_eq!(parse_rfc3339_ms("not a time"), None);
        assert_eq!(parse_rfc3339_ms("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn skew_rebases_local_time() {
        let s = Skew::default();
        assert_eq!(s.adjust(1_000), 1_000);
        s.observe(10_000, 7_000);
        assert_eq!(s.offset_ms(), 3_000);
        assert_eq!(s.adjust(1_000), 4_000);
        s.observe(5_000, 7_000);
        assert_eq!(s.adjust(1_000), -1_000);
    }
}
