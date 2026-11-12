//! Settings a person changes from a front end: where received files go, and
//! router port mapping for the background agent. Written once for `beam
//! receive-dir`, `beam service port-mapping` and the GUI's Settings page.
//!
//! Each change goes through [`config::set_value`], which keeps the rest of
//! `config.toml` (comments, order) as it was and never writes a file beam
//! could not read back. Asking before port mapping goes on (it makes this
//! device findable by scanning) is the front end's job; the GUI's question
//! starts on "Keep it off", like the CLI's y/N.

use std::path::{Path, PathBuf};

use crate::config;
use crate::identity::Store;

/// Why a setting was not saved; the text is cleaned for a screen.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

fn fail(e: impl std::fmt::Display) -> SettingsError {
    SettingsError(crate::untrusted::lines(&e.to_string()))
}

/// Where received files go: `Some(folder)` saves it (made if missing, as a
/// full path), `None` goes back to the default. Returns the folder saved.
pub fn set_receive_dir(
    store: &Store,
    folder: Option<&Path>,
) -> Result<Option<PathBuf>, SettingsError> {
    let path = store.config_path();
    let Some(folder) = folder else {
        config::set_value(&path, "receive_dir", None).map_err(fail)?;
        return Ok(None);
    };
    let folder = if folder.is_absolute() {
        folder.to_path_buf()
    } else {
        std::env::current_dir().map_err(fail)?.join(folder)
    };
    std::fs::create_dir_all(&folder).map_err(fail)?;
    let folder = strip_verbatim(std::fs::canonicalize(&folder).map_err(fail)?);
    config::set_value(
        &path,
        "receive_dir",
        Some(&config::toml_string(&folder.display().to_string())),
    )
    .map_err(fail)?;
    Ok(Some(folder))
}

/// Router port mapping for the background agent, on or off.
pub fn set_port_mapping(store: &Store, on: bool) -> Result<(), SettingsError> {
    config::set_value(
        &store.config_path(),
        "agent_port_mapping",
        on.then_some("true"),
    )
    .map_err(fail)
}

/// `C:\x` rather than `\\?\C:\x`, which canonicalize returns on Windows and
/// people do not recognise.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_receive_folder_is_made_saved_and_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        let wanted = tmp.path().join("in coming");
        let saved = set_receive_dir(&store, Some(&wanted)).unwrap().unwrap();
        assert!(wanted.is_dir(), "made if missing");
        let config = Config::load(&store.config_path()).unwrap();
        assert_eq!(config.receive_dir.as_deref(), Some(saved.as_path()));
        assert!(!saved.display().to_string().starts_with(r"\\?\"));

        assert_eq!(set_receive_dir(&store, None).unwrap(), None);
        assert_eq!(
            Config::load(&store.config_path()).unwrap().receive_dir,
            None
        );
    }

    #[test]
    fn port_mapping_goes_on_and_off_and_keeps_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("beam"));
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.config_path(), "# mine\nport = 7900\n").unwrap();
        set_port_mapping(&store, true).unwrap();
        assert!(
            Config::load(&store.config_path())
                .unwrap()
                .agent_port_mapping
        );
        set_port_mapping(&store, false).unwrap();
        let text = std::fs::read_to_string(store.config_path()).unwrap();
        assert!(
            text.contains("# mine") && text.contains("port = 7900"),
            "{text}"
        );
        assert!(
            !Config::load(&store.config_path())
                .unwrap()
                .agent_port_mapping
        );
    }
}
