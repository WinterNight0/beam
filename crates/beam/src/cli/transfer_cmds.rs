//! Partial-transfer management.


use super::{App, CommandError, Io};
use crate::transfer::{DEFAULT_MAX_AGE, PartialError};
use crate::ui;

use serde::Serialize;

#[derive(Serialize)]
struct PartialJson {
    id: String,
    peer_fingerprint: String,
    file_name: String,
    size: u64,
    have_bytes: u64,
    percent: u8,
    expired: bool,
}

impl App {
    pub(super) fn transfers(
        &self,
        clear: bool,
        id: Option<&str>,
        assume_yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        let partials = crate::transfer::PartialStore::new(self.store.tmp_path());
        let summaries = partials
            .list(DEFAULT_MAX_AGE)
            .map_err(|e| CommandError::Partial(Box::new(e)))?;

        if clear {
            return self.clear_transfers(&partials, &summaries, id, assume_yes, io);
        }

        if self.json {
            let list: Vec<PartialJson> = summaries
                .iter()
                .map(|p| PartialJson {
                    id: p.id.clone(),
                    peer_fingerprint: p.peer_fingerprint.clone(),
                    file_name: p.file_name.clone(),
                    size: p.size,
                    have_bytes: p.have_bytes,
                    percent: p.percent(),
                    expired: p.expired,
                })
                .collect();
            return super::identity_cmds::write_json(io, &list);
        }

        if summaries.is_empty() {
            writeln!(io.out, "No partially received transfers.")?;
            return Ok(());
        }

        let rows: Vec<Vec<String>> = summaries
            .iter()
            .map(|p| {
                let age = match (p.expired, p.age()) {
                    (true, _) => "expired".to_string(),
                    (false, Some(age)) => ui::format_age(age),
                    (false, None) => "-".to_string(),
                };
                vec![
                    p.id[..8.min(p.id.len())].to_string(),
                    p.file_name.clone(),
                    ui::format_bytes(p.size),
                    format!("{}%", p.percent()),
                    age,
                ]
            })
            .collect();
        ui::table(io.out, &["ID", "FILE", "SIZE", "HAVE", "UPDATED"], &rows)?;
        writeln!(io.out)?;
        writeln!(
            io.out,
            "Sending the same file again continues from where it stopped."
        )?;
        Ok(())
    }

    fn clear_transfers(
        &self,
        partials: &crate::transfer::PartialStore,
        summaries: &[crate::transfer::PartialSummary],
        id: Option<&str>,
        assume_yes: bool,
        io: &mut Io<'_>,
    ) -> Result<(), CommandError> {
        // An id may be given in the abbreviated form `beam transfers` prints.
        let targets: Vec<&crate::transfer::PartialSummary> = match id {
            Some(prefix) => {
                let matched: Vec<_> = summaries
                    .iter()
                    .filter(|p| p.id.starts_with(prefix))
                    .collect();
                if matched.is_empty() {
                    return Err(CommandError::Message(format!(
                        "no partial transfer starts with {prefix:?}"
                    )));
                }
                if matched.len() > 1 {
                    return Err(CommandError::Message(format!(
                        "{prefix:?} matches {} partial transfers; use more characters",
                        matched.len()
                    )));
                }
                matched
            }
            None => summaries.iter().collect(),
        };

        if targets.is_empty() {
            writeln!(io.out, "No partially received transfers.")?;
            return Ok(());
        }

        if !assume_yes {
            for target in &targets {
                ui::field(
                    io.out,
                    "Discarding",
                    &format!(
                        "{} ({} of {})",
                        target.file_name,
                        ui::format_bytes(target.have_bytes),
                        ui::format_bytes(target.size)
                    ),
                )?;
            }
            let question = format!(
                "Delete {} partial transfer(s)? The bytes already received are lost.",
                targets.len()
            );
            if !ui::confirm(io.input, io.out, &question)? {
                writeln!(io.out, "Cancelled.")?;
                return Ok(());
            }
        }

        let mut removed = 0;
        for target in targets {
            match partials.remove(&target.id) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(PartialError::Busy) => writeln!(
                    io.err,
                    "beam: {} is in use by another session; left alone",
                    target.file_name
                )?,
                Err(e) => return Err(CommandError::Partial(Box::new(e))),
            }
        }
        writeln!(io.out, "Deleted {removed} partial transfer(s).")?;
        Ok(())
    }
}

