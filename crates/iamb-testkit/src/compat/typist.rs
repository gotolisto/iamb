//! Typing, select-all, copy and clearing, through the operating system.
//!
//! The harness has to type the abbreviation so that Iamb's listener sees it as real typing.
//! The platform `Injector` cannot do that: every event it sends carries Iamb's own marker
//! (`Marker::Output`), which the listener drops by design, so an abbreviation typed through it
//! would never expand. This module therefore uses what the system itself offers for sending
//! keys, which sets no marker:
//!
//! - Windows: `SendKeys.SendWait` of .NET, run by `powershell.exe` (it calls `SendInput`);
//! - macOS: `keystroke` and `key code` of System Events, run by `osascript` (Accessibility must
//!   be granted to the terminal that runs the harness).
//!
//! Both are separate processes, so this crate stays free of `unsafe`. The scripts are built by
//! plain functions that are tested on every system; only starting the program is OS specific.

use std::process::Command;

use super::apps::Os;

/// Sends keys to whatever has the keyboard focus.
pub trait Typist {
    /// Types `text` as ordinary key presses.
    fn type_text(&mut self, text: &str) -> Result<(), String>;
    /// Selects everything in the focused field and copies it.
    fn select_all_copy(&mut self) -> Result<(), String>;
    /// Selects everything in the focused field and deletes it.
    fn clear_field(&mut self) -> Result<(), String>;
}

/// One thing to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keys {
    /// Text.
    Text(String),
    /// Select all, then copy.
    SelectAllCopy,
    /// Select all, then delete.
    ClearField,
}

/// The program and arguments that send `keys` on `os`.
pub fn command_line(os: Os, keys: &Keys) -> (String, Vec<String>) {
    match os {
        Os::Windows => (
            "powershell.exe".to_owned(),
            vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-Command".to_owned(),
                windows_script(keys),
            ],
        ),
        Os::Macos => {
            let mut args = Vec::new();
            for line in mac_script(keys) {
                args.push("-e".to_owned());
                args.push(line);
            }
            ("osascript".to_owned(), args)
        }
    }
}

/// The text `SendKeys` should send for `text`: its special characters in braces.
pub fn sendkeys_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '+' | '^' | '%' | '~' | '(' | ')' | '{' | '}' | '[' | ']' => {
                out.push('{');
                out.push(c);
                out.push('}');
            }
            '\n' => out.push_str("{ENTER}"),
            c => out.push(c),
        }
    }
    out
}

/// The `PowerShell` script of `keys`.
pub fn windows_script(keys: &Keys) -> String {
    let sequence = match keys {
        Keys::Text(t) => sendkeys_escape(t),
        Keys::SelectAllCopy => "^a^c".to_owned(),
        Keys::ClearField => "^a{DEL}".to_owned(),
    };
    format!(
        "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('{}')",
        sequence.replace('\'', "''")
    )
}

fn apple_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `AppleScript` lines of `keys`, one per `-e` argument.
pub fn mac_script(keys: &Keys) -> Vec<String> {
    let events = "tell application \"System Events\" to";
    match keys {
        Keys::Text(t) => t
            .split('\n')
            .enumerate()
            .flat_map(|(i, line)| {
                let mut v = Vec::new();
                if i > 0 {
                    v.push(format!("{events} key code 36"));
                }
                if !line.is_empty() {
                    v.push(format!("{events} keystroke {}", apple_string(line)));
                }
                v
            })
            .collect(),
        Keys::SelectAllCopy => vec![
            format!("{events} keystroke \"a\" using command down"),
            "delay 0.1".to_owned(),
            format!("{events} keystroke \"c\" using command down"),
        ],
        Keys::ClearField => vec![
            format!("{events} keystroke \"a\" using command down"),
            "delay 0.1".to_owned(),
            format!("{events} key code 51"),
        ],
    }
}

/// A [`Typist`] that starts the system's own key sender.
#[derive(Debug, Clone, Copy)]
pub struct SystemTypist {
    os: Os,
}

impl SystemTypist {
    /// A typist for the OS this program runs on; `None` where there is no matrix (Linux).
    pub fn native() -> Option<Self> {
        Os::current().map(|os| Self { os })
    }

    fn send(self, keys: &Keys) -> Result<(), String> {
        let (program, args) = command_line(self.os, keys);
        let out = Command::new(&program)
            .args(&args)
            .output()
            .map_err(|e| format!("cannot start {program}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!(
                "{program} failed ({}): {}",
                out.status,
                err.lines().next().unwrap_or("no message")
            ))
        }
    }
}

impl Typist for SystemTypist {
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        self.send(&Keys::Text(text.to_owned()))
    }
    fn select_all_copy(&mut self) -> Result<(), String> {
        self.send(&Keys::SelectAllCopy)
    }
    fn clear_field(&mut self) -> Result<(), String> {
        self.send(&Keys::ClearField)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sendkeys_special_characters_are_braced() {
        assert_eq!(sendkeys_escape(";cpt1"), ";cpt1");
        assert_eq!(sendkeys_escape("^"), "{^}");
        assert_eq!(sendkeys_escape("a+b%(c)"), "a{+}b{%}{(}c{)}");
        assert_eq!(sendkeys_escape("x\ny"), "x{ENTER}y");
    }

    #[test]
    fn windows_scripts() {
        let (program, args) = command_line(Os::Windows, &Keys::Text(";cpt1".to_owned()));
        assert_eq!(program, "powershell.exe");
        assert_eq!(
            args.last().map(String::as_str),
            Some(
                "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait(';cpt1')"
            )
        );
        assert!(windows_script(&Keys::SelectAllCopy).contains("SendWait('^a^c')"));
        assert!(windows_script(&Keys::ClearField).contains("SendWait('^a{DEL}')"));
        assert!(windows_script(&Keys::Text("it's".to_owned())).contains("it''s"));
    }

    #[test]
    fn mac_scripts() {
        let (program, args) = command_line(Os::Macos, &Keys::Text(";cpt1".to_owned()));
        assert_eq!(program, "osascript");
        assert_eq!(
            args,
            [
                "-e",
                "tell application \"System Events\" to keystroke \";cpt1\""
            ]
        );
        assert!(
            mac_script(&Keys::SelectAllCopy)[2].ends_with("keystroke \"c\" using command down")
        );
        assert!(mac_script(&Keys::ClearField)[2].ends_with("key code 51"));
        assert_eq!(
            mac_script(&Keys::Text("say \"hi\"".to_owned())),
            ["tell application \"System Events\" to keystroke \"say \\\"hi\\\"\""]
        );
    }
}
