//! `beam history`: what came and went (ADR-0043).

use serde::Serialize;

use super::{App, CommandError, Io};
use crate::history::{self, Direction, Outcome};
use crate::{ui, untrusted};

/// The `--json` shape of one entry. File and peer names are given as stored;
/// a program reading them is responsible for showing them safely.
#[derive(Serialize)]
struct EntryJson<'a> {
    at: u64,
    direction: Direction,
    peer: &'a str,
    fingerprint: &'a str,
    file: &'a str,
    size: u64,
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
}

impl App {
    pub(super) fn history(
        &self,
        clear: bool,
        yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        if clear {
            if !yes && !ui::confirm(io.input, io.out, "Delete the whole transfer history?")? {
                writeln!(io.out, "Nothing was deleted.")?;
                return Ok(());
            }
            let had = history::clear(&self.store)?;
            writeln!(
                io.out,
                "{}",
                if had {
                    "Transfer history deleted."
                } else {
                    "There was no transfer history."
                }
            )?;
            return Ok(());
        }

        let entries = history::read(&self.store);
        if self.json {
            let list: Vec<EntryJson> = entries
                .iter()
                .map(|e| EntryJson {
                    at: e.at,
                    direction: e.direction,
                    peer: &e.peer,
                    fingerprint: &e.fingerprint,
                    file: &e.file,
                    size: e.size,
                    outcome: e.outcome,
                    note: e.note.as_deref(),
                })
                .collect();
            return super::identity_cmds::write_json(io, &list);
        }
        if entries.is_empty() {
            writeln!(io.out, "No transfers yet.")?;
            return Ok(());
        }
        let now = history::unix_now();
        // Newest first: what a person looks for is what just happened.
        let rows: Vec<Vec<String>> = entries
            .iter()
            .rev()
            .map(|e| {
                let way = match e.direction {
                    Direction::Sent => "sent to",
                    Direction::Received => "from",
                };
                let mut result = history::outcome_word(e.outcome, e.direction).to_string();
                if let Some(note) = &e.note {
                    result.push_str(&format!(" ({})", untrusted::text(note)));
                }
                vec![
                    history::ago(e.at, now),
                    format!("{way} {}", untrusted::name(&e.peer)),
                    untrusted::name(&e.file),
                    ui::format_bytes(e.size),
                    result,
                ]
            })
            .collect();
        ui::table(io.out, &["WHEN", "WHO", "FILE", "SIZE", "RESULT"], &rows)?;
        writeln!(
            io.out,
            "\nKept in {} (the newest {}). `beam history --clear` deletes it.",
            self.store.history_path().display(),
            history::KEEP
        )?;
        Ok(())
    }
}
