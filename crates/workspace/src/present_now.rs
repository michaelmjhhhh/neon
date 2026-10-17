//! Present a window's frame right away instead of at the next display refresh.
//!
//! GPUI draws when the display link fires, once per refresh. A keystroke
//! therefore waits for the next tick, is drawn, and the frame reaches the
//! screen one refresh after that tick. macOS also asks a view to draw outside
//! the display link through `displayLayer:`, and GPUI answers that call by
//! drawing and presenting synchronously (it uses this when a window becomes
//! key). Calling it after a keystroke submits the frame as soon as it is
//! drawn, so it can make the very next refresh: about one refresh sooner.

use gpui_kit::Window;

/// The window's native view, to pass to [`present`] once no entity is being
/// updated.
#[derive(Clone, Copy)]
pub(crate) struct NativeView(#[allow(dead_code)] *mut std::ffi::c_void);

impl NativeView {
    pub(crate) fn of(window: &Window) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match HasWindowHandle::window_handle(window).ok()?.as_raw() {
                RawWindowHandle::AppKit(handle) => Some(Self(handle.ns_view.as_ptr())),
                _ => None,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window;
            None
        }
    }
}

/// Draw any pending changes and present them now.
///
/// GPUI's frame callback updates the app, so this must not run inside an
/// entity or window update: call it from a task, after the update returns.
/// `view` must belong to a window that is still open.
pub(crate) fn present(view: NativeView) {
    #[cfg(target_os = "macos")]
    // SAFETY: `view` is the GPUI view of an open window, which implements
    // `displayLayer:` and ignores the layer argument.
    unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        let view = view.0.cast::<AnyObject>();
        let _: () = msg_send![view, displayLayer: std::ptr::null_mut::<AnyObject>()];
    }
    #[cfg(not(target_os = "macos"))]
    let _ = view;
}
