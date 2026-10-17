//! UIScene lifecycle adoption for iOS 27.
//!
//! iOS 27 terminates apps that never adopted the scene lifecycle
//! (`_UIApplicationEvaluateRuntimeIssueForNoSceneLifecycleAdoption`). winit 0.30 still launches
//! through the app delegate and creates its `UIWindow` without a scene, so Info.plist declares
//! `CogitatorSceneDelegate`, and whichever side arrives second (scene connect or Bevy window
//! creation) attaches winit's window to the scene. Everything here runs on the main thread.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, RawHandleWrapper};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, ClassType};
use raw_window_handle::RawWindowHandle;

static SCENE: AtomicUsize = AtomicUsize::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);
static ATTACHED: AtomicBool = AtomicBool::new(false);

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "CogitatorSceneDelegate"]
    struct SceneDelegate;

    impl SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(&self, scene: &AnyObject, _session: &AnyObject, _options: &AnyObject) {
            // UIKit keeps the scene alive for the session; a plain pointer is enough here.
            SCENE.store(scene as *const AnyObject as usize, Ordering::Release);
            attach();
        }
    }
);

/// Register the delegate class before `UIApplicationMain`, which looks it up by name.
pub fn register() {
    let _ = SceneDelegate::class();
}

fn attach() {
    let (scene, window) = (SCENE.load(Ordering::Acquire), WINDOW.load(Ordering::Acquire));
    if scene == 0 || window == 0 || ATTACHED.swap(true, Ordering::AcqRel) {
        return;
    }
    let (scene, window) = (scene as *mut AnyObject, window as *mut AnyObject);
    unsafe {
        let _: () = msg_send![window, setWindowScene: scene];
        let _: () = msg_send![window, makeKeyAndVisible];
    }
}

/// Bevy side: once the primary window exists, hand its `UIWindow` over and try to attach.
pub fn attach_primary_window(
    windows: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    _main_thread: NonSendMarker,
) {
    if ATTACHED.load(Ordering::Acquire) || WINDOW.load(Ordering::Acquire) != 0 {
        return;
    }
    let Ok(handle) = windows.single() else { return };
    let RawWindowHandle::UiKit(ui) = handle.get_window_handle() else { return };
    let view = ui.ui_view.as_ptr() as *mut AnyObject;
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    if window.is_null() {
        return;
    }
    WINDOW.store(window as usize, Ordering::Release);
    attach();
}
