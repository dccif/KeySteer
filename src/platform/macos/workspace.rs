//! Cached workspace and appearance state.

use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSEventMask, NSWorkspace};
use objc2_core_foundation::CFRunLoop;
use objc2_foundation::{NSComparisonResult, NSDate, NSRunLoop, NSUserDefaults, ns_string};

use crate::api::backend::{Appearance, BackendEvent};
use crate::api::command::FocusedApp;

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const MAX_APP_EVENTS_PER_POLL: usize = 64;

/// Wake AppKit's main run loop after a backend producer queues an event.
/// A Rust channel wake alone does not commit pending NSWindow/NSView updates.
pub fn wake_main_run_loop() {
    if let Some(run_loop) = CFRunLoop::main() {
        run_loop.wake_up();
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
        NSRunLoop::mainRunLoop().runMode_beforeDate(
            super::native::default_run_loop_modes().foundation,
            &deadline,
        );
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
        let focused_pid = frontmost_process_id();
        if focused_pid != self.focused.as_ref().map(|app| app.process_id) {
            let focused = focused_app();
            if focused != self.focused {
                self.focused = focused.clone();
                events.push(BackendEvent::FocusChanged(focused));
            }
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
                super::native::default_run_loop_modes().foundation,
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
        if defaults
            .stringForKey(ns_string!("AppleInterfaceStyle"))
            .is_some_and(|style| {
                style.caseInsensitiveCompare(ns_string!("Dark")) == NSComparisonResult::Same
            })
        {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    })
}

fn frontmost_process_id() -> Option<u32> {
    autoreleasepool(|_| {
        let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        u32::try_from(application.processIdentifier()).ok()
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
