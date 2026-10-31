//! Software updates through Sparkle, which the release bundle embeds in
//! `Contents/Frameworks`. Sparkle checks for updates once a day, after asking
//! the user on the second launch, and does everything else itself: it shows
//! the release notes, downloads and verifies the update, replaces the app and
//! relaunches it.
//!
//! Builds without Sparkle, such as `cargo run`, have no updater.

use gpui_kit::App;

gpui_kit::actions!(neon, [CheckForUpdates]);

/// Start Sparkle if the app bundle has it, and handle `CheckForUpdates`.
/// Returns whether it started, so the menu shows Check for Updates only when
/// it can.
pub fn init(cx: &mut App) -> bool {
    #[cfg(target_os = "macos")]
    return sparkle::init(cx);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cx;
        false
    }
}

#[cfg(target_os = "macos")]
mod sparkle {
    use gpui_kit::{App, Global};
    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{NSBundle, NSString};

    /// Sparkle's `SPUStandardUpdaterController`, which runs the updater for as
    /// long as it lives.
    struct Updater(Retained<AnyObject>);

    impl Global for Updater {}

    pub(super) fn init(cx: &mut App) -> bool {
        let Some(controller) = start_sparkle() else {
            return false;
        };
        cx.set_global(Updater(controller));
        cx.on_action(|_: &super::CheckForUpdates, cx| {
            let controller = &cx.global::<Updater>().0;
            let sender: Option<&AnyObject> = None;
            // Shows Sparkle's window, or brings it forward if a check is running.
            unsafe {
                let _: () = msg_send![controller, checkForUpdates: sender];
            }
        });
        true
    }

    fn start_sparkle() -> Option<Retained<AnyObject>> {
        let frameworks = NSBundle::mainBundle().privateFrameworksPath()?;
        let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
        let bundle = NSBundle::bundleWithPath(&path)?;
        // SAFETY: loading runs Sparkle's initializers, from the copy that
        // script/bundle-mac embedded and signed with the app.
        if !unsafe { bundle.load() } {
            return None;
        }
        let class = AnyClass::get(c"SPUStandardUpdaterController")?;
        let no_delegate: Option<&AnyObject> = None;
        // SAFETY: matches `-[SPUStandardUpdaterController
        // initWithStartingUpdater:updaterDelegate:userDriverDelegate:]`, called
        // on the main thread as Sparkle requires: GPUI runs `App` code there.
        unsafe {
            let controller: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![
                controller,
                initWithStartingUpdater: Bool::YES,
                updaterDelegate: no_delegate,
                userDriverDelegate: no_delegate
            ]
        }
    }
}
