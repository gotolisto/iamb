//! Tests that need Windows: the listener's translation against the real layouts, and the hook
//! against real input events.
//!
//! They run in the Windows leg of CI. A test that needs something the session does not have
//! (a desktop that delivers synthetic input, a layout file) says `SKIPPED` and why, and fails
//! instead when `IAMB_REQUIRE_DESKTOP` is set.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use iamb_platform::{KeyListener, ListenerEvent, event_channel};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse as kam;
use windows_sys::Win32::UI::WindowsAndMessaging as wam;

use super::deadkey::{
    Composition, DeadKeyTable, DeadKeys, KeyQuery, Layout, LayoutDeadKeys, MAX_UNITS, Probe,
    Resolved, Translation, Units, find_probe,
};
use super::keys;
use super::keystate::KeyState;
use super::layouts::{Layouts, read_table};
use super::os::{self, Hook, HookKind, Message};
use super::thread::timing;
use crate::Platform;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The tests in this file share the keyboard: one at a time.
fn alone() -> MutexGuard<'static, ()> {
    static KEYBOARD: Mutex<()> = Mutex::new(());
    KEYBOARD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says why a test did not run. With `IAMB_REQUIRE_DESKTOP` set that is a failure.
fn skip(why: &str) {
    assert!(
        std::env::var_os("IAMB_REQUIRE_DESKTOP").is_none(),
        "IAMB_REQUIRE_DESKTOP is set, but {why}"
    );
    eprintln!("SKIPPED: {why}");
}

fn unit(ch: char) -> u16 {
    u16::try_from(u32::from(ch)).expect("a character of the Basic Multilingual Plane")
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

const VK_E: u32 = 0x45;
const VK_Q: u32 = 0x51;
const VK_U: u32 = 0x55;

const NO_KEYS: &[u32] = &[];
const SHIFT: &[u32] = &[keys::VK_LSHIFT];
const ALTGR: &[u32] = &[keys::VK_LCONTROL, keys::VK_RMENU];
const SHIFT_ALTGR: &[u32] = &[keys::VK_LSHIFT, keys::VK_LCONTROL, keys::VK_RMENU];

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

#[test]
fn the_numbers_written_out_for_every_system_are_the_systems_own() {
    let virtual_keys: [(u32, u16); 31] = [
        (keys::VK_BACK, kam::VK_BACK),
        (keys::VK_TAB, kam::VK_TAB),
        (keys::VK_RETURN, kam::VK_RETURN),
        (keys::VK_SHIFT, kam::VK_SHIFT),
        (keys::VK_CONTROL, kam::VK_CONTROL),
        (keys::VK_MENU, kam::VK_MENU),
        (keys::VK_CAPITAL, kam::VK_CAPITAL),
        (keys::VK_KANA, kam::VK_KANA),
        (keys::VK_ESCAPE, kam::VK_ESCAPE),
        (keys::VK_MODECHANGE, kam::VK_MODECHANGE),
        (keys::VK_SPACE, kam::VK_SPACE),
        (keys::VK_PRIOR, kam::VK_PRIOR),
        (keys::VK_NEXT, kam::VK_NEXT),
        (keys::VK_END, kam::VK_END),
        (keys::VK_HOME, kam::VK_HOME),
        (keys::VK_LEFT, kam::VK_LEFT),
        (keys::VK_UP, kam::VK_UP),
        (keys::VK_RIGHT, kam::VK_RIGHT),
        (keys::VK_DOWN, kam::VK_DOWN),
        (keys::VK_DELETE, kam::VK_DELETE),
        (keys::VK_LWIN, kam::VK_LWIN),
        (keys::VK_RWIN, kam::VK_RWIN),
        (keys::VK_F1, kam::VK_F1),
        (keys::VK_F10, kam::VK_F10),
        (keys::VK_NUMLOCK, kam::VK_NUMLOCK),
        (keys::VK_SCROLL, kam::VK_SCROLL),
        (keys::VK_LSHIFT, kam::VK_LSHIFT),
        (keys::VK_RMENU, kam::VK_RMENU),
        (keys::VK_PROCESSKEY, kam::VK_PROCESSKEY),
        (keys::VK_PACKET, kam::VK_PACKET),
        (keys::VK_DBE_FIRST, kam::VK_DBE_ALPHANUMERIC),
    ];
    for (ours, theirs) in virtual_keys {
        assert_eq!(ours, u32::from(theirs));
    }
    assert_eq!(keys::VK_RSHIFT, u32::from(kam::VK_RSHIFT));
    assert_eq!(keys::VK_LCONTROL, u32::from(kam::VK_LCONTROL));
    assert_eq!(keys::VK_RCONTROL, u32::from(kam::VK_RCONTROL));
    assert_eq!(keys::VK_LMENU, u32::from(kam::VK_LMENU));
    assert_eq!(keys::VK_DBE_LAST, u32::from(kam::VK_DBE_NOROMAN));

    assert_eq!(keys::LLKHF_INJECTED, wam::LLKHF_INJECTED);
    assert_eq!(keys::LLKHF_UP, wam::LLKHF_UP);
    assert_eq!(keys::LLMHF_INJECTED, wam::LLMHF_INJECTED);
    assert_eq!(keys::WM_KEYDOWN, wam::WM_KEYDOWN);
    assert_eq!(keys::WM_KEYUP, wam::WM_KEYUP);
    assert_eq!(keys::WM_SYSKEYDOWN, wam::WM_SYSKEYDOWN);
    assert_eq!(keys::WM_SYSKEYUP, wam::WM_SYSKEYUP);
    assert_eq!(keys::WM_LBUTTONDOWN, wam::WM_LBUTTONDOWN);
    assert_eq!(keys::WM_RBUTTONDOWN, wam::WM_RBUTTONDOWN);
    assert_eq!(keys::WM_MBUTTONDOWN, wam::WM_MBUTTONDOWN);
    assert_eq!(keys::WM_XBUTTONDOWN, wam::WM_XBUTTONDOWN);
}

// ---------------------------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------------------------

/// The four layouts of the delivery KPI: name, registry name, language.
const US: (&str, &str, usize) = ("US", "00000409", 0x0409);
const SPANISH: (&str, &str, usize) = ("Spanish", "0000040A", 0x040A);
const GERMAN: (&str, &str, usize) = ("German", "00000407", 0x0407);
const FRENCH: (&str, &str, usize) = ("French", "0000040C", 0x040C);

/// A layout loaded for a test. Dropping it unloads the layout if the test loaded it, and puts
/// the thread's active layout back if loading changed it.
struct Loaded {
    handle: usize,
    name: &'static str,
    unload: bool,
    active_before: usize,
}

impl Loaded {
    /// Loads a layout without activating it. `None`, with the reason printed, when the system
    /// does not have it.
    fn load((name, registry_name, language): (&'static str, &str, usize)) -> Option<Self> {
        let active_before = os::layout_of_thread(0);
        let before = os::keyboard_layouts();
        let wide: Vec<u16> = registry_name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is nul-terminated and outlives the call. Without the activate flag
        // the layout is loaded and nothing else changes.
        let handle = unsafe { kam::LoadKeyboardLayoutW(wide.as_ptr(), kam::KLF_NOTELLSHELL) };
        let handle = handle as usize;
        let loaded = Self {
            handle,
            name,
            unload: handle != 0 && !before.contains(&handle),
            active_before,
        };
        // The system answers with another layout when it does not have the one asked for. The
        // upper half of a handle names the layout.
        if (handle >> 16) & 0xFFFF != language {
            skip(&format!(
                "the {name} layout ({registry_name}) is not available: the system answered \
                 with the layout {handle:#x}"
            ));
            return None;
        }
        Some(loaded)
    }

    fn key(&self, vk: u32, held: &[u32]) -> KeyQuery {
        // SAFETY: plain values; an unknown key is answered with 0.
        let scan =
            unsafe { kam::MapVirtualKeyExW(vk, kam::MAPVK_VK_TO_VSC, self.handle as kam::HKL) };
        KeyQuery {
            vk,
            scan,
            keys: KeyState::holding(held),
        }
    }

    /// The first key that translates to `wanted` with nothing, Shift, `AltGr` or both held.
    fn key_for(&self, wanted: Translation) -> KeyQuery {
        for held in [NO_KEYS, SHIFT, ALTGR, SHIFT_ALTGR] {
            for vk in 0x20..0xFF {
                let key = self.key(vk, held);
                if os::translate(self.handle, &key) == wanted {
                    return key;
                }
            }
        }
        panic!("the {} layout has no key for that", self.name);
    }

    fn dead_key(&self, ch: char) -> KeyQuery {
        self.key_for(Translation::Dead(unit(ch)))
    }
}

impl Drop for Loaded {
    fn drop(&mut self) {
        if os::layout_of_thread(0) != self.active_before {
            // SAFETY: the handle is the one the thread had before; flags 0 change nothing
            // else.
            unsafe { kam::ActivateKeyboardLayout(self.active_before as kam::HKL, 0) };
        }
        if self.unload {
            // SAFETY: the handle came from `LoadKeyboardLayoutW` in `load`.
            unsafe { kam::UnloadKeyboardLayout(self.handle as kam::HKL) };
        }
    }
}

/// The system's own translation, the one an application's message loop uses: it stores a
/// dead key and applies it to the next key. Returns what the key delivers.
fn stateful(layout: usize, key: &KeyQuery) -> (i32, Vec<u16>) {
    let mut state = [0_u8; 256];
    for vk in key.keys.held() {
        state[vk as usize] = 0x80;
    }
    for (held, vk) in [
        (key.keys.shift(), keys::VK_SHIFT),
        (key.keys.ctrl(), keys::VK_CONTROL),
        (key.keys.alt(), keys::VK_MENU),
    ] {
        if held {
            state[vk as usize] = 0x80;
        }
    }
    let mut units = [0_u16; MAX_UNITS];
    // SAFETY: `state` is the 256-byte array the call reads and `units` has room for the
    // count passed.
    let count = unsafe {
        kam::ToUnicodeEx(
            key.vk,
            key.scan,
            state.as_ptr(),
            units.as_mut_ptr(),
            i32::try_from(MAX_UNITS).expect("a small count"),
            0,
            layout as kam::HKL,
        )
    };
    let delivered =
        usize::try_from(count).map_or(Vec::new(), |n| units[..n.min(MAX_UNITS)].to_vec());
    (count, delivered)
}

/// Makes the system forget a stored dead key of `layout`.
fn forget_stored_dead_key(layout: &Loaded) {
    let space = layout.key(keys::VK_SPACE, NO_KEYS);
    for _ in 0..3 {
        if stateful(layout.handle, &space).0 >= 0 {
            break;
        }
    }
    // The first space may have delivered the stored accent; a second one is plain.
    let _ = stateful(layout.handle, &space);
}

/// What an application receives for each of `keys`, typed in order from a clean state.
fn application_receives(layout: &Loaded, keys: &[KeyQuery]) -> Vec<Vec<u16>> {
    forget_stored_dead_key(layout);
    let received = keys
        .iter()
        .map(|key| stateful(layout.handle, key).1)
        .collect();
    forget_stored_dead_key(layout);
    received
}

/// The layout as the listener's dead-key logic sees it on Windows.
struct OnWindows<'a> {
    layout: usize,
    layouts: &'a Layouts,
}

impl Layout for OnWindows<'_> {
    fn translate(&mut self, key: &KeyQuery) -> Translation {
        os::translate(self.layout, key)
    }

    fn compose(&mut self, dead: u16, base: u16) -> Composition {
        self.layouts.compose(self.layout, dead, base)
    }

    fn probe(&mut self) -> Option<Probe> {
        self.layouts.probe(self.layout)
    }
}

/// What the listener reports for each of `keys`, typed in order: `None` for "cannot know".
fn listener_reports(
    layout: &Loaded,
    layouts: &Layouts,
    keys: &[KeyQuery],
) -> Vec<Option<Vec<u16>>> {
    // No application is in the middle of a dead-key sequence.
    forget_stored_dead_key(layout);
    let mut dead = DeadKeys::default();
    let mut on = OnWindows {
        layout: layout.handle,
        layouts,
    };
    keys.iter()
        .map(|key| match dead.resolve(&mut on, key) {
            Resolved::Nothing | Resolved::Held => Some(Vec::new()),
            Resolved::Text(units) => Some(units.as_slice().to_vec()),
            Resolved::Unknown => None,
        })
        .collect()
}

fn known(layout: &Loaded) -> Layouts {
    // No application is in the middle of a dead-key sequence while the probe is looked for.
    forget_stored_dead_key(layout);
    let mut layouts = Layouts::default();
    layouts.load(layout.handle);
    layouts
}

#[test]
fn the_kpi_translations_of_the_four_layouts() {
    let _alone = alone();
    let typed = |layout: &Loaded, keys: &[KeyQuery]| -> String {
        let layouts = known(layout);
        let reported = listener_reports(layout, &layouts, keys);
        let received = application_receives(layout, keys);
        let mut all = Vec::new();
        for (i, (reported, received)) in reported.iter().zip(&received).enumerate() {
            let reported = reported
                .as_ref()
                .unwrap_or_else(|| panic!("{}: key {i} is unknown", layout.name));
            assert_eq!(
                text(reported),
                text(received),
                "{}: key {i}: the listener and the system disagree",
                layout.name
            );
            all.extend(reported);
        }
        text(&all)
    };

    if let Some(us) = Loaded::load(US) {
        let keys = [
            us.key(0x48, NO_KEYS),
            us.key(0x49, SHIFT),
            us.key(0x32, SHIFT),
            us.key(keys::VK_SPACE, NO_KEYS),
            us.key(0xBF, SHIFT),
        ];
        assert_eq!(typed(&us, &keys), "hI@ ?");
        // No AltGr level: Ctrl+Alt+E types nothing.
        assert_eq!(
            os::translate(us.handle, &us.key(VK_E, ALTGR)),
            Translation::None
        );
    }
    if let Some(spanish) = Loaded::load(SPANISH) {
        let acute = spanish.dead_key('´');
        let e = spanish.key(VK_E, NO_KEYS);
        assert_eq!(typed(&spanish, &[acute, e]), "é");
        assert_eq!(typed(&spanish, &[acute, spanish.key(VK_E, SHIFT)]), "É");
        assert_eq!(typed(&spanish, &[acute, spanish.key(VK_Q, NO_KEYS)]), "´q");
        assert_eq!(typed(&spanish, &[acute, acute]), "´´");
        assert_eq!(
            typed(&spanish, &[acute, spanish.key(keys::VK_SPACE, NO_KEYS)]),
            "´"
        );
        let diaeresis = spanish.dead_key('¨');
        assert_eq!(
            typed(&spanish, &[diaeresis, spanish.key(VK_U, NO_KEYS)]),
            "ü"
        );
        assert_eq!(
            typed(
                &spanish,
                &[spanish.key_for(Translation::text(&[unit('ñ')]))]
            ),
            "ñ"
        );
    }
    if let Some(german) = Loaded::load(GERMAN) {
        assert_eq!(typed(&german, &[german.key(VK_Q, ALTGR)]), "@");
        assert_eq!(typed(&german, &[german.key(VK_E, ALTGR)]), "€");
        let circumflex = german.dead_key('^');
        assert_eq!(
            typed(&german, &[circumflex, german.key(VK_E, NO_KEYS)]),
            "ê"
        );
        // A dead key, then an AltGr character: both are delivered.
        assert_eq!(typed(&german, &[circumflex, german.key(VK_Q, ALTGR)]), "^@");
    }
    if let Some(french) = Loaded::load(FRENCH) {
        let circumflex = french.dead_key('^');
        assert_eq!(
            typed(&french, &[circumflex, french.key(VK_E, NO_KEYS)]),
            "ê"
        );
        assert_eq!(typed(&french, &[french.key(VK_E, ALTGR)]), "€");
        // The French layout has an é key, and ^ does not compose with it.
        let e_acute = french.key_for(Translation::text(&[unit('é')]));
        assert_eq!(typed(&french, &[circumflex, e_acute]), "^é");
    }
}

/// Every dead key of each layout followed by every key that types one character or is a dead
/// key, and then a space: the listener reports exactly what the system's own stateful
/// translation delivers.
#[test]
fn every_dead_key_sequence_of_the_four_layouts_matches_the_system() {
    let _alone = alone();
    for which in [US, SPANISH, GERMAN, FRENCH] {
        let Some(layout) = Loaded::load(which) else {
            continue;
        };
        let layouts = known(&layout);
        let mut dead_keys = Vec::new();
        let mut keys = vec![
            layout.key(keys::VK_BACK, NO_KEYS),
            layout.key(keys::VK_RETURN, NO_KEYS),
            layout.key(keys::VK_TAB, NO_KEYS),
            layout.key(keys::VK_ESCAPE, NO_KEYS),
            layout.key(keys::VK_LEFT, NO_KEYS),
        ];
        for held in [NO_KEYS, SHIFT, ALTGR, SHIFT_ALTGR] {
            for vk in 0x20..0xFF {
                let key = layout.key(vk, held);
                match os::translate(layout.handle, &key) {
                    Translation::Dead(_) => {
                        dead_keys.push(key);
                        keys.push(key);
                    }
                    Translation::Text(units) if units.as_slice().len() == 1 => keys.push(key),
                    Translation::Text(_) | Translation::None => {}
                }
            }
        }
        let has_dead_keys = which != US;
        assert_eq!(!dead_keys.is_empty(), has_dead_keys, "{}", layout.name);
        assert_eq!(
            layouts.probe(layout.handle).is_some(),
            has_dead_keys,
            "{}: a probe exactly when the layout has dead keys",
            layout.name
        );
        let table = layouts
            .get(layout.handle)
            .and_then(LayoutDeadKeys::table)
            .unwrap_or_else(|| panic!("the {} layout's file could not be read", layout.name));
        assert!(!layouts.wants_load(layout.handle), "{}", layout.name);
        assert_eq!(!table.is_empty(), has_dead_keys, "{}", layout.name);

        let space = layout.key(keys::VK_SPACE, NO_KEYS);
        let mut sequences = 0_usize;
        for dead_key in &dead_keys {
            for key in &keys {
                let sequence = [*dead_key, *key, space];
                let reported = listener_reports(&layout, &layouts, &sequence);
                let received = application_receives(&layout, &sequence);
                for (i, (reported, received)) in reported.iter().zip(&received).enumerate() {
                    let reported = reported.as_ref().map(|units| text(units));
                    assert_eq!(
                        reported,
                        Some(text(received)),
                        "{}: dead key {:#x} then key {:#x}, position {i}",
                        layout.name,
                        dead_key.vk,
                        key.vk
                    );
                }
                sequences += 1;
            }
        }
        eprintln!(
            "{}: {} dead keys, {} table pairs, {sequences} sequences match the system",
            layout.name,
            dead_keys.len(),
            table.len()
        );
    }
}

/// The translation the listener uses leaves the system's dead-key state alone, in both
/// directions: it stores nothing, and it uses up nothing an application has stored. And the
/// listener is right whether or not the system lets that stored dead key show through.
#[test]
fn translating_never_disturbs_the_applications_dead_keys() {
    let _alone = alone();
    let Some(spanish) = Loaded::load(SPANISH) else {
        return;
    };
    let layouts = known(&spanish);
    let acute = spanish.dead_key('´');
    let grave = spanish.dead_key('`');
    let e = spanish.key(VK_E, NO_KEYS);
    let e_acute = vec![unit('é')];

    // The listener translates the dead key (as its hook does before the application sees
    // the key): the system must not have stored it.
    forget_stored_dead_key(&spanish);
    assert_eq!(
        os::translate(spanish.handle, &acute),
        Translation::Dead(unit('´'))
    );
    assert_eq!(
        stateful(spanish.handle, &e).1,
        vec![unit('e')],
        "the listener's translation stored the dead key"
    );

    // The application has stored the dead key (it is in step with the typing). The listener
    // translates the next key any number of times: the application still gets its é.
    forget_stored_dead_key(&spanish);
    let mut dead = DeadKeys::default();
    let mut on = OnWindows {
        layout: spanish.handle,
        layouts: &layouts,
    };
    assert_eq!(dead.resolve(&mut on, &acute), Resolved::Held);
    assert_eq!(
        stateful(spanish.handle, &acute).0,
        -1,
        "the application's ´"
    );
    let shows_through = os::translate(spanish.handle, &e) == Translation::text(&e_acute);
    eprintln!(
        "note: on this system the stateless translation {} a dead key an application has stored",
        if shows_through { "applies" } else { "ignores" }
    );
    for _ in 0..3 {
        let _ = os::translate(spanish.handle, &e);
    }
    assert_eq!(
        dead.resolve(&mut on, &e),
        Resolved::Text(Units::new(&e_acute)),
        "in step with the application"
    );
    assert_eq!(
        stateful(spanish.handle, &e).1,
        e_acute,
        "the listener's translations used up the application's dead key"
    );

    // The application lags: it has stored nothing yet when the listener sees both keys.
    forget_stored_dead_key(&spanish);
    let mut dead = DeadKeys::default();
    assert_eq!(dead.resolve(&mut on, &acute), Resolved::Held);
    assert_eq!(
        dead.resolve(&mut on, &e),
        Resolved::Text(Units::new(&e_acute)),
        "ahead of the application"
    );

    // The application has stored another dead key than the one the listener tracks (it is
    // two dead keys behind). The listener may not know, but it must not be wrong.
    forget_stored_dead_key(&spanish);
    let mut dead = DeadKeys::default();
    assert_eq!(dead.resolve(&mut on, &acute), Resolved::Held);
    assert_eq!(
        stateful(spanish.handle, &grave).0,
        -1,
        "the application's `"
    );
    let resolved = dead.resolve(&mut on, &e);
    assert!(
        resolved == Resolved::Unknown
            || (!shows_through && resolved == Resolved::Text(Units::new(&e_acute))),
        "a guess from a dead key that is not the listener's"
    );
    forget_stored_dead_key(&spanish);
}

#[test]
fn the_dead_key_tables_are_read_from_the_layout_files() {
    let _alone = alone();
    let compose = |table: &DeadKeyTable, dead: char, base: char| match table
        .compose(unit(dead), unit(base))
    {
        Composition::Char(ch) => Some(text(&[ch])),
        _ => None,
    };
    if let Some(spanish) = Loaded::load(SPANISH) {
        let table = read_table(spanish.handle).expect("the Spanish layout's file");
        assert_eq!(compose(&table, '´', 'e').as_deref(), Some("é"));
        assert_eq!(compose(&table, '¨', 'u').as_deref(), Some("ü"));
        assert_eq!(compose(&table, '`', 'a').as_deref(), Some("à"));
        assert_eq!(compose(&table, '^', 'o').as_deref(), Some("ô"));
        assert_eq!(compose(&table, '´', ' ').as_deref(), Some("´"));
        assert_eq!(compose(&table, '´', 'q'), None);
        assert!(table.is_dead(unit('´')) && !table.is_dead(unit('e')));
        // The probe is one of the layout's dead keys.
        let probe = find_probe(
            |key| os::translate(spanish.handle, key),
            |ch| table.is_dead(ch),
        )
        .expect("a dead key");
        assert!(table.is_dead(probe.ch));
    }
    if let Some(french) = Loaded::load(FRENCH) {
        let table = read_table(french.handle).expect("the French layout's file");
        assert_eq!(compose(&table, '^', 'e').as_deref(), Some("ê"));
        assert_eq!(compose(&table, '^', 'é'), None);
    }
    if let Some(us) = Loaded::load(US) {
        let table = read_table(us.handle).expect("the US layout's file");
        assert!(table.is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// The hook, against real input events
// ---------------------------------------------------------------------------------------------

/// Marks the key events of this file, in the extra-info field. Not an Iamb marker: the
/// listener takes these events for real typing.
const TEST_TAG: usize = 0x7E57_1A3B_0000_0001;

/// How many tagged events the swallowing hook has taken out of the stream.
static SWALLOWED: AtomicUsize = AtomicUsize::new(0);

/// A hook procedure that swallows the tagged events of this file, so that they reach neither
/// an application nor the system's picture of the keyboard.
unsafe extern "system" fn swallow_tagged(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(wam::HC_ACTION) {
        // SAFETY: this is a `WH_KEYBOARD_LL` hook procedure called with `HC_ACTION`, and it
        // has not returned: `lparam` points to the event.
        let event = unsafe { os::keyboard_event(lparam) };
        if event.flags & wam::LLKHF_INJECTED != 0 && event.dwExtraInfo == TEST_TAG {
            SWALLOWED.fetch_add(1, Ordering::SeqCst);
            return 1;
        }
    }
    os::call_next_hook(code, wparam, lparam)
}

/// A thread with the swallowing hook. Install it *before* the listener starts: hooks are
/// called newest first, so the listener sees each event and this hook then swallows it.
struct Swallower {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl Swallower {
    fn start() -> Option<Self> {
        let (ready, announced) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            os::create_message_queue();
            let hook = Hook::install(HookKind::Keyboard, swallow_tagged);
            let _ = ready.send(hook.is_ok().then(os::current_thread_id));
            if let Ok(hook) = hook {
                while !matches!(os::next_message(), Message::Stop | Message::Failed(_)) {}
                let _ = hook.remove();
            }
        });
        if let Ok(Some(thread_id)) = announced.recv() {
            Some(Self {
                thread: Some(thread),
                thread_id,
            })
        } else {
            let _ = thread.join();
            None
        }
    }
}

impl Drop for Swallower {
    fn drop(&mut self) {
        let _ = os::post_thread_message(self.thread_id, os::WM_STOP, 0);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Posts tagged key events through the system's input stream: `(virtual key, release)`.
fn send_tagged(events: &[(u32, bool)]) -> usize {
    let inputs: Vec<kam::INPUT> = events
        .iter()
        .map(|&(vk, release)| kam::INPUT {
            r#type: kam::INPUT_KEYBOARD,
            Anonymous: kam::INPUT_0 {
                ki: kam::KEYBDINPUT {
                    wVk: u16::try_from(vk).expect("a virtual key"),
                    wScan: 0,
                    dwFlags: if release { kam::KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: TEST_TAG,
                },
            },
        })
        .collect();
    let count = u32::try_from(inputs.len()).expect("a small batch");
    let size = i32::try_from(size_of::<kam::INPUT>()).expect("a small structure");
    // SAFETY: `inputs` holds `count` structures of the size passed.
    let sent = unsafe { kam::SendInput(count, inputs.as_ptr(), size) };
    usize::try_from(sent).expect("a count")
}

/// A swallowing hook, if this session's desktop delivers synthetic input to hooks.
fn desktop() -> Option<Swallower> {
    let Some(swallower) = Swallower::start() else {
        skip("this session does not allow a low-level keyboard hook");
        return None;
    };
    let before = SWALLOWED.load(Ordering::SeqCst);
    let sent = send_tagged(&[(keys::VK_UNASSIGNED, true)]);
    let deadline = Instant::now() + Duration::from_secs(2);
    while SWALLOWED.load(Ordering::SeqCst) == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if sent != 1 || SWALLOWED.load(Ordering::SeqCst) == before {
        skip("this session's desktop does not deliver synthetic input (no interactive desktop)");
        return None;
    }
    Some(swallower)
}

/// The letter keys and the character each types right now, as the listener will see it: on
/// the layout of the window in front, lower case.
fn letters() -> Vec<(u32, String)> {
    let front = os::foreground();
    let layout = if front.layout == 0 {
        os::layout_of_thread(0)
    } else {
        front.layout
    };
    (0x41..=0x5A)
        .filter_map(|vk| {
            let key = KeyQuery {
                vk,
                scan: 0,
                keys: KeyState::default(),
            };
            match os::translate(layout, &key) {
                Translation::Text(units) if units.as_slice().len() == 1 => {
                    Some((vk, text(units.as_slice()).to_lowercase()))
                }
                _ => None,
            }
        })
        .collect()
}

/// The delivery KPI for latency: the keyboard hook procedure's own work takes under 1 ms at
/// the 99th percentile over 10,000 key presses (and their releases). The time of the whole
/// call is printed too; it includes the hooks after the listener's, which here is this
/// file's swallowing hook on another thread. Also checks that every key arrives, in order,
/// and that every event was passed on to the hooks after the listener's.
///
/// Other tests of this crate may inject input while this one runs, so this test only checks
/// that its own keys arrive in order; `tests/listener_e2e.rs` checks "exactly once" with the
/// keyboard to itself.
#[test]
fn ten_thousand_keys_and_the_hook_stays_under_a_millisecond() {
    const KEYS: usize = 10_000;
    const BATCH: usize = 50;
    let _alone = alone();
    let Some(_swallower) = desktop() else {
        return;
    };
    let letters = letters();
    if letters.len() < 2 {
        skip("the layout of the window in front types no letters");
        return;
    }
    let platform = Platform::new();
    let (sink, events) = event_channel(4 * KEYS);
    platform.start(Box::new(sink)).expect("the listener starts");
    timing::reset();
    let swallowed_before = SWALLOWED.load(Ordering::SeqCst);

    let mut expected = Vec::with_capacity(KEYS);
    let mut typed = 0;
    while typed < KEYS {
        let mut batch = Vec::with_capacity(2 * BATCH);
        for i in typed..(typed + BATCH).min(KEYS) {
            let (vk, ch) = &letters[i % letters.len()];
            batch.extend([(*vk, false), (*vk, true)]);
            expected.push(ch.clone());
        }
        assert_eq!(
            send_tagged(&batch),
            batch.len(),
            "the system took the batch"
        );
        typed += batch.len() / 2;
    }
    platform.stop().expect("the listener stops");

    // Every event went on to the hooks after the listener's: nothing was swallowed by it.
    assert_eq!(
        SWALLOWED.load(Ordering::SeqCst) - swallowed_before,
        2 * KEYS,
        "events the listener did not pass on"
    );

    // Every key arrived, in order.
    let mut matched = 0;
    let mut last_seq = 0;
    for event in events.try_iter() {
        match event {
            ListenerEvent::Overflow => panic!("the listener lost events"),
            ListenerEvent::Char { ch, seq } => {
                assert!(seq > last_seq, "sequence numbers grow");
                last_seq = seq;
                let got = ch.expose().to_lowercase().to_string();
                if matched < KEYS && got == expected[matched] {
                    matched += 1;
                }
            }
            _ => {}
        }
    }
    assert_eq!(matched, KEYS, "keys that did not arrive in order");

    let (mut own, mut whole) = timing::samples();
    assert!(own.len() >= 2 * KEYS, "{} hook calls were timed", own.len());
    own.sort_unstable();
    whole.sort_unstable();
    let percentile = |sorted: &[u32], p: usize| {
        Duration::from_nanos(u64::from(sorted[(sorted.len() - 1) * p / 100]))
    };
    eprintln!(
        "keyboard hook procedure over {} calls, its own work: p50 = {:?}, p99 = {:?}, max = {:?}",
        own.len(),
        percentile(&own, 50),
        percentile(&own, 99),
        percentile(&own, 100)
    );
    eprintln!(
        "keyboard hook procedure, whole call with the hooks after it: p50 = {:?}, p99 = {:?}, \
         max = {:?}",
        percentile(&whole, 50),
        percentile(&whole, 99),
        percentile(&whole, 100)
    );
    let p99 = percentile(&own, 99);
    assert!(
        p99 < Duration::from_millis(1),
        "the 99th percentile is {p99:?}"
    );
}

/// What the hook reads when it is installed and after a pause.
#[test]
fn the_physical_keyboard_can_be_read() {
    let _alone = alone();
    assert!(
        !os::physically_down(keys::VK_UNASSIGNED),
        "a key no keyboard has"
    );
    assert!(!os::physically_down(0xFFFF_FFFF), "out of range");
    // The toggle is read on a thread created for the question; asking does not change it.
    let _ = os::caps_lock_on();
    assert!(
        os::layout_of_thread(0) != 0,
        "the calling thread has a layout"
    );
    let layouts = os::keyboard_layouts();
    assert!(!layouts.is_empty() && !layouts.contains(&0));
}
