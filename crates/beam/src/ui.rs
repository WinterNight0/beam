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
