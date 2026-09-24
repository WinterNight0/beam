//! Terminal output helpers shared by the beam commands.

use std::io::{self, BufRead, Write};

/// Writes one `label  value` line of a detail block.
pub fn field(out: &mut dyn Write, label: &str, value: &str) -> io::Result<()> {
    writeln!(out, "  {label:<13} {value}")
}

/// Writes a simple aligned table. An empty `rows` prints the headers only.
pub fn table(out: &mut dyn Write, headers: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }

    let mut line = String::new();
    for (i, header) in headers.iter().enumerate() {
        push_cell(&mut line, header, widths[i], i + 1 == headers.len());
    }
    writeln!(out, "{line}")?;

    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            push_cell(&mut line, cell, widths[i], i + 1 == row.len());
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn push_cell(line: &mut String, cell: &str, width: usize, last: bool) {
    if last {
        line.push_str(cell);
        return;
    }
    line.push_str(cell);
    for _ in cell.chars().count()..width + 2 {
        line.push(' ');
    }
}

/// Asks a yes/no question.
///
/// Anything other than `y` or `yes` is a no, and end-of-input is a no: a
/// destructive action never proceeds by default.
pub fn confirm(input: &mut dyn BufRead, out: &mut dyn Write, question: &str) -> io::Result<bool> {
    write!(out, "{question} [y/N]: ")?;
    out.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(out)?;
        return Ok(false);
    }
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

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

    #[test]
    fn a_table_lines_up_its_columns() {
        let mut out = Vec::new();
        table(
            &mut out,
            &["NAME", "SIZE"],
            &[
                vec!["short".to_string(), "1".to_string()],
                vec!["a-much-longer-name".to_string(), "2".to_string()],
            ],
        )
        .expect("table");
        let rendered = String::from_utf8(out).expect("utf-8");
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        let size_column = lines[0].find("SIZE").expect("header");
        for line in &lines[1..] {
            assert_eq!(line.find(['1', '2']), Some(size_column), "{line:?}");
        }
    }

    #[test]
    fn confirm_says_no_to_everything_but_yes() {
        for (answer, expected) in [
            ("y\n", true),
            ("Y\n", true),
            ("yes\n", true),
            ("YES\n", true),
            ("n\n", false),
            ("no\n", false),
            ("\n", false),
            ("maybe\n", false),
            ("", false),
        ] {
            let mut input = answer.as_bytes();
            let mut out = Vec::new();
            assert_eq!(
                confirm(&mut input, &mut out, "Remove alice?").expect("confirm"),
                expected,
                "answer {answer:?}"
            );
        }
    }
}
