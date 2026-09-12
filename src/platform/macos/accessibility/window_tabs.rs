//! Worker-owned AX observer sources; callbacks only enqueue opaque identities.
use super::*;
use crate::api::window::WindowId;
use crate::api::window_tabs::TabNativeEvent;
use core_foundation::base::{CFEqual, CFRetain};
use core_foundation::runloop::{
    CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRemoveSource, CFRunLoopRunInMode,
    CFRunLoopSourceRef,
};
use std::collections::BTreeMap;

type ObserverRef = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXObserverCreate(
        pid: i32,
        callback: unsafe extern "C" fn(ObserverRef, AXUIElementRef, CFStringRef, *mut c_void),
        observer: *mut ObserverRef,
    ) -> c_int;
    fn AXObserverAddNotification(
        observer: ObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        context: *mut c_void,
    ) -> c_int;
    fn AXObserverGetRunLoopSource(observer: ObserverRef) -> CFRunLoopSourceRef;
}
struct Context {
    windows: Vec<(WindowId, OwnedCf)>,
}
struct Observer {
    observer: OwnedCf,
    context: Box<Context>,
    ids: Vec<WindowId>,
}
impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: remove the source on its installing worker before releasing callback context.
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetCurrent(),
                AXObserverGetRunLoopSource(self.observer.as_ptr()),
                crate::platform::macos::native::default_run_loop_modes().core_foundation,
            );
        }
    }
}
unsafe extern "C" fn callback(
    _observer: ObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    raw: *mut c_void,
) {
    if raw.is_null() {
        return;
    }
    // SAFETY: the Box outlives its registered run-loop source and AX supplies live CF references.
    let (context, name) = unsafe {
        (
            &*(raw as *const Context),
            CFString::wrap_under_get_rule(notification).to_string(),
        )
    };
    if name == "AXFocusedWindowChanged" {
        // Resolve the application's focused window once in the worker, not
        // once per member and never from this native callback.
        crate::platform::macos::window_tabs::enqueue(TabNativeEvent::VisibilityChanged);
        return;
    }
    let id = context
        .windows
        .iter()
        .find(|(_, window)| {
            // SAFETY: both retained CF objects are live during this callback.
            unsafe { CFEqual(window.as_ptr(), element) != 0 }
        })
        .map(|(id, _)| *id);
    if let Some(id) = id {
        crate::platform::macos::window_tabs::enqueue(if name == "AXUIElementDestroyed" {
            TabNativeEvent::Closed(id)
        } else if name == "AXMoved" || name == "AXResized" {
            TabNativeEvent::GeometryChanged(id)
        } else {
            TabNativeEvent::Changed(id)
        });
    }
}
#[derive(Default)]
pub(super) struct Monitor {
    observers: BTreeMap<i32, Observer>,
}
impl Monitor {
    pub fn watch(&mut self, windows: &[(WindowId, i32, AXUIElementRef)]) -> Result<(), String> {
        self.observers
            .retain(|pid, _| windows.iter().any(|(_, process, _)| pid == process));
        for pid in windows
            .iter()
            .map(|(_, pid, _)| *pid)
            .collect::<std::collections::BTreeSet<_>>()
        {
            let selected: Vec<_> = windows
                .iter()
                .filter(|(_, process, _)| *process == pid)
                .collect();
            let ids: Vec<_> = selected.iter().map(|(id, _, _)| *id).collect();
            if self
                .observers
                .get(&pid)
                .is_some_and(|owner| owner.ids == ids)
            {
                continue;
            }
            let mut raw = ptr::null();
            // SAFETY: exact AX callback ABI and initialized output pointer.
            let error = unsafe { AXObserverCreate(pid, callback, &mut raw) };
            if error != AX_OK {
                return Err(format!("Cannot monitor application: AXError {error}"));
            }
            // SAFETY: successful AXObserverCreate returns an owned CF object.
            let observer =
                unsafe { OwnedCf::from_create_rule(raw) }.ok_or("AX observer is unavailable")?;
            let mut context = Box::new(Context {
                windows: Vec::new(),
            });
            for (id, _, window) in selected {
                // SAFETY: caller retains each AX element; this source takes its own reference.
                let retained = unsafe { OwnedCf::from_create_rule(CFRetain(*window)) }
                    .ok_or("Window expired")?;
                context.windows.push((*id, retained));
            }
            let mut owner = Observer {
                observer,
                context,
                ids,
            };
            let context = (&mut *owner.context as *mut Context).cast();
            for (_, window) in &owner.context.windows {
                for name in [
                    "AXMoved",
                    "AXResized",
                    "AXUIElementDestroyed",
                    "AXWindowMiniaturized",
                    "AXWindowDeminiaturized",
                    "AXTitleChanged",
                ] {
                    // SAFETY: retained observer, element and context outlive the subscription; AX copies the name.
                    let error = unsafe {
                        AXObserverAddNotification(
                            owner.observer.as_ptr(),
                            window.as_ptr(),
                            CFString::new(name).as_concrete_TypeRef(),
                            context,
                        )
                    };
                    if error != AX_OK && error != -25207 && error != -25209 {
                        return Err(format!("Cannot observe tab member: AXError {error}"));
                    }
                }
            }
            let app = AxApplication::new(pid)?;
            // SAFETY: all references live through registration; only this worker runs/removes the source.
            unsafe {
                let _ = AXObserverAddNotification(
                    owner.observer.as_ptr(),
                    app.as_ptr(),
                    CFString::new("AXFocusedWindowChanged").as_concrete_TypeRef(),
                    context,
                );
                CFRunLoopAddSource(
                    CFRunLoopGetCurrent(),
                    AXObserverGetRunLoopSource(owner.observer.as_ptr()),
                    crate::platform::macos::native::default_run_loop_modes().core_foundation,
                );
            }
            self.observers.insert(pid, owner);
        }
        Ok(())
    }
    pub fn pump(&mut self) {
        // SAFETY: bounded nonblocking dispatch of this worker's installed AX sources.
        unsafe {
            for _ in 0..64 {
                if CFRunLoopRunInMode(
                    crate::platform::macos::native::default_run_loop_modes().core_foundation,
                    0.0,
                    1,
                ) != 4
                {
                    break;
                }
            }
        }
    }
}
