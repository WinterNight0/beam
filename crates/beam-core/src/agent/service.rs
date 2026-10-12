//! Starting the agent at login, and starting or stopping it now (ADR-0042).
//!
//! Always for the current user, never system-wide, and never with
//! administrator or root rights:
//!
//! * **Windows:** a value under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` runs
//!   `beam service start` at login. That starts the agent as a process with
//!   no console window (`CREATE_NO_WINDOW`) and returns. A console window
//!   flashes for a moment at login while `service start` runs.
//! * **Linux:** a `systemd --user` unit, `~/.config/systemd/user/beam-agent.service`,
//!   enabled with `systemctl --user enable --now`. Without systemd, `beam
//!   service start` still starts the agent for this session.
//! * **macOS:** no login integration yet; `beam service start` works.
//!
//! The OS tools (`reg`, `systemctl`) are run as programs with arguments,
//! never through a shell.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The name of the login entry (Windows) or unit (Linux).
pub const SERVICE_NAME: &str = "beam-agent";

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Why a service action failed.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn run(cmd: &mut Command, what: &str) -> Result<(), ServiceError> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ServiceError::Message(format!("could not run {what}: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ServiceError::Message(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// The command line the login entry runs: `exe`, for the beam home
/// `beam_dir`, so the agent at login uses the same identity as the person
/// who enabled it.
pub fn login_command(exe: &Path, beam_dir: &Path) -> String {
    // A trailing backslash would escape the closing quote.
    let home = beam_dir.display().to_string();
    let home = home.trim_end_matches('\\');
    format!("\"{}\" --beam-dir \"{home}\" service start", exe.display())
}

/// The systemd unit for `exe` and the beam home `beam_dir`, run in `dir`.
pub fn systemd_unit(exe: &Path, beam_dir: &Path, dir: &Path) -> Result<String, ServiceError> {
    for (what, path) in [
        ("beam's path", exe),
        ("the beam folder", beam_dir),
        ("the folder", dir),
    ] {
        let text = path.display().to_string();
        if text.contains(['"', '\n', '\r', '%', '\\']) && !cfg!(windows) {
            return Err(ServiceError::Message(format!(
                "{what} {text:?} contains a character a systemd unit cannot hold safely; \
                 move beam or choose another folder"
            )));
        }
    }
    Ok(format!(
        "[Unit]\n\
         Description=beam background agent (receives files from paired devices)\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\" --beam-dir \"{}\" agent\n\
         WorkingDirectory={}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display(),
        beam_dir.display(),
        dir.display()
    ))
}

/// Where the systemd user unit lives.
pub fn unit_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| {
        d.join("systemd")
            .join("user")
            .join(format!("{SERVICE_NAME}.service"))
    })
}

/// Makes the agent start at login. Does not start it now.
pub fn enable(exe: &Path, beam_dir: &Path, dir: &Path) -> Result<(), ServiceError> {
    #[cfg(windows)]
    {
        let _ = dir;
        run(
            Command::new("reg").args([
                "add",
                RUN_KEY,
                "/v",
                SERVICE_NAME,
                "/t",
                "REG_SZ",
                "/d",
                &login_command(exe, beam_dir),
                "/f",
            ]),
            "reg add",
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let unit = systemd_unit(exe, beam_dir, dir)?;
        let path = unit_path()
            .ok_or_else(|| ServiceError::Message("no config directory for systemd units".into()))?;
        std::fs::create_dir_all(path.parent().expect("a unit path has a parent"))?;
        std::fs::write(&path, unit)?;
        run(
            Command::new("systemctl").args(["--user", "daemon-reload"]),
            "systemctl --user daemon-reload",
        )
        .map_err(no_systemd)?;
        run(
            Command::new("systemctl").args(["--user", "enable", SERVICE_NAME]),
            "systemctl --user enable",
        )
        .map_err(no_systemd)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (exe, beam_dir, dir);
        Err(ServiceError::Message(
            "starting at login is not supported on macOS yet; run `beam service start` \
             after logging in"
                .into(),
        ))
    }
}

/// Stops the agent starting at login. Does not stop a running one.
pub fn disable() -> Result<(), ServiceError> {
    #[cfg(windows)]
    {
        if !is_enabled() {
            return Ok(());
        }
        run(
            Command::new("reg").args(["delete", RUN_KEY, "/v", SERVICE_NAME, "/f"]),
            "reg delete",
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", SERVICE_NAME])
            .stdin(Stdio::null())
            .output();
        if let Some(path) = unit_path()
            && path.exists()
        {
            std::fs::remove_file(path)?;
        }
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdin(Stdio::null())
            .output();
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        Ok(())
    }
}

/// Whether the agent is set to start at login.
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        Command::new("reg")
            .args(["query", RUN_KEY, "/v", SERVICE_NAME])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        unit_path().is_some_and(|p| p.exists())
            && Command::new("systemctl")
                .args(["--user", "is-enabled", "--quiet", SERVICE_NAME])
                .stdin(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }
    #[cfg(target_os = "macos")]
    {
        false
    }
}

/// Starts the agent in the background, in `dir`, and returns at once.
pub fn start(exe: &Path, dir: &Path, beam_dir: &Path) -> Result<(), ServiceError> {
    if let Some(result) = start_with_systemd() {
        return result;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Through Start-Process, which launches via the shell: the agent then
        // inherits none of this process's handles. Spawned directly, it would
        // inherit them all, and if `beam service start`'s output were a pipe
        // (a script), whoever read it would wait until the agent exited. The
        // window is hidden, and it is a console of its own, so Ctrl+C in this
        // terminal does not reach it. The paths travel in the environment,
        // never inside the script.
        let home = beam_dir.display().to_string();
        let args = format!("--beam-dir \"{}\" agent", home.trim_end_matches('\\'));
        run(
            Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    "Start-Process -FilePath $env:BEAM_EXE -ArgumentList $env:BEAM_ARGS \
                     -WorkingDirectory $env:BEAM_CWD -WindowStyle Hidden",
                ])
                .env("BEAM_EXE", exe)
                .env("BEAM_ARGS", args)
                .env("BEAM_CWD", dir)
                .creation_flags(CREATE_NO_WINDOW),
            "Start-Process",
        )
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new(exe);
        cmd.arg("--beam-dir")
            .arg(beam_dir)
            .arg("agent")
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Out of the terminal's process group, so closing it does not
            // stop the agent.
            .process_group(0);
        cmd.spawn()?;
        Ok(())
    }
}

/// With the systemd unit installed, systemd starts the agent, so
/// `systemctl --user status` tells the truth. `None` when there is no unit.
#[cfg(all(unix, not(target_os = "macos")))]
fn start_with_systemd() -> Option<Result<(), ServiceError>> {
    if !unit_path().is_some_and(|p| p.exists()) {
        return None;
    }
    Some(run(
        Command::new("systemctl").args(["--user", "start", SERVICE_NAME]),
        "systemctl --user start",
    ))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn start_with_systemd() -> Option<Result<(), ServiceError>> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn no_systemd(e: ServiceError) -> ServiceError {
    ServiceError::Message(format!(
        "{e}\nsystemd --user is not available here. `beam service start` still starts the \
         agent for this session; to start it at login, run that from your login script"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_entry_quotes_beams_path() {
        let exe = Path::new(r"C:\Program Files\beam\beam.exe");
        // The beam home goes along, so the agent at login has the same identity.
        let home = Path::new(r"C:\Users\a b\.beam\");
        assert_eq!(
            login_command(exe, home),
            r#""C:\Program Files\beam\beam.exe" --beam-dir "C:\Users\a b\.beam" service start"#
        );
    }

    /// What `enable` stores survives `reg add` exactly, quotes and spaces
    /// included. Written to a throwaway key, never the real Run key; run by
    /// hand: `cargo test -p beam --lib login_entry_survives -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "writes to the registry (a throwaway HKCU key)"]
    fn the_login_entry_survives_the_registry() {
        let key = format!(r"HKCU\Software\beam-test-{}", std::process::id());
        let value = login_command(
            Path::new(r"C:\Program Files\beam\beam.exe"),
            Path::new(r"C:\Users\a b\.beam"),
        );
        run(
            Command::new("reg").args(["add", &key, "/v", "x", "/t", "REG_SZ", "/d", &value, "/f"]),
            "reg add",
        )
        .unwrap();
        let out = Command::new("reg")
            .args(["query", &key, "/v", "x"])
            .output()
            .unwrap();
        let _ = Command::new("reg").args(["delete", &key, "/f"]).output();
        let text = String::from_utf8_lossy(&out.stdout);
        let stored = text
            .lines()
            .find_map(|l| l.split("REG_SZ").nth(1))
            .map(str::trim)
            .unwrap();
        assert_eq!(stored, value);
    }

    #[cfg(unix)]
    #[test]
    fn the_systemd_unit_runs_the_agent_for_the_user() {
        let unit = systemd_unit(
            Path::new("/home/a/.local/bin/beam"),
            Path::new("/home/a/.beam"),
            Path::new("/home/a"),
        )
        .unwrap();
        assert!(
            unit.contains(
                "ExecStart=\"/home/a/.local/bin/beam\" --beam-dir \"/home/a/.beam\" agent\n"
            ),
            "{unit}"
        );
        assert!(unit.contains("WorkingDirectory=/home/a\n"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert!(
            !unit.contains("User="),
            "a user unit runs as the user already"
        );
        assert!(
            systemd_unit(Path::new("/tmp/a\"b/beam"), Path::new("/h"), Path::new("/")).is_err()
        );
    }
}
