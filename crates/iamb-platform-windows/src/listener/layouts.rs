//! The dead-key tables of keyboard layouts, read from the layouts' own files.
//!
//! The system offers no call that says "´ followed by e is é" without typing at it, and
//! typing at it changes the dead-key state the application in front depends on. The table is
//! in the layout's file, though: every layout is a library in the system directory that
//! exports one function, `KbdLayerDescriptor`, which returns the address of the layout's
//! tables, and one of those is the list of dead-key compositions. This module finds the file
//! of a layout handle in the registry, loads it, copies that list and unloads it. Nothing
//! about the keyboard changes.
//!
//! Reading a table takes a registry lookup and a library load, so it never happens in a hook
//! call: the listener thread reads the tables of the user's layouts before it installs its
//! hooks, and the table of a layout it meets later after the hook call that met it has
//! returned. Until then a dead-key sequence on that layout is reported as a boundary.
//!
//! The tables hold addresses, in the width of the system's own pointers. A 64-bit process
//! reads them directly; a 32-bit process on a 64-bit system cannot, and Iamb is 64-bit only,
//! so on other widths no table is read.

use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{DEADKEY, DKF_DEAD, KBDTABLES};
use windows_sys::s;

use super::deadkey::{Composition, DeadKeyTable, KeyQuery, LayoutDeadKeys, Probe};
use super::os;

/// More entries than any layout has; a table without its end marker is not followed further.
const MOST_ENTRIES: usize = 16_384;

/// More layouts than anyone installs; further ones are not remembered.
const MOST_LAYOUTS: usize = 64;

/// The layouts whose tables have been read.
#[derive(Debug, Default)]
pub(super) struct Layouts {
    known: Vec<(usize, LayoutDeadKeys)>,
}

impl Layouts {
    /// Reads the table of `layout` and looks for its probe; for a layout that is known
    /// already, looks for the probe again if it is still wanted. Not for a hook call.
    pub(super) fn load(&mut self, layout: usize) {
        let translate = |key: &KeyQuery| os::translate(layout, key);
        if let Some((_, known)) = self.known.iter_mut().find(|(known, _)| *known == layout) {
            known.search(translate);
        } else if layout != 0 && self.known.len() < MOST_LAYOUTS {
            self.known
                .push((layout, LayoutDeadKeys::new(read_table(layout), translate)));
        }
    }

    /// What is known about `layout`.
    pub(super) fn get(&self, layout: usize) -> Option<&LayoutDeadKeys> {
        self.known
            .iter()
            .find(|(known, _)| *known == layout)
            .map(|(_, dead_keys)| dead_keys)
    }

    /// True when [`load`](Self::load) should be called for `layout`: it has not been read,
    /// or its probe has not been found.
    pub(super) fn wants_load(&self, layout: usize) -> bool {
        self.get(layout).is_none_or(LayoutDeadKeys::wants_search)
    }

    /// The dead-key table entry of `layout` for `dead` followed by `base`.
    pub(super) fn compose(&self, layout: usize, dead: u16, base: u16) -> Composition {
        self.get(layout)
            .map_or(Composition::Unknown, |known| known.compose(dead, base))
    }

    /// A dead key of `layout`, if it has any and one has been found.
    pub(super) fn probe(&self, layout: usize) -> Option<Probe> {
        self.get(layout).and_then(LayoutDeadKeys::probe)
    }
}

/// The dead-key table of the layout with the handle `layout`; `None` when its file cannot be
/// found or read.
#[cfg(target_pointer_width = "64")]
pub(super) fn read_table(layout: usize) -> Option<DeadKeyTable> {
    let file = layout_file(layout)?;
    let library = Library::load_from_system_directory(&file)?;
    library.dead_keys()
}

/// No table on a system whose pointers are not 64 bits wide (see the module documentation).
#[cfg(not(target_pointer_width = "64"))]
pub(super) fn read_table(_layout: usize) -> Option<DeadKeyTable> {
    None
}

/// A UTF-16 string that ends with a nul and has none before it, as the system's calls want
/// their names.
struct Wide(Vec<u16>);

impl Wide {
    /// `units` up to its first nul, terminated.
    fn new(units: impl IntoIterator<Item = u16>) -> Self {
        Self(
            units
                .into_iter()
                .take_while(|unit| *unit != 0)
                .chain(Some(0))
                .collect(),
        )
    }

    fn of(text: &str) -> Self {
        Self::new(text.encode_utf16())
    }

    /// The units without the terminator.
    fn units(&self) -> &[u16] {
        &self.0[..self.0.len() - 1]
    }

    fn as_ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

/// An open registry key, closed when dropped.
struct Key(HKEY);

impl Key {
    /// Opens the list of installed keyboard layouts for reading.
    fn keyboard_layouts() -> Option<Self> {
        let path = Wide::of("SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts");
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `path` is nul-terminated and outlives the call; `key` is a valid place for
        // the handle.
        let status =
            unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &raw mut key) };
        (status == ERROR_SUCCESS).then_some(Self(key))
    }

    /// The string value `value` of the sub-key `subkey`.
    fn string(&self, subkey: &Wide, value: &str) -> Option<Wide> {
        let value = Wide::of(value);
        let mut text = [0_u16; 260];
        let mut bytes = u32::try_from(size_of_val(&text)).ok()?;
        // SAFETY: `subkey` and `value` are nul-terminated and outlive the call; `text` is
        // writable for `bytes` bytes; with this flag the call returns strings only.
        let status = unsafe {
            RegGetValueW(
                self.0,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                text.as_mut_ptr().cast(),
                &raw mut bytes,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let units = (usize::try_from(bytes).ok()? / 2).min(text.len());
        Some(Wide::new(text[..units].iter().copied()))
    }

    /// The name of the `index`th sub-key.
    fn subkey(&self, index: u32) -> Option<Wide> {
        let mut name = [0_u16; 64];
        let mut length = u32::try_from(name.len()).ok()?;
        // SAFETY: `name` is writable for `length` units, terminator included; the optional
        // pointers are null.
        let status = unsafe {
            RegEnumKeyExW(
                self.0,
                index,
                name.as_mut_ptr(),
                &raw mut length,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let length = usize::try_from(length).ok()?.min(name.len());
        Some(Wide::new(name[..length].iter().copied()))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `RegOpenKeyExW` and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// The name of the registry entry of a layout handle.
///
/// The upper half of a handle names the layout: for most layouts it is the number that is
/// also the entry's name; for an input-method file the whole handle is; and when it starts
/// with the digit F, the rest is a "layout id" that some entry lists as a value.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    /// The entry has this name.
    Named(String),
    /// The entry is the one whose `Layout Id` value is this number.
    WithLayoutId(u32),
}

fn entry_of(layout: usize) -> Entry {
    let handle = layout & 0xFFFF_FFFF;
    let device = handle >> 16;
    match device & 0xF000 {
        0xF000 => Entry::WithLayoutId(u32::try_from(device & 0x0FFF).unwrap_or(0)),
        0xE000 => Entry::Named(format!("{handle:08X}")),
        _ => Entry::Named(format!("{device:08X}")),
    }
}

/// The file name of the layout with the handle `layout`.
fn layout_file(layout: usize) -> Option<Wide> {
    let layouts = Key::keyboard_layouts()?;
    let name =
        match entry_of(layout) {
            Entry::Named(name) => Wide::of(&name),
            Entry::WithLayoutId(wanted) => (0..4_096)
                .map_while(|index| layouts.subkey(index))
                .find(|name| {
                    layouts.string(name, "Layout Id").and_then(|id| {
                        u32::from_str_radix(&String::from_utf16_lossy(id.units()), 16).ok()
                    }) == Some(wanted)
                })?,
        };
    let file = layouts.string(&name, "Layout File")?;
    // A bare file name: the library is loaded from the system directory and nowhere else.
    let is_separator = |unit: &u16| b"\\/:".map(u16::from).contains(unit);
    if file.units().is_empty() || file.units().iter().any(is_separator) {
        return None;
    }
    Some(file)
}

/// A loaded layout file, unloaded when dropped.
struct Library(HMODULE);

impl Library {
    /// Loads the library `file` (a bare file name) from the system directory.
    fn load_from_system_directory(file: &Wide) -> Option<Self> {
        // SAFETY: `file` is nul-terminated and outlives the call; the file handle must be
        // null; the flag limits the search to the system directory, so a file of the same
        // name elsewhere is never loaded.
        let module =
            unsafe { LoadLibraryExW(file.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        (!module.is_null()).then_some(Self(module))
    }

    /// Copies the layout's dead-key table. `None` when the library is not a keyboard layout.
    fn dead_keys(&self) -> Option<DeadKeyTable> {
        // SAFETY: the module is loaded and the name is a nul-terminated constant.
        let descriptor = unsafe { GetProcAddress(self.0, s!("KbdLayerDescriptor")) }?;
        // SAFETY: every keyboard layout exports this function; it takes no arguments and
        // returns the address of the layout's tables (declared as an integer of pointer
        // width by the bindings).
        let tables = unsafe { descriptor() } as *const KBDTABLES;
        if tables.is_null() {
            return None;
        }
        // SAFETY: the address is that of the layout's `KBDTABLES`, which is part of the
        // loaded image and stays valid until `self` is dropped.
        let mut entry: *const DEADKEY = unsafe { ptr::read_unaligned(tables) }.pDeadKey;
        let mut entries = Vec::new();
        // A layout without dead keys has no table.
        while !entry.is_null() && entries.len() < MOST_ENTRIES {
            // SAFETY: `entry` is inside the layout's dead-key table: the table ends with an
            // entry of zeros, and the loop has not passed one.
            let DEADKEY {
                dwBoth: both,
                wchComposed: composed,
                uFlags: flags,
            } = unsafe { ptr::read_unaligned(entry) };
            if both == 0 {
                break;
            }
            // The dead character is the upper half, the character after it the lower.
            let dead = u16::try_from(both >> 16).unwrap_or(0);
            let base = u16::try_from(both & 0xFFFF).unwrap_or(0);
            entries.push((dead, base, composed, u32::from(flags) & DKF_DEAD != 0));
            // SAFETY: the entry just read was not the end marker, so the next one is still
            // inside the table.
            entry = unsafe { entry.add(1) };
        }
        Some(DeadKeyTable::new(entries))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the module was loaded by `LoadLibraryExW` and is released once; nothing
        // keeps an address into it (the table was copied).
        unsafe { FreeLibrary(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_entries_of_layout_handles() {
        assert_eq!(entry_of(0x0409_0409), Entry::Named("00000409".to_owned()));
        // The German layout under the English language.
        assert_eq!(entry_of(0x0407_0409), Entry::Named("00000407".to_owned()));
        assert_eq!(entry_of(0x040A_040A), Entry::Named("0000040A".to_owned()));
        // US International: layout id 1. Handles with the top bit set arrive sign-extended.
        assert_eq!(entry_of(0xF001_0409), Entry::WithLayoutId(1));
        assert_eq!(
            entry_of(usize::MAX & !0x0FFE_FBF6_usize),
            Entry::WithLayoutId(1)
        );
        assert_eq!(entry_of(0xF0C0_0407), Entry::WithLayoutId(0xC0));
        // An input-method file.
        assert_eq!(entry_of(0xE001_0411), Entry::Named("E0010411".to_owned()));
    }

    #[test]
    fn a_file_that_is_not_a_layout_has_no_table() {
        let library = Library::load_from_system_directory(&Wide::of("kernel32.dll"))
            .expect("the system directory has this library");
        assert!(library.dead_keys().is_none());
        assert!(
            Library::load_from_system_directory(&Wide::of("iamb-no-such-layout.dll")).is_none()
        );
        assert!(read_table(0).is_none(), "no layout has the handle 0");
    }

    #[test]
    fn names_end_at_their_first_nul() {
        let name = Wide::new([0x61, 0x62, 0, 0x63]);
        assert_eq!(name.units(), [0x61, 0x62]);
        assert_eq!(name.0, [0x61, 0x62, 0]);
        assert_eq!(Wide::of("").0, [0]);
    }
}
