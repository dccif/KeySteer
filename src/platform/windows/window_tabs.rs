//! Independent, non-activating tab strips. Application windows stay top-level.
mod strip;
use crate::api::Screen;
use crate::api::window::WindowId;
use crate::api::window_tabs::{TabBar, TabDrop, TabGroupId, TabNativeEvent, WindowTarget};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(super) fn event_waker() -> std::sync::Arc<dyn Fn() + Send + Sync> {
    // SAFETY: create this worker's message queue before publishing its thread id.
    let thread = unsafe {
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        windows::Win32::System::Threading::GetCurrentThreadId()
    };
    std::sync::Arc::new(move || {
        // SAFETY: WM_NULL carries no pointers; the waker belongs to the worker
        // mailbox and is used only while that worker is accepting work/stopping.
        unsafe {
            let _ = PostThreadMessageW(thread, WM_NULL, Default::default(), Default::default());
        }
    })
}

pub(super) fn wait_for_events(timeout_ms: u32) {
    // SAFETY: wait only on this thread's queue, without holding mailbox locks.
    // INPUTAVAILABLE covers messages posted before entering the wait. The
    // Normal tracking waits indefinitely. Only failed visibility recovery has a
    // timeout; neither movement nor selection is driven by that deadline.
    unsafe {
        MsgWaitForMultipleObjectsEx(None, timeout_ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_wake_queued_before_native_wait_is_not_lost() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let (ready, receive) = mpsc::channel();
        let (posted, proceed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            ready.send(event_waker()).unwrap();
            proceed.recv().unwrap();
            let start = Instant::now();
            wait_for_events(5000);
            start.elapsed()
        });
        let wake = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        wake();
        posted.send(()).unwrap();
        assert!(worker.join().unwrap() < Duration::from_secs(1));
    }
}
thread_local! {
    static WATCHED: RefCell<BTreeMap<isize, WindowId>> = const { RefCell::new(BTreeMap::new()) };
    static EVENTS: RefCell<Vec<TabNativeEvent>> = const { RefCell::new(Vec::new()) };
    static TRACKING: RefCell<BTreeMap<WindowId, Weak<strip::Strip>>> = const { RefCell::new(BTreeMap::new()) };
    static INTERACTING: RefCell<BTreeSet<WindowId>> = const { RefCell::new(BTreeSet::new()) };
    static SCREENS: RefCell<Vec<Screen>> = const { RefCell::new(Vec::new()) };
}
pub(super) fn interacting(id: WindowId) -> bool {
    INTERACTING.with(|ids| ids.borrow().contains(&id))
}
fn enqueue(event: TabNativeEvent) {
    EVENTS.with(|events| {
        let mut events = events.borrow_mut();
        if !events.contains(&event) && events.len() < 512 {
            events.push(event);
        }
    });
}
fn drag_over(hwnd: HWND, point: POINT, source: WindowTarget) -> Option<TabDrop> {
    TRACKING.with(|strips| {
        let mut result = None;
        for strip in strips.borrow().values().filter_map(Weak::upgrade) {
            let hit = strip
                .is_handle(hwnd)
                .then(|| strip.drop_at(source, point))
                .flatten();
            strip.mark_drop(hit.map(|(_, marker)| marker));
            if let Some((drop, _)) = hit {
                result = Some(drop);
            }
        }
        result
    })
}
fn clear_drag() {
    TRACKING.with(|strips| {
        for strip in strips.borrow().values().filter_map(Weak::upgrade) {
            strip.mark_drop(None);
        }
    });
}
unsafe extern "system" fn window_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    handle_window_event(event, hwnd, object);
}

pub(super) fn handle_window_event(event: u32, hwnd: HWND, object: i32) {
    if event == EVENT_SYSTEM_FOREGROUND {
        enqueue(TabNativeEvent::VisibilityChanged);
    }
    if object != 0 && event >= EVENT_OBJECT_CREATE && event != EVENT_OBJECT_FOCUS {
        return;
    }
    let direct = WATCHED.with(|watched| watched.borrow().get(&(hwnd.0 as isize)).copied());
    if let Some(id) = direct {
        if event == EVENT_SYSTEM_MOVESIZESTART {
            INTERACTING.with(|ids| {
                ids.borrow_mut().insert(id);
            });
            enqueue(TabNativeEvent::MoveResizeStarted(id));
            return;
        }
        if event == EVENT_SYSTEM_MOVESIZEEND {
            INTERACTING.with(|ids| {
                ids.borrow_mut().remove(&id);
            });
            enqueue(TabNativeEvent::MoveResizeEnded(id));
            return;
        }
        if event == EVENT_OBJECT_LOCATIONCHANGE {
            let strip = TRACKING.with(|strips| strips.try_borrow().ok()?.get(&id)?.upgrade());
            let followed = strip.is_some_and(|strip| {
                SCREENS.with(|screens| {
                    screens
                        .try_borrow()
                        .is_ok_and(|screens| strip.follow(hwnd, &screens))
                })
            });
            // Native move/size updates only the owned strip. The end event
            // reconciles the model once, without fighting the application.
            if !followed || !interacting(id) {
                enqueue(TabNativeEvent::GeometryChanged(id));
            }
            return;
        }
        if event == EVENT_OBJECT_DESTROY {
            INTERACTING.with(|ids| {
                ids.borrow_mut().remove(&id);
            });
        }
        enqueue(match event {
            EVENT_OBJECT_DESTROY => TabNativeEvent::Closed(id),
            EVENT_SYSTEM_FOREGROUND | EVENT_OBJECT_FOCUS => TabNativeEvent::Focused(id),
            _ => TabNativeEvent::Changed(id),
        });
    }
}

#[derive(Default)]
pub(super) struct NativeTabs {
    hooks: Vec<HWINEVENTHOOK>,
    strips: BTreeMap<TabGroupId, Rc<strip::Strip>>,
}
impl NativeTabs {
    pub fn member_shown(&self, id: WindowId, owner: HWND) -> Result<(), String> {
        for strip in self.strips.values().filter(|strip| strip.contains(id)) {
            strip.set_owner(owner)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn strip_handle(&self, group: TabGroupId) -> HWND {
        self.strips[&group].handle()
    }
    pub fn watch(&mut self, identities: &[(WindowId, HWND)]) -> Result<(), String> {
        WATCHED.with(|watched| {
            *watched.borrow_mut() = identities
                .iter()
                .map(|(id, hwnd)| (hwnd.0 as isize, *id))
                .collect()
        });
        INTERACTING.with(|ids| {
            ids.borrow_mut()
                .retain(|id| identities.iter().any(|(current, _)| current == id))
        });
        if identities.is_empty() {
            self.unhook();
            return Ok(());
        }
        if self.hooks.is_empty() {
            for (first, last) in [
                (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
                (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
                (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
                (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE),
                (EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS),
            ] {
                // SAFETY: callback queues identities and moves only owned strips on this thread.
                // App movement remains entirely with the application/window manager.
                let hook = unsafe {
                    SetWinEventHook(
                        first,
                        last,
                        None,
                        Some(window_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };
                if hook.0.is_null() {
                    self.unhook();
                    return Err("Cannot monitor grouped windows".into());
                }
                self.hooks.push(hook);
            }
        }
        Ok(())
    }
    fn unhook(&mut self) {
        for hook in self.hooks.drain(..) {
            // SAFETY: consume each owned event-hook token once on its owner thread.
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
    pub fn dispatch_messages() {
        // SAFETY: dispatch only this worker's messages with a finite budget.
        // Only the small independent strips belong to this worker.
        unsafe {
            let mut message = MSG::default();
            for _ in 0..128 {
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    pub fn events(&mut self) -> Vec<TabNativeEvent> {
        Self::dispatch_messages();
        EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }
    pub fn update_bar(&mut self, bar: &TabBar, screens: &[Screen]) -> Result<(), String> {
        // Membership and ownership cannot change on this path.
        let strip = self
            .strips
            .get(&bar.group)
            .ok_or("Tab strip is no longer available")?;
        strip.update(bar, screens.get(bar.screen))
    }
    pub fn show(&mut self, bars: &[TabBar], screens: &[Screen]) -> Result<(), String> {
        SCREENS.with(|current| {
            if *current.borrow() != screens {
                *current.borrow_mut() = screens.to_vec();
            }
        });
        self.strips
            .retain(|id, strip| strip.alive() && bars.iter().any(|bar| bar.group == *id));
        for bar in bars {
            let owner = WATCHED
                .with(|watched| {
                    watched
                        .borrow()
                        .iter()
                        .find_map(|(hwnd, id)| (*id == bar.active).then_some(HWND(*hwnd as *mut _)))
                })
                .ok_or("Tab owner is no longer available")?;
            if let std::collections::btree_map::Entry::Vacant(entry) = self.strips.entry(bar.group)
            {
                entry.insert(strip::Strip::new(bar, owner)?);
            }
            self.strips[&bar.group].set_owner(owner)?;
            self.strips[&bar.group].update(bar, screens.get(bar.screen))?;
        }
        TRACKING.with(|tracking| {
            *tracking.borrow_mut() = bars
                .iter()
                .filter_map(|bar| {
                    self.strips
                        .get(&bar.group)
                        .map(|strip| (bar.active, Rc::downgrade(strip)))
                })
                .collect();
        });
        Ok(())
    }
}
impl Drop for NativeTabs {
    fn drop(&mut self) {
        self.unhook();
        INTERACTING.with(|w| w.borrow_mut().clear());
        WATCHED.with(|w| w.borrow_mut().clear());
        TRACKING.with(|w| w.borrow_mut().clear());
        SCREENS.with(|w| w.borrow_mut().clear());
    }
}
