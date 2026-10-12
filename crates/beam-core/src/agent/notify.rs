//! Desktop notifications, with the tools the operating system already has
//! (ADR-0042, option A: no new dependency).
//!
//! * Windows: Windows PowerShell 5.1 shows a toast through the WinRT toast
//!   API it can load by itself.
//! * Linux: `notify-send`, present on most desktops. A server without a
//!   desktop has none; the request is still in the agent's log and in
//!   `beam inbox`.
//! * macOS: `osascript`.
//!
//! The text never becomes part of a command line or a script. It travels in
//! two environment variables that the command reads as data, and Windows
//! XML-escapes it before building the toast. A peer's file name therefore
//! cannot inject anything; it is also already cleaned by `untrusted` before it
//! gets here.
//!
//! A notification only says that a request is waiting. It cannot accept
//! anything: that happens in `beam inbox`, at the full prompt (rule 1).

use std::process::{Command, Stdio};

/// Environment variables that carry the text to the notifier.
const TITLE_ENV: &str = "BEAM_NOTIFY_TITLE";
const BODY_ENV: &str = "BEAM_NOTIFY_BODY";

/// The longest text handed to a notifier; anything longer is cut.
const MAX_TEXT: usize = 300;

/// Windows PowerShell's own app id, which may show toasts without beam
/// registering one of its own.
#[cfg(windows)]
const POWERSHELL_APP_ID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

#[cfg(windows)]
const TOAST_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
[void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]
$t = [Security.SecurityElement]::Escape($env:BEAM_NOTIFY_TITLE)
$b = [Security.SecurityElement]::Escape($env:BEAM_NOTIFY_BODY)
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$t</text><text>$b</text></binding></visual></toast>")
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($env:BEAM_NOTIFY_APP).Show($toast)
"#;

/// The command that shows `title` and `body`, or `None` where beam knows no
/// way to notify.
pub fn command(title: &str, body: &str) -> Option<Command> {
    let title = cut(title);
    let body = cut(body);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            TOAST_SCRIPT,
        ])
        .env("BEAM_NOTIFY_APP", POWERSHELL_APP_ID)
        .env(TITLE_ENV, title)
        .env(BODY_ENV, body)
        .creation_flags(CREATE_NO_WINDOW);
        Some(cmd)
    }

    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("osascript");
        cmd.args([
            "-e",
            "display notification (system attribute \"BEAM_NOTIFY_BODY\") \
             with title (system attribute \"BEAM_NOTIFY_TITLE\")",
        ])
        .env(TITLE_ENV, title)
        .env(BODY_ENV, body);
        Some(cmd)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // notify-send takes the text as arguments, but as arguments to exec,
        // never through a shell, so nothing in them is interpreted. The
        // environment variables are set too, for symmetry and for tests.
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=beam", title.as_str(), body.as_str()])
            .env(TITLE_ENV, &title)
            .env(BODY_ENV, &body);
        Some(cmd)
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = (title, body);
        None
    }
}

/// Shows a notification without waiting for it. Returns whether a notifier
/// could be started; a failure is not an error, because the request is still
/// in the log and in `beam inbox`.
pub fn show(title: &str, body: &str) -> bool {
    let Some(mut cmd) = command(title, body) else {
        return false;
    };
    match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // Reap it, so it does not linger as a zombie on Unix.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

fn cut(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_TEXT).collect();
    if text.chars().count() > MAX_TEXT {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text reaches the notifier as data, in the environment, and never
    /// inside the script or command that runs.
    #[test]
    fn the_text_travels_as_data_not_as_code() {
        let hostile = "x\"; Remove-Item C:\\ -Recurse; $(rm -rf ~) `whoami` </text><text>";
        let cmd = command("beam", hostile).expect("this platform can notify");
        let env: Vec<_> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert!(env.contains(&(BODY_ENV.to_string(), hostile.to_string())));

        #[cfg(any(windows, target_os = "macos"))]
        for arg in cmd.get_args() {
            assert!(
                !arg.to_string_lossy().contains("Remove-Item"),
                "the text leaked into the script"
            );
        }
    }

    #[test]
    fn long_text_is_cut() {
        let long = "a".repeat(1000);
        assert_eq!(cut(&long).chars().count(), MAX_TEXT + 1);
        assert_eq!(cut("short"), "short");
    }
}
