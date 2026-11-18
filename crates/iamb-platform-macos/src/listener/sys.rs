//! The system functions this crate declares by hand, all in one place.
//!
//! The `objc2` family has no usable Carbon crate, and its Core Foundation crate is built here
//! without the notification-centre header. What the listener, the hotkeys and their tests need
//! from those is declared below, each with the signature of the system header named in its
//! comment. Nothing here is checked by the compiler against the real headers, and the Linux
//! build cannot link, so a mistake in this file shows only on macOS: keep it short, and change
//! it with the header open. (The names, signatures and constants below were compared with the
//! headers and export lists of the macOS 11.3 SDK when this file was written.)
//!
//! | Functions | Header | Used by |
//! | --- | --- | --- |
//! | `TIS…`, `kTIS…` | `HIToolbox/TextInputSources.h` | `layout.rs`: the current keyboard layout and input source |
//! | `UCKeyTranslate` | `CarbonCore/UnicodeUtilities.h` | `layout.rs`: key press to text |
//! | `LMGetKbdType` | `LowMem.h` (exported by `HIToolbox`) | `layout.rs`: the keyboard type when an event names none |
//! | `RegisterEventHotKey` and the event-handler calls | `HIToolbox/CarbonEvents.h` | `hotkeys/mac.rs` |
//! | `IsSecureEventInputEnabled` | `HIToolbox/CarbonEventsCore.h` | `tap.rs`: a self-test that fails during secure keyboard entry is not a failure |
//! | `CFNotificationCenter…` | `CoreFoundation/CFNotificationCenter.h` | `layout.rs`: "the input source changed" |
//!
//! # Threads
//!
//! The headers mark every Carbon function here "not thread safe", and the Text Input Sources
//! header adds that a UI application must call its functions on the main
//! thread. Recent systems are reported to enforce that by stopping a process that calls them
//! from another thread. So every function of the first, third, fourth and fifth row is called
//! on the main thread only; the callers enforce it by taking a `MainThreadMarker`.
//! `UCKeyTranslate` is a pure function of its arguments and is called on the listener thread.
//! The notification-centre functions are thread safe.

#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

#[cfg(test)]
use objc2_core_foundation::{CFArray, CFDictionary};
use objc2_core_foundation::{CFString, CFType};

/// `OSStatus`: 0 is success.
pub(crate) type OSStatus = i32;

/// `noErr`.
pub(crate) const NO_ERR: OSStatus = 0;
/// `eventNotHandledErr`: what a Carbon event handler returns for an event it leaves to others.
pub(crate) const EVENT_NOT_HANDLED_ERR: OSStatus = -9874;
/// `eventHotKeyExistsErr`: the chord is registered already.
pub(crate) const EVENT_HOT_KEY_EXISTS_ERR: OSStatus = -9878;

/// `kEventClassKeyboard` (`'keyb'`).
pub(crate) const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
/// `kEventHotKeyPressed`.
pub(crate) const EVENT_HOT_KEY_PRESSED: u32 = 5;
/// `kEventParamDirectObject` (`'----'`).
pub(crate) const EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
/// `typeEventHotKeyID` (`'hkid'`).
pub(crate) const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
/// `kEventHotKeyExclusive`: registration fails when anyone holds the chord already.
pub(crate) const EVENT_HOT_KEY_EXCLUSIVE: u32 = 1;

/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
pub(crate) const NOTIFICATION_DELIVER_IMMEDIATELY: isize = 4;

/// `EventHotKeyID`: the application's name for a hotkey, passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct EventHotKeyID {
    /// A four-character code naming the application.
    pub(crate) signature: u32,
    /// The application's number for the hotkey.
    pub(crate) id: u32,
}

/// `EventTypeSpec`: one kind of Carbon event.
#[repr(C)]
pub(crate) struct EventTypeSpec {
    /// The event class.
    pub(crate) eventClass: u32,
    /// The kind within the class.
    pub(crate) eventKind: u32,
}

/// `EventTargetRef`.
pub(crate) type EventTargetRef = *mut c_void;
/// `EventHandlerRef`.
pub(crate) type EventHandlerRef = *mut c_void;
/// `EventHandlerCallRef`.
pub(crate) type EventHandlerCallRef = *mut c_void;
/// `EventRef`.
pub(crate) type EventRef = *mut c_void;
/// `EventHotKeyRef`.
pub(crate) type EventHotKeyRef = *mut c_void;

/// `EventHandlerUPP`: a Carbon event handler.
pub(crate) type EventHandlerUPP =
    unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

/// `CFNotificationCallback`.
pub(crate) type CFNotificationCallback = unsafe extern "C" fn(
    center: *mut c_void,
    observer: *mut c_void,
    name: *const c_void,
    object: *const c_void,
    user_info: *const c_void,
);

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// `TISInputSourceRef TISCopyCurrentKeyboardLayoutInputSource(void)`: the keyboard layout
    /// in use (for an input method, the layout it types through). Returns a retained object,
    /// or null. Main thread only.
    pub(crate) fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut CFType;

    /// `TISInputSourceRef TISCopyCurrentKeyboardInputSource(void)`: the selected input source,
    /// a keyboard layout or an input method. Returns a retained object, or null. Main thread
    /// only.
    pub(crate) fn TISCopyCurrentKeyboardInputSource() -> *mut CFType;

    /// `void *TISGetInputSourceProperty(TISInputSourceRef, CFStringRef)`: a property of an
    /// input source, not retained, or null. Main thread only.
    pub(crate) fn TISGetInputSourceProperty(
        inputSource: &CFType,
        propertyKey: &CFString,
    ) -> *const c_void;

    /// `CFArrayRef TISCreateInputSourceList(CFDictionaryRef, Boolean)`: the input sources that
    /// match the given properties. Returns a retained array, or null when none matches. Main
    /// thread only.
    #[cfg(test)]
    pub(crate) fn TISCreateInputSourceList(
        properties: Option<&CFDictionary>,
        includeAllInstalled: u8,
    ) -> *mut CFArray;

    /// The property key of an input source's `uchr` keyboard-layout data (a `CFData`).
    pub(crate) static kTISPropertyUnicodeKeyLayoutData: &'static CFString;
    /// The property key of an input source's identifier (a `CFString`), such as
    /// `com.apple.keylayout.US`.
    pub(crate) static kTISPropertyInputSourceID: &'static CFString;
    /// The property key of an input source's type (a `CFString`).
    pub(crate) static kTISPropertyInputSourceType: &'static CFString;
    /// The type of an input source that is a plain keyboard layout.
    pub(crate) static kTISTypeKeyboardLayout: &'static CFString;
    /// The name of the distributed notification sent when the selected input source changes.
    pub(crate) static kTISNotifySelectedKeyboardInputSourceChanged: &'static CFString;

    /// `UInt8 LMGetKbdType(void)`: the type of the keyboard last used.
    pub(crate) fn LMGetKbdType() -> u8;

    /// `OSStatus UCKeyTranslate(const UCKeyboardLayout *, UInt16, UInt16, UInt32, UInt32,
    /// OptionBits, UInt32 *, UniCharCount, UniCharCount *, UniChar[])`: what a key press types
    /// on a layout. A pure function of its arguments.
    pub(crate) fn UCKeyTranslate(
        keyLayoutPtr: *const c_void,
        virtualKeyCode: u16,
        keyAction: u16,
        modifierKeyState: u32,
        keyboardType: u32,
        keyTranslateOptions: u32,
        deadKeyState: *mut u32,
        maxStringLength: usize,
        actualStringLength: *mut usize,
        unicodeString: *mut u16,
    ) -> OSStatus;

    /// `Boolean IsSecureEventInputEnabled(void)`: whether any process has secure keyboard
    /// entry on (a password field has the focus). Main thread only.
    pub(crate) fn IsSecureEventInputEnabled() -> u8;

    /// `EventTargetRef GetApplicationEventTarget(void)`: where the application's Carbon events
    /// are dispatched. Main thread only.
    pub(crate) fn GetApplicationEventTarget() -> EventTargetRef;

    /// `OSStatus InstallEventHandler(EventTargetRef, EventHandlerUPP, ItemCount, const
    /// EventTypeSpec *, void *, EventHandlerRef *)`. Main thread only.
    pub(crate) fn InstallEventHandler(
        inTarget: EventTargetRef,
        inHandler: EventHandlerUPP,
        inNumTypes: usize,
        inList: *const EventTypeSpec,
        inUserData: *mut c_void,
        outRef: *mut EventHandlerRef,
    ) -> OSStatus;

    /// `OSStatus RegisterEventHotKey(UInt32, UInt32, EventHotKeyID, EventTargetRef, OptionBits,
    /// EventHotKeyRef *)`. Main thread only.
    pub(crate) fn RegisterEventHotKey(
        inHotKeyCode: u32,
        inHotKeyModifiers: u32,
        inHotKeyID: EventHotKeyID,
        inTarget: EventTargetRef,
        inOptions: u32,
        outRef: *mut EventHotKeyRef,
    ) -> OSStatus;

    /// `OSStatus UnregisterEventHotKey(EventHotKeyRef)`. Main thread only.
    pub(crate) fn UnregisterEventHotKey(inHotKey: EventHotKeyRef) -> OSStatus;

    /// `OSStatus GetEventParameter(EventRef, EventParamName, EventParamType, EventParamType *,
    /// ByteCount, ByteCount *, void *)`.
    pub(crate) fn GetEventParameter(
        inEvent: EventRef,
        inName: u32,
        inDesiredType: u32,
        outActualType: *mut u32,
        inBufferSize: usize,
        outActualSize: *mut usize,
        outData: *mut c_void,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// `CFNotificationCenterRef CFNotificationCenterGetDistributedCenter(void)`: the centre
    /// for notifications between processes. Not retained; lives as long as the process.
    pub(crate) fn CFNotificationCenterGetDistributedCenter() -> *mut c_void;

    /// `void CFNotificationCenterAddObserver(CFNotificationCenterRef, const void *,
    /// CFNotificationCallback, CFStringRef, const void *, CFNotificationSuspensionBehavior)`.
    /// A distributed notification is delivered on the main thread's run loop.
    pub(crate) fn CFNotificationCenterAddObserver(
        center: *mut c_void,
        observer: *const c_void,
        callBack: CFNotificationCallback,
        name: &CFString,
        object: *const c_void,
        suspensionBehavior: isize,
    );
}
