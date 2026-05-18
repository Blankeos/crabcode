use std::time::{Duration, SystemTime};

pub fn relative_readable_time_from_now(time: SystemTime) -> String {
    relative_readable_time(time, SystemTime::now())
}

pub fn relative_readable_time(time: SystemTime, now: SystemTime) -> String {
    let elapsed = now.duration_since(time).unwrap_or(Duration::ZERO);
    let seconds = elapsed.as_secs();

    if seconds < 60 {
        return format!("{}s ago", seconds);
    }

    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{}m ago", minutes);
    }

    let hours = minutes / 60;
    if hours < 24 {
        return format!("{}{} ago", hours, if hours == 1 { "hr" } else { "hrs" });
    }

    let days = hours / 24;
    if days < 30 {
        return format!("{}d ago", days);
    }

    let months = days / 30;
    if months < 12 {
        return format!("{}{} ago", months, if months == 1 { "mo" } else { "mos" });
    }

    let years = days / 365;
    format!("{}{} ago", years, if years == 1 { "yr" } else { "yrs" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ago(seconds: u64) -> (SystemTime, SystemTime) {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        (now - Duration::from_secs(seconds), now)
    }

    #[test]
    fn formats_single_relative_unit() {
        let cases = [
            (2, "2s ago"),
            (120, "2m ago"),
            (7_200, "2hrs ago"),
            (172_800, "2d ago"),
            (5_184_000, "2mos ago"),
            (63_072_000, "2yrs ago"),
        ];

        for (seconds, expected) in cases {
            let (time, now) = ago(seconds);
            assert_eq!(relative_readable_time(time, now), expected);
        }
    }

    #[test]
    fn clamps_future_times_to_zero_seconds() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let future = now + Duration::from_secs(5);

        assert_eq!(relative_readable_time(future, now), "0s ago");
    }
}
