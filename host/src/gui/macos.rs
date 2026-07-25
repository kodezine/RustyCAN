//! macOS-specific startup workarounds.
//!
//! # Touch Bar `NSRangeException` crash
//!
//! On macOS (notably macOS 26 and MacBooks with a Touch Bar / emulated function
//! row) RustyCAN could terminate at launch or exit with:
//!
//! ```text
//! *** Terminating app due to uncaught exception 'NSRangeException',
//!     reason: 'Cannot remove an observer <_NSTouchBarFinderObservation …> for
//!     the key path "nextResponder" from <WinitView …> because it is not
//!     registered as an observer.'
//! ```
//!
//! ## Why it happens
//!
//! AppKit lazily creates a process-wide `_NSTouchBarFinder` the first time the
//! Touch Bar API is engaged. That finder installs KVO observers on the
//! `nextResponder` key path of **every** `NSResponder` in the chain so it can
//! rebuild the Touch Bar when the first responder changes. `winit`'s
//! `WinitView` mutates its responder chain during setup/teardown; under rapid
//! changes the finder queues several observation-invalidation blocks and the
//! second `removeObserver:forKeyPath:` targets an observation that was already
//! removed, which throws `NSRangeException` from the underlying
//! `NSKeyValueObservationInfo` array. RustyCAN never uses the Touch Bar
//! (`WinitView`'s `touchBar` already returns `nil`), so the observation exists
//! only to service a feature we do not want.
//!
//! Disabling the Touch Bar API process-wide (`NSFunctionBarAPIEnabled = NO`)
//! does **not** help on macOS 26 — the finder still engages and still crashes.
//!
//! ## The fix
//!
//! Add unconditional no-op overrides for the three KVO registration selectors
//! directly onto the `WinitView` class at runtime, so the finder's
//! `addObserver:`/`removeObserver:` calls become no-ops and no observation is
//! ever registered or invalidated on `WinitView`. RustyCAN never uses the
//! Touch Bar (`WinitView.touchBar` already returns `nil`) and nothing
//! legitimately KVO-observes `WinitView`, so a full no-op is safe.
//!
//! This must run **after** the `WinitView` class has been registered (i.e. once
//! the first window/view exists, from inside the `eframe` creation callback)
//! but before AppKit's first display-cycle flush spins the run loop.

use core::ffi::{c_char, c_void, CStr};

use objc2::ffi::{class_addMethod, objc_getClass};
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;

// No-op IMPs. The Objective-C runtime calls these as
// `fn(self, _cmd, <method args>)`; `extern "C-unwind"` matches `Imp` and the
// ABI AppKit expects. They intentionally do nothing.

extern "C-unwind" fn noop_add_observer(
    _this: *mut AnyObject,
    _cmd: Sel,
    _observer: *mut AnyObject,
    _key_path: *mut AnyObject,
    _options: usize,
    _context: *mut c_void,
) {
}

extern "C-unwind" fn noop_remove_observer_context(
    _this: *mut AnyObject,
    _cmd: Sel,
    _observer: *mut AnyObject,
    _key_path: *mut AnyObject,
    _context: *mut c_void,
) {
}

extern "C-unwind" fn noop_remove_observer(
    _this: *mut AnyObject,
    _cmd: Sel,
    _observer: *mut AnyObject,
    _key_path: *mut AnyObject,
) {
}

/// Install no-op KVO overrides on the `WinitView` class to prevent the
/// `_NSTouchBarFinder` `NSRangeException` crash.
///
/// If the `WinitView` class is not (yet) registered the crash mitigation cannot
/// be installed: this panics in debug builds (via `debug_assert!`) so the
/// regression is caught during development, and is a no-op in release builds.
/// Safe to call more than once: `class_addMethod` leaves an already-installed
/// override untouched.
pub(super) fn suppress_touch_bar_kvo() {
    // SAFETY: we look up the `WinitView` class by name and, if present, attach
    // no-op method implementations whose signatures match the standard KVO
    // registration selectors. All arguments are opaque pointers/integers that
    // the no-ops never dereference.
    unsafe {
        let cls = objc_getClass(c"WinitView".as_ptr()) as *mut AnyClass;
        if cls.is_null() {
            // If `WinitView` is renamed or not yet registered when this runs,
            // the crash mitigation is silently skipped. Fail loudly in debug
            // builds so such a regression is caught during development, while
            // leaving release behavior unchanged.
            debug_assert!(
                !cls.is_null(),
                "WinitView class not found; Touch Bar KVO crash mitigation not installed"
            );
            return;
        }

        add_noop(
            cls,
            sel!(addObserver:forKeyPath:options:context:),
            core::mem::transmute::<
                extern "C-unwind" fn(
                    *mut AnyObject,
                    Sel,
                    *mut AnyObject,
                    *mut AnyObject,
                    usize,
                    *mut c_void,
                ),
                Imp,
            >(noop_add_observer),
            c"v@:@@Q^v",
        );
        add_noop(
            cls,
            sel!(removeObserver:forKeyPath:context:),
            core::mem::transmute::<
                extern "C-unwind" fn(
                    *mut AnyObject,
                    Sel,
                    *mut AnyObject,
                    *mut AnyObject,
                    *mut c_void,
                ),
                Imp,
            >(noop_remove_observer_context),
            c"v@:@@^v",
        );
        add_noop(
            cls,
            sel!(removeObserver:forKeyPath:),
            core::mem::transmute::<
                extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
                Imp,
            >(noop_remove_observer),
            c"v@:@@",
        );
    }
}

/// SAFETY: `cls` must be a valid class pointer, `imp` must match the ABI implied
/// by `types`, and `types` must be a valid Objective-C method type encoding.
unsafe fn add_noop(cls: *mut AnyClass, sel: Sel, imp: Imp, types: &CStr) {
    // `class_addMethod` adds an override that shadows the inherited NSObject
    // implementation. It returns false (leaving any existing method) if the
    // selector is already defined directly on the class — harmless here.
    let _: objc2::runtime::Bool = class_addMethod(cls, sel, imp, types.as_ptr() as *const c_char);
}
