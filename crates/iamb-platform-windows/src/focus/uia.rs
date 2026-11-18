//! UI Automation, as far as the secure-field question needs it: two interfaces, three
//! methods and `Release`.
//!
//! The bindings crate declares functions and constants but no COM interfaces, so the two
//! method tables are declared here. A COM interface pointer points to a pointer to a table
//! of function pointers in the order of the interface definition (`UIAutomationClient.idl`
//! of the Windows SDK), after the three methods of `IUnknown`. Only the methods called have
//! a name; the slots between them are counted, and the offsets are checked when the crate is
//! compiled.
//!
//! | Interface | Slot | Method |
//! | --- | --- | --- |
//! | both | 2 | `Release` |
//! | `IUIAutomation` | 8 | `GetFocusedElement` |
//! | `IUIAutomationElement` | 21 | `get_CurrentControlType` |
//! | `IUIAutomationElement` | 35 | `get_CurrentIsPassword` |
//!
//! # Privacy
//!
//! Nothing here can read what a field holds. The methods that would (`get_CurrentName`, the
//! value and text patterns, the property getters) have no name in these tables, so no code
//! in this crate can call them; the two that are called return a number and a yes or no.

use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr::{self, NonNull};

use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Accessibility::CUIAutomation;
use windows_sys::core::GUID;

use super::classify::Observation;
use super::inspect::{Probe, ProbeSource};

/// The class of the automation object (`CLSID_CUIAutomation`).
static CLASS_AUTOMATION: GUID = CUIAutomation;
/// `IID_IUIAutomation`.
static IID_AUTOMATION: GUID = GUID::from_u128(0x30cbe57d_d9d0_452a_ab13_7ac5ac4825ee);

/// The size of one slot of a method table.
const SLOT: usize = size_of::<usize>();

type Release = unsafe extern "system" fn(this: *mut c_void) -> u32;

/// The method table of `IUIAutomation`, up to the last method called.
#[repr(C)]
struct AutomationMethods {
    /// `QueryInterface`, `AddRef`.
    _unknown: [usize; 2],
    release: Release,
    /// `CompareElements`, `CompareRuntimeIds`, `GetRootElement`, `ElementFromHandle`,
    /// `ElementFromPoint`.
    _before_focused: [usize; 5],
    get_focused_element:
        unsafe extern "system" fn(this: *mut c_void, element: *mut *mut c_void) -> i32,
}

/// The method table of `IUIAutomationElement`, up to the last method called.
#[repr(C)]
struct ElementMethods {
    /// `QueryInterface`, `AddRef`.
    _unknown: [usize; 2],
    release: Release,
    /// `SetFocus`, `GetRuntimeId`, the four `Find` methods, `BuildUpdatedCache`, the four
    /// property-value getters, the four pattern getters, `GetCachedParent`,
    /// `GetCachedChildren`, `get_CurrentProcessId`.
    _before_control_type: [usize; 18],
    get_current_control_type:
        unsafe extern "system" fn(this: *mut c_void, control_type: *mut i32) -> i32,
    /// `get_CurrentLocalizedControlType`, `get_CurrentName`, `get_CurrentAcceleratorKey`,
    /// `get_CurrentAccessKey`, `get_CurrentHasKeyboardFocus`,
    /// `get_CurrentIsKeyboardFocusable`, `get_CurrentIsEnabled`, `get_CurrentAutomationId`,
    /// `get_CurrentClassName`, `get_CurrentHelpText`, `get_CurrentCulture`,
    /// `get_CurrentIsControlElement`, `get_CurrentIsContentElement`.
    _before_is_password: [usize; 13],
    get_current_is_password:
        unsafe extern "system" fn(this: *mut c_void, is_password: *mut i32) -> i32,
}

const _: () = {
    assert!(offset_of!(AutomationMethods, release) == 2 * SLOT);
    assert!(offset_of!(AutomationMethods, get_focused_element) == 8 * SLOT);
    assert!(offset_of!(ElementMethods, release) == 2 * SLOT);
    assert!(offset_of!(ElementMethods, get_current_control_type) == 21 * SLOT);
    assert!(offset_of!(ElementMethods, get_current_is_password) == 35 * SLOT);
};

/// An `IUIAutomation` this thread holds a reference to.
struct Automation(NonNull<c_void>);

impl Automation {
    fn methods(&self) -> &AutomationMethods {
        // SAFETY: `self.0` is an `IUIAutomation` pointer this value holds a reference to (it
        // comes from `CoCreateInstance` asked for that interface). An interface pointer
        // points to a pointer to the interface's method table, which lives as long as the
        // object, and `AutomationMethods` is the start of that table.
        unsafe { &**self.0.as_ptr().cast::<*const AutomationMethods>() }
    }

    /// The element with the keyboard focus: `Ok(None)` when the system names none.
    fn focused_element(&self) -> Result<Option<Element>, ()> {
        let mut element = ptr::null_mut();
        // SAFETY: the method is called on its own object with a valid place for the result.
        // On success the result is null or an `IUIAutomationElement` whose reference now
        // belongs to the caller.
        let result =
            unsafe { (self.methods().get_focused_element)(self.0.as_ptr(), &raw mut element) };
        if result < 0 {
            return Err(());
        }
        Ok(NonNull::new(element).map(Element))
    }
}

impl Drop for Automation {
    fn drop(&mut self) {
        // SAFETY: the one reference this value holds is given back, once.
        unsafe { (self.methods().release)(self.0.as_ptr()) };
    }
}

/// An `IUIAutomationElement` this thread holds a reference to.
struct Element(NonNull<c_void>);

impl Element {
    fn methods(&self) -> &ElementMethods {
        // SAFETY: `self.0` is an `IUIAutomationElement` pointer this value holds a reference
        // to (it comes from `GetFocusedElement`). An interface pointer points to a pointer
        // to the interface's method table, which lives as long as the object, and
        // `ElementMethods` is the start of that table.
        unsafe { &**self.0.as_ptr().cast::<*const ElementMethods>() }
    }

    /// The element's `IsPassword` property.
    fn is_password(&self) -> Result<bool, ()> {
        let mut is_password = 0;
        // SAFETY: the method is called on its own object with a valid place for a `BOOL`.
        let result = unsafe {
            (self.methods().get_current_is_password)(self.0.as_ptr(), &raw mut is_password)
        };
        if result < 0 {
            Err(())
        } else {
            Ok(is_password != 0)
        }
    }

    /// The element's control type.
    fn control_type(&self) -> Result<i32, ()> {
        let mut control_type = 0;
        // SAFETY: the method is called on its own object with a valid place for the
        // control type, a 32-bit number.
        let result = unsafe {
            (self.methods().get_current_control_type)(self.0.as_ptr(), &raw mut control_type)
        };
        if result < 0 {
            Err(())
        } else {
            Ok(control_type)
        }
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        // SAFETY: the one reference this value holds is given back, once.
        unsafe { (self.methods().release)(self.0.as_ptr()) };
    }
}

/// UI Automation on the thread that opened it.
pub(super) struct Session {
    /// `None` only while the session is being opened or dropped.
    automation: Option<Automation>,
}

impl Session {
    /// Initialises COM on the calling thread and creates the automation object. The thread
    /// must be one this module owns: it joins the multithreaded apartment.
    pub(super) fn open() -> Option<Self> {
        // SAFETY: the reserved pointer is null; the call is balanced by the `CoUninitialize`
        // in `Drop`, which runs on this thread because the session cannot be sent to
        // another one.
        let initialised =
            unsafe { CoInitializeEx(ptr::null(), COINIT_MULTITHREADED.cast_unsigned()) };
        if initialised < 0 {
            return None;
        }
        // From here on dropping the session uninitialises COM.
        let mut session = Self { automation: None };
        let mut automation = ptr::null_mut();
        // SAFETY: the class and interface identifiers are valid for the call, there is no
        // outer object, and `automation` is a valid place for the result. The object is
        // created inside this process; nothing is started and nothing is reached over a
        // network.
        let created = unsafe {
            CoCreateInstance(
                &raw const CLASS_AUTOMATION,
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &raw const IID_AUTOMATION,
                &raw mut automation,
            )
        };
        if created < 0 {
            return None;
        }
        session.automation = Some(Automation(NonNull::new(automation)?));
        Some(session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The object goes before the apartment it lives in.
        self.automation = None;
        // SAFETY: balances the successful `CoInitializeEx` of `open`, on the same thread.
        unsafe { CoUninitialize() };
    }
}

impl Probe for Session {
    fn observe(&mut self) -> Observation {
        let Some(automation) = &self.automation else {
            return Observation::Failed;
        };
        let element = match automation.focused_element() {
            Ok(Some(element)) => element,
            Ok(None) => return Observation::NoElement,
            Err(()) => return Observation::Failed,
        };
        match (element.is_password(), element.control_type()) {
            (Ok(password), Ok(control_type)) => Observation::Element {
                password,
                control_type,
            },
            _ => Observation::Failed,
        }
    }
}

/// Opens a [`Session`] on the inspection thread.
#[derive(Debug, Default)]
pub(super) struct Source;

impl ProbeSource for Source {
    type Probe = Session;

    fn open(&self) -> Option<Session> {
        Session::open()
    }
}
