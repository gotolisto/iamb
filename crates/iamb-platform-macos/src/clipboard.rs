//! The clipboard: the general pasteboard (architecture §5, §8 rules 2, 3 and 8; the trait
//! docs of [`ClipboardAccess`](iamb_platform::ClipboardAccess)).
//!
//! | Call | Does |
//! | --- | --- |
//! | `read_text` | the plain text of the first item that has some (`public.utf8-plain-text`), as `Sensitive` |
//! | `snapshot` | every item, every type, its data copied out as bytes; types the pasteboard gives no data for, or that pass the size limit, are recorded and left out ([`layout`]) |
//! | `write(content, transient)` | clears the pasteboard and writes plain text and HTML in one item, and one item per file; with `transient`, the nspasteboard.org transient and concealed marker types on every item, which clipboard managers honour |
//! | `restore(snapshot)` | clears the pasteboard and writes every saved item back, type for type and byte for byte, with the transient marker, unless the change-count rule says to leave it alone |
//!
//! # The change-count rule
//!
//! After Iamb writes, it remembers the pasteboard's `changeCount`. If the count has moved by
//! the time of the restore, someone copied something after Iamb's write: the restore then
//! leaves the pasteboard alone and succeeds, because putting the old content back would
//! destroy the newer copy. See [`ledger`] for the full table.
//!
//! # Pasting
//!
//! [`paste`] holds the flow the injector uses: snapshot, transient write, Command+V, a bounded
//! wait, restore, with the restore on every path once the write was attempted.
//!
//! # Privacy
//!
//! Pasteboard content is the user's own words. It is copied into `Sensitive` buffers, the
//! types that hold it have no `Debug` or one that shows sizes only, and errors name the call
//! that failed (`NSPasteboard.writeObjects`), never the content.
//!
//! # Limits
//!
//! - Data the owner provides lazily is read (and so produced) at snapshot time; a type whose
//!   provider gives nothing is left out of the restore.
//! - Recent macOS releases can ask the user before an application reads the pasteboard
//!   without a user action (`accessBehavior`). The worker reads it only for `{clipboard}` and
//!   around a paste, both of which the user caused, but the system may still ask once.

pub(crate) mod board;
pub(crate) mod layout;
pub(crate) mod ledger;
pub(crate) mod paste;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub(crate) use mac::State;
