//! Terminal output helpers shared by the beam commands.

use std::io::{self, BufRead, Write};

/// Writes one `label  value` line of a detail block.
///
/// The value goes through `untrusted::text` whatever its origin: most values
/// are ours, but some carry a peer's text, and a detail line is exactly where a
/// forged fingerprint would be printed (ADR-0034).
pub fn field(out: &mut dyn Write, label: &str, value: &str) -> io::Result<()> {
    writeln!(out, "  {label:<13} {}", beam_core::untrusted::text(value))
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

#[cfg(test)]
mod tests {
    use super::*;

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
