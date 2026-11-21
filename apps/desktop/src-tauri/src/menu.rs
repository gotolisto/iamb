//! What the tray or menu-bar menu shows (UX specification §4.1), independent of Tauri:
//! `app/tray.rs` builds the native menu and the icon from it.
//!
//! # Icon states and the first line
//!
//! Three icon states, never more: **On** (the mark), **Paused** (the mark at reduced strength)
//! and **Needs you** (the mark with a dot). The first line of the menu, not selectable, says
//! which, with a second line under it:
//!
//! | State | First line | Second line |
//! | --- | --- | --- |
//! | On | Iamb is on | "76 snippets" |
//! | Paused | Iamb is paused | "Typed abbreviations do not expand. Search still works." |
//! | Needs you | Iamb cannot expand yet | "2 permissions are missing" |
//! | Needs you | Iamb is not running | "Its background service stopped." |
//!
//! "Needs you" wins over "Paused": the cause is something the user has to act on. While the
//! bridge is still connecting the menu says so with the plain mark (it lasts a moment at start;
//! a "Needs you" dot there would flicker at every login). The icon does not change with the
//! focused app (specification §4.1).
//!
//! # "Never expand in *App*"
//!
//! The menu offers to exclude, or to stop excluding, the application that was in front when the
//! menu opened, named by its id as the focus inspector reports it (the bundle identifier on
//! macOS, the executable's file name on Windows): [`app_name`] makes the name shown from it, as
//! the platform cannot give a display name yet (specification §11.2, entry 13). The item is
//! hidden when that application is Iamb or cannot be named ([`exclusion_item`]).

use serde::{Deserialize, Serialize};

use crate::contract::{ConnectionState, StatusView};

/// The icon's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// The mark.
    On,
    /// The mark at reduced strength.
    Paused,
    /// The mark with a dot.
    NeedsYou,
}

/// The "Needs you" item for the cause (specification §4.1: "Fix permissions… / Start Iamb
/// again / Restart to update").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedsItem {
    /// "Fix permissions…".
    FixPermissions,
    /// "Start Iamb again".
    StartAgain,
}

/// The menu's first line, its second line, the icon and the item for the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuStatus {
    /// The first line ("Iamb is on").
    pub title: &'static str,
    /// The second line ("76 snippets"), when there is one.
    pub detail: Option<String>,
    /// The icon.
    pub icon: IconState,
    /// The item for the cause, in "Needs you" only.
    pub needs: Option<NeedsItem>,
}

/// "1 permission is missing" / "2 permissions are missing".
#[must_use]
pub fn permissions_missing(n: u8) -> String {
    if n == 1 {
        "1 permission is missing".to_owned()
    } else {
        format!("{n} permissions are missing")
    }
}

/// "1 snippet" / "76 snippets".
#[must_use]
pub fn snippet_count(n: u64) -> String {
    if n == 1 {
        "1 snippet".to_owned()
    } else {
        format!("{n} snippets")
    }
}

/// What the menu shows for the bridge's `view`, with `missing` permissions (macOS; the service
/// is asked every 30 s once setup is done, `app::onboarding_commands`).
#[must_use]
pub fn status(view: &StatusView, missing: u8) -> MenuStatus {
    match (view.connection, view.service.as_ref()) {
        (ConnectionState::Disconnected, _) => MenuStatus {
            title: "Iamb is not running",
            detail: Some("Its background service stopped.".to_owned()),
            icon: IconState::NeedsYou,
            needs: Some(NeedsItem::StartAgain),
        },
        (ConnectionState::Connecting, _) | (ConnectionState::Connected, None) => MenuStatus {
            title: "Connecting…",
            detail: None,
            icon: IconState::On,
            needs: None,
        },
        (ConnectionState::Connected, Some(_)) if missing > 0 => MenuStatus {
            title: "Iamb cannot expand yet",
            detail: Some(permissions_missing(missing)),
            icon: IconState::NeedsYou,
            needs: Some(NeedsItem::FixPermissions),
        },
        (ConnectionState::Connected, Some(s)) if s.paused => MenuStatus {
            title: "Iamb is paused",
            detail: Some("Typed abbreviations do not expand. Search still works.".to_owned()),
            icon: IconState::Paused,
            needs: None,
        },
        (ConnectionState::Connected, Some(s)) if !s.listener => MenuStatus {
            title: "Iamb cannot expand yet",
            detail: None,
            icon: IconState::NeedsYou,
            needs: None,
        },
        (ConnectionState::Connected, Some(s)) => MenuStatus {
            title: "Iamb is on",
            detail: Some(snippet_count(s.snippets)),
            icon: IconState::On,
            needs: None,
        },
    }
}

/// The name shown for the application with the id `app_id`: the last part of a bundle
/// identifier (`com.example.Mail` → "Mail"), an executable's name without `.exe`
/// (`winword.exe` → "winword"). `None` when nothing is left to show.
#[must_use]
pub fn app_name(app_id: &str) -> Option<String> {
    let id = app_id.trim();
    let base = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let stem = base
        .len()
        .checked_sub(4)
        .filter(|&at| at > 0)
        .and_then(|at| Some((base.get(..at)?, base.get(at..)?)))
        .filter(|(_, ext)| ext.eq_ignore_ascii_case(".exe"))
        .map(|(stem, _)| stem);
    let name = if let Some(stem) = stem {
        stem
    } else if base.contains('.') {
        base.rsplit('.').next().unwrap_or(base)
    } else {
        base
    };
    let name = name.trim();
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

/// The application the menu's exclusion item is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionTarget {
    /// Its id, as the focus inspector reports it.
    pub app_id: String,
    /// The name shown.
    pub name: String,
    /// It is excluded now.
    pub excluded: bool,
}

impl ExclusionTarget {
    /// The item's text: "Never expand in Mail" / "Expand in Mail again".
    #[must_use]
    pub fn label(&self) -> String {
        if self.excluded {
            format!("Expand in {} again", self.name)
        } else {
            format!("Never expand in {}", self.name)
        }
    }
}

/// The exclusion item for the application in front (`app_id`, `own` when it is part of Iamb),
/// given the ids excluded now; `None` hides the item: no application, Iamb itself, or one that
/// cannot be named.
#[must_use]
pub fn exclusion_item<'a>(
    front: Option<(&str, bool)>,
    excluded: impl IntoIterator<Item = &'a str>,
) -> Option<ExclusionTarget> {
    let (app_id, own) = front?;
    if own {
        return None;
    }
    let name = app_name(app_id)?;
    let excluded = excluded
        .into_iter()
        .any(|id| id.trim().eq_ignore_ascii_case(app_id.trim()));
    Some(ExclusionTarget {
        app_id: app_id.to_owned(),
        name,
        excluded,
    })
}

/// "Quit Iamb" (macOS) / "Exit Iamb" (Windows) (specification §7).
#[must_use]
pub const fn quit_label(windows: bool) -> &'static str {
    if windows { "Exit Iamb" } else { "Quit Iamb" }
}

/// The search shortcut as the menu's accelerator text understands it: the stored chord
/// (`Ctrl+Shift+Space`) with `Meta` written `Super`. `None` for an empty chord.
#[must_use]
pub fn accelerator(chord: &str) -> Option<String> {
    let parts: Vec<&str> = chord
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p.eq_ignore_ascii_case("meta") {
                "Super"
            } else {
                p
            }
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("+"))
}

/// The "Paused" icon: the mark at reduced strength (every pixel at 40 % of its opacity).
/// `rgba` holds 4 bytes per pixel.
pub fn dim(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        px[3] = u8::try_from(u16::from(px[3]) * 2 / 5).unwrap_or(u8::MAX);
    }
}

/// The "Needs you" icon: the mark with a dot, a filled circle of the warning colour in the
/// top-right corner, a third of the icon wide, with a ring of transparency around it so that
/// it stands out from the mark. `rgba` holds `width × height` pixels of 4 bytes.
pub fn add_dot(rgba: &mut [u8], width: u32, height: u32) {
    const DOT: [u8; 3] = [0xD9, 0x48, 0x1A];
    let size = width.min(height);
    if size == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
        return;
    }
    let r = f64::from(size) / 6.0;
    let ring = r + f64::from(size) / 16.0;
    let cx = f64::from(width) - r - 0.5;
    let cy = r + 0.5;
    for y in 0..height {
        for x in 0..width {
            let dx = f64::from(x) + 0.5 - cx;
            let dy = f64::from(y) + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let i = ((y * width + x) * 4) as usize;
            if d <= r {
                rgba[i..i + 3].copy_from_slice(&DOT);
                rgba[i + 3] = u8::MAX;
            } else if d <= ring {
                rgba[i + 3] = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ServiceView;

    fn connected(paused: bool, listener: bool, snippets: u64) -> StatusView {
        StatusView {
            connection: ConnectionState::Connected,
            service: Some(ServiceView {
                version: "0.1.0".into(),
                paused,
                suspended: paused,
                snippets,
                revision: 1,
                listener,
                memory_locked: true,
            }),
        }
    }

    fn bare(connection: ConnectionState) -> StatusView {
        StatusView {
            connection,
            service: None,
        }
    }

    #[test]
    fn the_first_line_follows_the_specification() {
        let on = status(&connected(false, true, 76), 0);
        assert_eq!(on.title, "Iamb is on");
        assert_eq!(on.detail.as_deref(), Some("76 snippets"));
        assert_eq!(on.icon, IconState::On);
        assert_eq!(on.needs, None);
        assert_eq!(
            status(&connected(false, true, 1), 0).detail.as_deref(),
            Some("1 snippet")
        );

        let paused = status(&connected(true, true, 76), 0);
        assert_eq!(paused.title, "Iamb is paused");
        assert_eq!(
            paused.detail.as_deref(),
            Some("Typed abbreviations do not expand. Search still works.")
        );
        assert_eq!(paused.icon, IconState::Paused);

        let down = status(&bare(ConnectionState::Disconnected), 0);
        assert_eq!(down.title, "Iamb is not running");
        assert_eq!(
            down.detail.as_deref(),
            Some("Its background service stopped.")
        );
        assert_eq!(down.icon, IconState::NeedsYou);
        assert_eq!(down.needs, Some(NeedsItem::StartAgain));

        let missing = status(&connected(false, false, 76), 2);
        assert_eq!(missing.title, "Iamb cannot expand yet");
        assert_eq!(missing.detail.as_deref(), Some("2 permissions are missing"));
        assert_eq!(missing.icon, IconState::NeedsYou);
        assert_eq!(missing.needs, Some(NeedsItem::FixPermissions));
        assert_eq!(
            status(&connected(false, false, 76), 1).detail.as_deref(),
            Some("1 permission is missing")
        );
    }

    #[test]
    fn needs_you_wins_over_paused_and_connecting_shows_the_plain_mark() {
        assert_eq!(
            status(&connected(true, false, 3), 1).icon,
            IconState::NeedsYou
        );
        let connecting = status(&bare(ConnectionState::Connecting), 0);
        assert_eq!(connecting.icon, IconState::On);
        assert_eq!(connecting.needs, None);
        // A listener that is off without a missing permission: needs you, no item to offer.
        let deaf = status(&connected(false, false, 3), 0);
        assert_eq!(deaf.title, "Iamb cannot expand yet");
        assert_eq!(deaf.icon, IconState::NeedsYou);
        assert_eq!(deaf.needs, None);
    }

    #[test]
    fn applications_are_named_from_their_ids() {
        assert_eq!(app_name("com.example.Mail").as_deref(), Some("Mail"));
        assert_eq!(app_name("winword.exe").as_deref(), Some("winword"));
        assert_eq!(app_name("NOTEPAD.EXE").as_deref(), Some("NOTEPAD"));
        assert_eq!(
            app_name(r"C:\Program Files\Tool\tool.exe").as_deref(),
            Some("tool")
        );
        assert_eq!(app_name("terminal").as_deref(), Some("terminal"));
        assert_eq!(app_name(""), None);
        assert_eq!(app_name("  "), None);
        assert_eq!(app_name(".exe"), Some("exe".to_owned()));
        assert_eq!(app_name("com.example."), None);
    }

    #[test]
    fn the_exclusion_item_names_the_app_in_front_and_hides_for_iamb() {
        let item = exclusion_item(Some(("com.example.Mail", false)), ["other.exe"]).expect("item");
        assert_eq!(item.label(), "Never expand in Mail");
        let item =
            exclusion_item(Some(("com.example.Mail", false)), ["COM.EXAMPLE.MAIL"]).expect("item");
        assert!(item.excluded);
        assert_eq!(item.label(), "Expand in Mail again");
        assert_eq!(exclusion_item(Some(("org.iamb.Iamb", true)), []), None);
        assert_eq!(exclusion_item(None, []), None);
        assert_eq!(exclusion_item(Some(("", false)), []), None);
    }

    #[test]
    fn quit_is_exit_on_windows() {
        assert_eq!(quit_label(true), "Exit Iamb");
        assert_eq!(quit_label(false), "Quit Iamb");
    }

    #[test]
    fn chords_become_accelerators() {
        assert_eq!(
            accelerator("Ctrl+Shift+Space").as_deref(),
            Some("Ctrl+Shift+Space")
        );
        assert_eq!(accelerator("Meta+Alt+K").as_deref(), Some("Super+Alt+K"));
        assert_eq!(accelerator(""), None);
    }

    #[test]
    fn the_paused_icon_is_dimmed_and_the_needs_you_icon_has_a_dot() {
        let mut px = vec![10, 20, 30, 255, 1, 2, 3, 100];
        dim(&mut px);
        assert_eq!(px, [10, 20, 30, 102, 1, 2, 3, 40]);

        let (w, h) = (32_u32, 32_u32);
        let mut icon = vec![0_u8; (w * h * 4) as usize];
        for p in icon.chunks_exact_mut(4) {
            p.copy_from_slice(&[0, 0, 0, 255]);
        }
        add_dot(&mut icon, w, h);
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [icon[i], icon[i + 1], icon[i + 2], icon[i + 3]]
        };
        // The dot's centre is the warning colour, the far corner is untouched.
        assert_eq!(at(w - 6, 5), [0xD9, 0x48, 0x1A, 255]);
        assert_eq!(at(0, h - 1), [0, 0, 0, 255]);
        // A ring of transparency separates the dot from the mark.
        assert_eq!(at(w - 6 - 7, 5)[3], 0);
        // A buffer of the wrong size is left alone.
        let mut short = vec![0_u8; 8];
        add_dot(&mut short, w, h);
        assert_eq!(short, [0; 8]);
    }
}
