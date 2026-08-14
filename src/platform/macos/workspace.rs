//! Cached workspace and appearance state.

use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSEventMask, NSWorkspace};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString, NSUserDefaults};

use crate::api::backend::{Appearance, BackendEvent};
use crate::api::command::FocusedApp;

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const MAX_APP_EVENTS_PER_POLL: usize = 64;

unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopWakeUp(run_loop: *mut std::ffi::c_void);
}

/// Wake AppKit's main run loop after a backend producer queues an event.
/// A Rust channel wake alone does not commit pending NSWindow/NSView updates.
pub fn wake_main_run_loop() {
    // SAFETY: Core Foundation returns a borrowed process-lifetime main run loop;
    // the null check precedes the wake call and no ownership is transferred.
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
        // SAFETY: NSDefaultRunLoopMode is a process-lifetime Foundation
        // constant used for this synchronous main-thread pump.
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
        // A System Settings/TCC transition can enqueue an event burst. Keep a
        // fixed budget so AppKit cannot indefinitely starve the engine's Quit
        // or capture-loss event; remaining events stay queued for next poll.
        for _ in 0..MAX_APP_EVENTS_PER_POLL {
            let Some(event) = application.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&expiration),
                // SAFETY: NSDefaultRunLoopMode is a process-lifetime Foundation
                // constant used only for this synchronous main-thread call.
                unsafe { NSDefaultRunLoopMode },
                true,
            ) else {
                break;
            };
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

    #[test]
    fn app_event_pump_has_a_finite_budget() {
        assert!((1..=256).contains(&MAX_APP_EVENTS_PER_POLL));
    }
}
