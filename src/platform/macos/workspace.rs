//! Cached workspace and appearance state.

use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSEventMask, NSWorkspace};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString, NSUserDefaults};

use crate::api::backend::{Appearance, BackendEvent};
use crate::api::command::FocusedApp;

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopWakeUp(run_loop: *mut std::ffi::c_void);
}

/// Wake AppKit's main run loop after a backend producer queues an event.
/// A Rust channel wake alone does not commit pending NSWindow/NSView updates.
pub fn wake_main_run_loop() {
    unsafe {
        let run_loop = CFRunLoopGetMain();
        if !run_loop.is_null() {
            CFRunLoopWakeUp(run_loop);
        }
    }
}

/// Let AppKit process native sources until an event producer wakes the loop or
/// the engine deadline expires. This is a blocking bound, not a periodic timer.
pub fn wait_for_app_event(timeout: Duration) {
    if timeout.is_zero() || MainThreadMarker::new().is_none() {
        return;
    }
    autoreleasepool(|_| {
        let deadline = NSDate::dateWithTimeIntervalSinceNow(timeout.as_secs_f64());
        NSRunLoop::mainRunLoop().runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &deadline);
    });
}

pub struct Workspace {
    focused: Option<FocusedApp>,
    appearance: Appearance,
    next_refresh: Instant,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            focused: focused_app(),
            appearance: appearance(),
            next_refresh: Instant::now(),
        }
    }

    pub fn focused_app(&self) -> Option<FocusedApp> {
        self.focused.clone()
    }

    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    pub fn refresh(&mut self) -> Vec<BackendEvent> {
        pump_app_events();
        if Instant::now() < self.next_refresh {
            return Vec::new();
        }
        self.next_refresh = Instant::now() + REFRESH_INTERVAL;

        let mut events = Vec::new();
        let focused = focused_app();
        if focused != self.focused {
            self.focused = focused.clone();
            events.push(BackendEvent::FocusChanged(focused));
        }
        let appearance = appearance();
        if appearance != self.appearance {
            self.appearance = appearance;
            events.push(BackendEvent::AppearanceChanged(appearance));
        }
        events
    }
}

pub fn pump_app_events() {
    autoreleasepool(|_| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let application = NSApplication::sharedApplication(mtm);
        let expiration = NSDate::distantPast();
        while let Some(event) = application.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&expiration),
            unsafe { NSDefaultRunLoopMode },
            true,
        ) {
            application.sendEvent(&event);
        }
    });
}

fn focused_app() -> Option<FocusedApp> {
    autoreleasepool(|_| {
        let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let bundle_id = application.bundleIdentifier()?.to_string();
        let process_id = u32::try_from(application.processIdentifier()).ok()?;
        Some(FocusedApp {
            bundle_id,
            window_title: String::new(),
            process_id,
        })
    })
}

fn appearance() -> Appearance {
    autoreleasepool(|_| {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str("AppleInterfaceStyle");
        if defaults
            .stringForKey(&key)
            .is_some_and(|style| style.to_string().eq_ignore_ascii_case("dark"))
        {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_interval_is_short_but_not_a_hot_loop() {
        assert!(REFRESH_INTERVAL >= Duration::from_millis(100));
        assert!(REFRESH_INTERVAL <= Duration::from_secs(1));
    }
}
