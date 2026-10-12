//! Numbers and times in the words a person reads: sizes, percentages,
//! ages. Plain text, no terminal: the core, the CLI, the full-screen view
//! and a GUI all say "4.0 MiB" and "2 minutes ago" the same way.

/// Renders a byte count the way a person reads it.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A percentage, with an empty total treated as complete.
pub fn percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 100;
    }
    ((done.min(total) as f64 / total as f64) * 100.0).round() as u8
}

/// How long ago something happened, in the words a person would use.
pub fn format_age(age: std::time::Duration) -> String {
    let seconds = age.as_secs();
    // Truncating rather than rounding, so a value never overflows into the next
    // unit's name: 3599 seconds is "59 minutes ago", not "60 minutes ago".
    let (value, unit) = match seconds {
        0..90 => (seconds.max(1), "second"),
        90..3600 => (seconds / 60, "minute"),
        3600..86_400 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counts_read_like_a_person_would_say_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(4 * 1024 * 1024), "4.0 MiB");
        assert_eq!(format_bytes(3_221_225_472), "3.0 GiB");
    }

    #[test]
    fn percentages_are_bounded_and_an_empty_file_is_complete() {
        assert_eq!(percent(0, 0), 100, "an empty file is not 0% done");
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(100, 100), 100);
        assert_eq!(percent(200, 100), 100, "progress ran past the end");
    }

    #[test]
    fn ages_read_the_way_a_person_would_say_them() {
        use std::time::Duration;
        assert_eq!(format_age(Duration::from_secs(1)), "1 second ago");
        assert_eq!(format_age(Duration::from_secs(45)), "45 seconds ago");
        assert_eq!(format_age(Duration::from_secs(120)), "2 minutes ago");
        assert_eq!(format_age(Duration::from_secs(3600)), "1 hour ago");
        assert_eq!(format_age(Duration::from_secs(7200)), "2 hours ago");
        assert_eq!(format_age(Duration::from_secs(86_400)), "1 day ago");
        assert_eq!(format_age(Duration::from_secs(2 * 86_400)), "2 days ago");
        // Never "0 seconds ago", which reads as though nothing happened.
        assert_eq!(format_age(Duration::from_millis(10)), "1 second ago");
    }
}
