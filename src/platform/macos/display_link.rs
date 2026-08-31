//! AppKit display-synchronised frame delivery for continuous pointer movement.
//!
//! macOS 14's `NSView.displayLinkWithTarget:selector:` tracks the display that
//! contains the view. Moving the cursor layer across screens therefore changes
//! cadence automatically without querying display IDs or refresh rates. The
//! callback uses display timestamps and accumulates elapsed time, so a delayed
//! main-loop turn or a screen transition cannot silently discard travel time.

use std::cell::Cell;
use std::time::Duration;

use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_foundation::{NSDate, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;

struct FrameTargetIvars {
    last_timestamp: Cell<Option<f64>>,
    pending_elapsed: Cell<Duration>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KeySteerDisplayLinkTarget"]
    #[ivars = FrameTargetIvars]
    struct FrameTarget;

    impl FrameTarget {
        #[unsafe(method(displayFrame:))]
        fn display_frame(&self, link: &CADisplayLink) {
            let timestamp = link.timestamp();
            // A timestamp delta spans any callbacks AppKit coalesced while the
            // main thread was busy. Accumulating it until the engine consumes a
            // frame preserves wall-clock movement without a catch-up queue.
            let elapsed = self
                .ivars()
                .last_timestamp
                .get()
                .filter(|last| timestamp > *last)
                .map_or_else(|| link.duration(), |last| timestamp - last);
            self.ivars().last_timestamp.set(Some(timestamp));

            if elapsed.is_finite() && elapsed > 0.0 {
                self.ivars().pending_elapsed.set(
                    self.ivars()
                        .pending_elapsed
                        .get()
                        .saturating_add(Duration::from_secs_f64(elapsed)),
                );
            }
        }
    }
);

impl FrameTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(FrameTargetIvars {
            last_timestamp: Cell::new(None),
            pending_elapsed: Cell::new(Duration::ZERO),
        });
        // SAFETY: The NSObject superclass is initialized after Rust ivars.
        unsafe { msg_send![super(this), init] }
    }

    fn reset(&self) {
        self.ivars().last_timestamp.set(None);
        self.ivars().pending_elapsed.set(Duration::ZERO);
    }

    fn take_elapsed(&self) -> Option<Duration> {
        let elapsed = self.ivars().pending_elapsed.replace(Duration::ZERO);
        (!elapsed.is_zero()).then_some(elapsed)
    }
}

pub struct DisplayFrameClock {
    target: Retained<FrameTarget>,
    link: Option<Retained<CADisplayLink>>,
}

impl DisplayFrameClock {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            target: FrameTarget::new(mtm),
            link: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.link.is_some()
    }

    pub fn start(&mut self, source: &NSView) {
        if self.is_running() {
            return;
        }
        self.target.reset();
        // SAFETY: `displayFrame:` has the required `(CADisplayLink *)`
        // callback signature, and both target and source stay retained.
        let link =
            unsafe { source.displayLinkWithTarget_selector(&self.target, sel!(displayFrame:)) };
        // Native cadence is the default. Registering in common modes keeps
        // motion synchronised during AppKit event/menu tracking as well.
        // SAFETY: `link`, the main run loop and the process-lifetime common
        // mode token remain live for the synchronous registration.
        unsafe {
            link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }
        self.link = Some(link);
    }

    pub fn stop(&mut self) {
        if let Some(link) = self.link.take() {
            link.invalidate();
            self.target.reset();
        }
    }

    /// Run the main run loop until AppKit delivers a display frame or the
    /// engine's own deadline expires. The deadline is a blocking bound, not a
    /// periodic movement timer.
    pub fn next(&self, timeout: Duration) -> Option<Duration> {
        autoreleasepool(|_| self.next_inner(timeout))
    }

    fn next_inner(&self, timeout: Duration) -> Option<Duration> {
        if let Some(elapsed) = self.target.take_elapsed() {
            return Some(elapsed);
        }

        let deadline = NSDate::dateWithTimeIntervalSinceNow(timeout.as_secs_f64());
        let run_loop = NSRunLoop::mainRunLoop();
        let mode = super::native::default_run_loop_modes().foundation;
        let _handled = run_loop.runMode_beforeDate(mode, &deadline);
        // Any non-frame AppKit wake returns control to Backend::poll so a
        // synchronously waiting hook is checked before another VBlank wait.
        self.target.take_elapsed()
    }
}

impl Drop for DisplayFrameClock {
    fn drop(&mut self) {
        if let Some(link) = self.link.take() {
            link.invalidate();
        }
    }
}
