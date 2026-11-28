//! UI Automation, as far as the secure-field question needs it: three interfaces, seven
//! methods and `Release`.
//!
//! The bindings crate declares functions and constants but no COM interfaces, so the method
//! tables are declared here. A COM interface pointer points to a pointer to a table of function
//! pointers in the order of the interface definition (`UIAutomationClient.h` of the Windows
//! SDK), after the three methods of `IUnknown`. Only the methods called have a name; the slots
//! between them are counted, and the offsets are checked when the crate is compiled.
//!
//! | Interface | Slot | Method |
//! | --- | --- | --- |
//! | all three | 2 | `Release` |
//! | `IUIAutomation` | 12 | `GetFocusedElementBuildCache` |
//! | `IUIAutomation` | 20 | `CreateCacheRequest` |
//! | `IUIAutomationCacheRequest` | 3 | `AddProperty` |
//! | `IUIAutomationElement` | 52 | `get_CachedProcessId` |
//! | `IUIAutomationElement` | 53 | `get_CachedControlType` |
//! | `IUIAutomationElement` | 58 | `get_CachedHasKeyboardFocus` |
//! | `IUIAutomationElement` | 67 | `get_CachedIsPassword` |
//! | `IUIAutomationElement` | 68 | `get_CachedNativeWindowHandle` |
//! | `IUIAutomationElement` | 72 | `get_CachedFrameworkId` |
//!
//! The slot numbers were counted from the SDK's interface definitions as the `windows` crate
//! 0.62 generates them (`IUIAutomation_Vtbl`, `IUIAutomationCacheRequest_Vtbl`,
//! `IUIAutomationElement_Vtbl` in `Win32/UI/Accessibility`), `IUnknown`'s three first; the same
//! count gives the `Current` getters this module used before (`get_CurrentProcessId` 20,
//! `get_CurrentControlType` 21, `get_CurrentIsPassword` 35) and `GetFocusedElement` 8, which
//! the Windows tests had confirmed against real controls.
//!
//! # One call into the application
//!
//! The focused element and its properties come in **one** request: a cache request that names
//! them ([`PROPERTIES`]), passed to `GetFocusedElementBuildCache`. UI Automation fetches the
//! element and the properties together, in one round trip to the application that provides
//! it, and the `Cached` getters then read them inside this process. So the element is chosen
//! when the answer arrives, and an application that takes long over each call takes long
//! once, not four times (the real-machine data of the late-answer work: an answer that took
//! four round trips arrived older than its freshness bound, every time).
//!
//! # Privacy
//!
//! Nothing here can read what a field holds. The methods that would (`get_CurrentName`, the
//! value and text patterns, the property getters) have no name in these tables, and the cache
//! request names six properties, none of them a name, a value or text. The getters called
//! return numbers, yes or no, a window handle, and the framework's name, which is compared
//! with the names of the system's frameworks and freed at once ([`Framework`]).

use std::ffi::c_void;
use std::mem::offset_of;
use std::ptr::{self, NonNull};
use std::time::Instant;

use windows_sys::Win32::Foundation::{SysFreeString, SysStringLen};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Accessibility::{
    CUIAutomation, UIA_ControlTypePropertyId, UIA_FrameworkIdPropertyId,
    UIA_HasKeyboardFocusPropertyId, UIA_IsPasswordPropertyId, UIA_NativeWindowHandlePropertyId,
    UIA_ProcessIdPropertyId,
};
use windows_sys::core::BSTR;
use windows_sys::core::GUID;

use super::classify::{Detail, Framework, Observation};
use super::inspect::{Probe, ProbeSource, Probed};

/// The class of the automation object (`CLSID_CUIAutomation`).
static CLASS_AUTOMATION: GUID = CUIAutomation;
/// `IID_IUIAutomation`.
static IID_AUTOMATION: GUID = GUID::from_u128(0x30cbe57d_d9d0_452a_ab13_7ac5ac4825ee);

/// The properties the question needs, fetched with the element (`UIA_*PropertyId`).
const PROPERTIES: [i32; 6] = [
    UIA_IsPasswordPropertyId,
    UIA_ControlTypePropertyId,
    UIA_ProcessIdPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_NativeWindowHandlePropertyId,
    UIA_FrameworkIdPropertyId,
];

/// The size of one slot of a method table.
const SLOT: usize = size_of::<usize>();

type Release = unsafe extern "system" fn(this: *mut c_void) -> u32;
type GetI32 = unsafe extern "system" fn(this: *mut c_void, value: *mut i32) -> i32;

/// The method table of `IUIAutomation`, up to the last method called.
#[repr(C)]
struct AutomationMethods {
    /// `QueryInterface`, `AddRef`.
    _unknown: [usize; 2],
    /// Slot 2.
    release: Release,
    /// Slots 3 to 11: `CompareElements`, `CompareRuntimeIds`, `GetRootElement`,
    /// `ElementFromHandle`, `ElementFromPoint`, `GetFocusedElement`,
    /// `GetRootElementBuildCache`, `ElementFromHandleBuildCache`, `ElementFromPointBuildCache`.
    _before_focused: [usize; 9],
    /// Slot 12.
    get_focused_element_build_cache: unsafe extern "system" fn(
        this: *mut c_void,
        cache_request: *mut c_void,
        element: *mut *mut c_void,
    ) -> i32,
    /// Slots 13 to 19: `CreateTreeWalker`, `get_ControlViewWalker`, `get_ContentViewWalker`,
    /// `get_RawViewWalker`, `get_RawViewCondition`, `get_ControlViewCondition`,
    /// `get_ContentViewCondition`.
    _before_cache_request: [usize; 7],
    /// Slot 20.
    create_cache_request:
        unsafe extern "system" fn(this: *mut c_void, cache_request: *mut *mut c_void) -> i32,
}

/// The method table of `IUIAutomationCacheRequest`, up to the last method called.
#[repr(C)]
struct CacheRequestMethods {
    /// `QueryInterface`, `AddRef`.
    _unknown: [usize; 2],
    /// Slot 2.
    release: Release,
    /// Slot 3.
    add_property: unsafe extern "system" fn(this: *mut c_void, property: i32) -> i32,
}

/// The method table of `IUIAutomationElement`, up to the last method called.
#[repr(C)]
struct ElementMethods {
    /// `QueryInterface`, `AddRef`.
    _unknown: [usize; 2],
    /// Slot 2.
    release: Release,
    /// Slots 3 to 51: `SetFocus`, `GetRuntimeId`, the four `Find` methods,
    /// `BuildUpdatedCache`, the four property-value getters, the four pattern getters,
    /// `GetCachedParent`, `GetCachedChildren` (3 to 19), and the 32 `Current` property
    /// getters from `get_CurrentProcessId` to `get_CurrentProviderDescription` (20 to 51).
    _before_cached: [usize; 49],
    /// Slot 52.
    get_cached_process_id: GetI32,
    /// Slot 53.
    get_cached_control_type: GetI32,
    /// Slots 54 to 57: `get_CachedLocalizedControlType`, `get_CachedName`,
    /// `get_CachedAcceleratorKey`, `get_CachedAccessKey`.
    _before_has_focus: [usize; 4],
    /// Slot 58.
    get_cached_has_keyboard_focus: GetI32,
    /// Slots 59 to 66: `get_CachedIsKeyboardFocusable`, `get_CachedIsEnabled`,
    /// `get_CachedAutomationId`, `get_CachedClassName`, `get_CachedHelpText`,
    /// `get_CachedCulture`, `get_CachedIsControlElement`, `get_CachedIsContentElement`.
    _before_is_password: [usize; 8],
    /// Slot 67.
    get_cached_is_password: GetI32,
    /// Slot 68.
    get_cached_native_window_handle:
        unsafe extern "system" fn(this: *mut c_void, window: *mut *mut c_void) -> i32,
    /// Slots 69 to 71: `get_CachedItemType`, `get_CachedIsOffscreen`, `get_CachedOrientation`.
    _before_framework: [usize; 3],
    /// Slot 72.
    get_cached_framework_id: unsafe extern "system" fn(this: *mut c_void, name: *mut BSTR) -> i32,
}

const _: () = {
    assert!(offset_of!(AutomationMethods, release) == 2 * SLOT);
    assert!(offset_of!(AutomationMethods, get_focused_element_build_cache) == 12 * SLOT);
    assert!(offset_of!(AutomationMethods, create_cache_request) == 20 * SLOT);
    assert!(offset_of!(CacheRequestMethods, release) == 2 * SLOT);
    assert!(offset_of!(CacheRequestMethods, add_property) == 3 * SLOT);
    assert!(offset_of!(ElementMethods, release) == 2 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_process_id) == 52 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_control_type) == 53 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_has_keyboard_focus) == 58 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_is_password) == 67 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_native_window_handle) == 68 * SLOT);
    assert!(offset_of!(ElementMethods, get_cached_framework_id) == 72 * SLOT);
};

/// The method table behind an interface pointer.
///
/// # Safety
///
/// `this` must be a live pointer to an interface whose method table starts with `T`.
unsafe fn table<'a, T>(this: NonNull<c_void>) -> &'a T {
    // SAFETY: by the caller's promise. An interface pointer points to a pointer to the
    // interface's method table, which lives as long as the object.
    unsafe { &**this.as_ptr().cast::<*const T>() }
}

/// An `IUIAutomation` this thread holds a reference to.
struct Automation(NonNull<c_void>);

impl Automation {
    fn methods(&self) -> &AutomationMethods {
        // SAFETY: `self.0` is an `IUIAutomation` pointer this value holds a reference to (it
        // comes from `CoCreateInstance` asked for that interface).
        unsafe { table(self.0) }
    }

    /// A cache request for [`PROPERTIES`], of the element alone (the default scope) and with
    /// the full element (the default mode). `None` when a call fails.
    fn cache_request(&self) -> Option<CacheRequest> {
        let mut request = ptr::null_mut();
        // SAFETY: the method is called on its own object with a valid place for the result;
        // on success it is an `IUIAutomationCacheRequest` whose reference belongs to the
        // caller.
        let result =
            unsafe { (self.methods().create_cache_request)(self.0.as_ptr(), &raw mut request) };
        if result < 0 {
            return None;
        }
        let request = CacheRequest(NonNull::new(request)?);
        for property in PROPERTIES {
            // SAFETY: the method is called on its own object with a property id.
            let added = unsafe { (request.methods().add_property)(request.0.as_ptr(), property) };
            if added < 0 {
                return None;
            }
        }
        Some(request)
    }

    /// The element with the keyboard focus, with the properties of `request`: `Ok(None)` when
    /// the system names none. One call into the application (module docs).
    fn focused_element(&self, request: &CacheRequest) -> Result<Option<Element>, ()> {
        let mut element = ptr::null_mut();
        // SAFETY: the method is called on its own object with a live cache request and a
        // valid place for the result. On success the result is null or an
        // `IUIAutomationElement` whose reference now belongs to the caller.
        let result = unsafe {
            (self.methods().get_focused_element_build_cache)(
                self.0.as_ptr(),
                request.0.as_ptr(),
                &raw mut element,
            )
        };
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

/// An `IUIAutomationCacheRequest` this thread holds a reference to.
struct CacheRequest(NonNull<c_void>);

impl CacheRequest {
    fn methods(&self) -> &CacheRequestMethods {
        // SAFETY: `self.0` is an `IUIAutomationCacheRequest` pointer this value holds a
        // reference to (it comes from `CreateCacheRequest`).
        unsafe { table(self.0) }
    }
}

impl Drop for CacheRequest {
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
        // to (it comes from `GetFocusedElementBuildCache`).
        unsafe { table(self.0) }
    }

    /// One cached 32-bit property. Reads the cache inside this process.
    fn cached(&self, getter: GetI32) -> Result<i32, ()> {
        let mut value = 0;
        // SAFETY: `getter` is one of this element's own cached getters (a `BOOL` or a 32-bit
        // number), called on its own object with a valid place for the value.
        let result = unsafe { getter(self.0.as_ptr(), &raw mut value) };
        if result < 0 { Err(()) } else { Ok(value) }
    }

    /// Whether the element is a window's own: its cached `NativeWindowHandle` is not 0.
    fn window_root(&self) -> Result<bool, ()> {
        self.native_window().map(|window| window != 0)
    }

    /// The cached `NativeWindowHandle`, as a number (0: none).
    fn native_window(&self) -> Result<u64, ()> {
        let mut window = ptr::null_mut();
        // SAFETY: the method is called on its own object with a valid place for a handle.
        let result = unsafe {
            (self.methods().get_cached_native_window_handle)(self.0.as_ptr(), &raw mut window)
        };
        if result < 0 {
            Err(())
        } else {
            Ok(window as usize as u64)
        }
    }

    /// The framework of its cached `FrameworkId`; the name is freed here and not kept.
    fn framework(&self) -> Framework {
        let mut name: BSTR = ptr::null();
        // SAFETY: the method is called on its own object with a valid place for a string; on
        // success the string (or null) belongs to the caller, who frees it below.
        let result =
            unsafe { (self.methods().get_cached_framework_id)(self.0.as_ptr(), &raw mut name) };
        if result < 0 || name.is_null() {
            return Framework::Other;
        }
        // SAFETY: a string the system made: its length is stored before it, and it is valid
        // until it is freed, which happens after the comparison.
        let framework = unsafe {
            let units = std::slice::from_raw_parts(name, SysStringLen(name) as usize);
            // Long names are none of the system's: not even looked at.
            if units.len() > 16 {
                Framework::Other
            } else {
                Framework::named(&String::from_utf16_lossy(units))
            }
        };
        // SAFETY: the string belongs to this function and is freed once.
        unsafe { SysFreeString(name) };
        framework
    }

    /// The facts of the cache request.
    fn observation(&self) -> Observation {
        let methods = self.methods();
        match (
            self.cached(methods.get_cached_is_password),
            self.cached(methods.get_cached_control_type),
            self.cached(methods.get_cached_process_id),
        ) {
            (Ok(password), Ok(control_type), Ok(process)) => Observation::Element {
                password: password != 0,
                control_type,
                process: process.cast_unsigned(),
                // The detail only ever makes an element believed (`classify`), so a getter
                // that fails gives the value that believes nothing more.
                detail: Detail {
                    has_focus: self
                        .cached(methods.get_cached_has_keyboard_focus)
                        .is_ok_and(|focus| focus != 0),
                    window_root: self.window_root().unwrap_or(true),
                    framework: self.framework(),
                },
            },
            _ => Observation::Failed,
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
    /// The cache request of every question; dropped before the automation object.
    request: Option<CacheRequest>,
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
        let mut session = Self {
            request: None,
            automation: None,
        };
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
        let automation = Automation(NonNull::new(automation)?);
        session.request = Some(automation.cache_request()?);
        session.automation = Some(automation);
        Some(session)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The objects go before the apartment they live in.
        self.request = None;
        self.automation = None;
        // SAFETY: balances the successful `CoInitializeEx` of `open`, on the same thread.
        unsafe { CoUninitialize() };
    }
}

impl Session {
    /// The focused element's two facts and its process, in one call into the application,
    /// and how long that call took.
    fn probe(&self) -> Probed {
        let started = Instant::now();
        let answered = |observation| Probed {
            observation,
            chosen: Instant::now(),
            took: started.elapsed(),
        };
        let (Some(automation), Some(request)) = (&self.automation, &self.request) else {
            return answered(Observation::Failed);
        };
        // The one call: the element and its properties are chosen together, here.
        let element = automation.focused_element(request);
        let probed = answered(Observation::Failed);
        let observation = match element {
            Ok(Some(element)) => element.observation(),
            Ok(None) => Observation::NoElement,
            Err(()) => Observation::Failed,
        };
        Probed {
            observation,
            ..probed
        }
    }
}

#[cfg(test)]
impl Session {
    /// The focused element's cached `NativeWindowHandle`, for the Windows test that pins its
    /// slot against the real handle (`os_tests`). `None` when there is none to read.
    pub(super) fn focused_native_window(&self) -> Option<u64> {
        let (automation, request) = (self.automation.as_ref()?, self.request.as_ref()?);
        automation
            .focused_element(request)
            .ok()
            .flatten()?
            .native_window()
            .ok()
    }
}

impl Probe for Session {
    fn observe(&mut self) -> Probed {
        self.probe()
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
