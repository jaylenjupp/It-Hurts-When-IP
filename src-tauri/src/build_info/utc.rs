// UTC timestamp formatting for the build stamp.
//
// Shared by two compilation units: `build.rs` pulls this file in with
// `#[path = "src/build_info/utc.rs"] mod utc;` to stamp IHWIP_BUILD_UTC, and
// `build_info.rs` declares it as a test-only submodule so the tests below run
// under `cargo test`. Keep it std-only: build.rs has no serde or other crates.

/// Formats seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn format_utc(secs_since_epoch: u64) -> String {
    let days = secs_since_epoch / 86_400;
    let secs_of_day = secs_since_epoch % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60
    )
}

/// Converts days since 1970-01-01 to a proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days`, restricted to non-negative day counts.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468; // shift the epoch to 0000-03-01
    let era = z / 146_097; // 400-year eras
    let doe = z - era * 146_097; // day of era, [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of March-based year
    let mp = (5 * doy + 2) / 153; // March-based month, [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::format_utc;

    #[test]
    fn epoch_zero() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn leap_day_2000() {
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn matches_date_command() {
        // `date -u -r 1794565230 +%Y-%m-%dT%H:%M:%SZ` on macOS
        assert_eq!(format_utc(1_794_565_230), "2026-11-13T10:20:30Z");
    }

    #[test]
    fn end_of_year() {
        // `date -u -r 1767225599 +%Y-%m-%dT%H:%M:%SZ` on macOS
        assert_eq!(format_utc(1_767_225_599), "2025-12-31T23:59:59Z");
        // and the second after rolls into the new year
        assert_eq!(format_utc(1_767_225_600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn end_of_leap_year() {
        // `date -u -r 1735689599 +%Y-%m-%dT%H:%M:%SZ` on macOS
        assert_eq!(format_utc(1_735_689_599), "2024-12-31T23:59:59Z");
    }
}
