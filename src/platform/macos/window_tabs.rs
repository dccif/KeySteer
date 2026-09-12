//! Main-thread floating strips; the worker exchanges only portable values.
use crate::api::{
    Screen,
    window::WindowId,
    window_tabs::{TabBar, TabNativeEvent},
};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSPanel, NSTextField, NSView, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct PendingBars {
    full: Option<Vec<TabBar>>,
    updates: BTreeMap<u32, TabBar>,
}
static PENDING: Mutex<PendingBars> = Mutex::new(PendingBars {
    full: None,
    updates: BTreeMap::new(),
});
static EVENTS: Mutex<Vec<TabNativeEvent>> = Mutex::new(Vec::new());
thread_local! { static STRIPS: RefCell<BTreeMap<u32, Strip>> = const { RefCell::new(BTreeMap::new()) }; }

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KeySteerTabButtonTarget"]
    #[ivars = TabNativeEvent]
    struct Target;
    impl Target {
        #[unsafe(method(selectTab:))]
        fn select_tab(&self, _sender: Option<&NSButton>) {
            enqueue(*self.ivars());
        }
    }
);
struct Strip {
    panel: Retained<NSPanel>,
    targets: Vec<Retained<Target>>,
    buttons: Vec<Retained<NSButton>>,
    tabs: Vec<(WindowId, u32, String)>,
    layout_width: f64,
    active: Option<WindowId>,
}
impl Drop for Strip {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}
pub(super) fn enqueue(event: TabNativeEvent) {
    let mut events = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if !events.contains(&event) && events.len() < 512 {
        events.push(event);
    }
}
pub(super) fn publish(bars: &[TabBar]) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.full = Some(bars.to_vec());
        pending.updates.clear();
        super::workspace::wake_main_run_loop();
    }
}
pub(super) fn publish_one(bar: &TabBar) -> Result<(), String> {
    let mut pending = PENDING
        .lock()
        .map_err(|_| "Tab publication queue poisoned")?;
    if let Some(full) = &mut pending.full {
        if let Some(current) = full.iter_mut().find(|current| current.group == bar.group) {
            current.clone_from(bar);
        }
    } else {
        pending.updates.insert(bar.group.0, bar.clone());
    }
    drop(pending);
    super::workspace::wake_main_run_loop();
    Ok(())
}
pub(super) fn take_events() -> Vec<TabNativeEvent> {
    EVENTS
        .lock()
        .map(|mut events| std::mem::take(&mut *events))
        .unwrap_or_default()
}
pub(super) fn clear(mtm: MainThreadMarker) {
    refresh(mtm, &[]);
    STRIPS.with(|strips| strips.borrow_mut().clear());
}
pub(super) fn refresh(mtm: MainThreadMarker, screens: &[Screen]) {
    thread_local! { static FOREGROUND: std::cell::Cell<i32> = const { std::cell::Cell::new(0) }; }
    let pid = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map_or(0, |app| app.processIdentifier());
    FOREGROUND.with(|previous| {
        if previous.replace(pid) != pid {
            enqueue(TabNativeEvent::VisibilityChanged);
        }
    });
    let Some(pending) = PENDING.lock().ok().map(|mut p| std::mem::take(&mut *p)) else {
        return;
    };
    let structural = pending.full.is_some();
    let bars = pending
        .full
        .unwrap_or_else(|| pending.updates.into_values().collect());
    if bars.is_empty() && !structural {
        return;
    }
    let top = screens
        .iter()
        .find(|s| s.is_primary)
        .map_or(0.0, |s| s.bounds.bottom());
    STRIPS.with(|strips| {
        let mut strips = strips.borrow_mut();
        if structural {
            strips.retain(|id, _| bars.iter().any(|bar| bar.group.0 == *id));
        }
        for bar in bars {
            if !structural && !strips.contains_key(&bar.group.0) {
                continue;
            }
            let strip = strips.entry(bar.group.0).or_insert_with(|| {
                let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                    NSPanel::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 30.0)),
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    NSBackingStoreType::Buffered,
                    false,
                );
                panel.setHidesOnDeactivate(false);
                panel.setBecomesKeyOnlyIfNeeded(true);
                panel.setHasShadow(false);
                panel.setLevel(3);
                Strip {
                    panel,
                    targets: Vec::new(),
                    buttons: Vec::new(),
                    tabs: Vec::new(),
                    layout_width: 0.0,
                    active: None,
                }
            });
            let layout_changed = strip.layout_width != bar.bounds.width || strip.tabs != bar.tabs;
            let selection_changed = strip.active != Some(bar.active) || strip.tabs != bar.tabs;
            if strip.tabs != bar.tabs {
                let content = NSView::initWithFrame(
                    NSView::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(bar.bounds.width, 30.0)),
                );
                let heading = NSTextField::labelWithString(
                    &NSString::from_str(&format!("~{}", bar.group.0)),
                    mtm,
                );
                heading.setFrame(NSRect::new(NSPoint::new(8.0, 6.0), NSSize::new(36.0, 24.0)));
                content.addSubview(&heading);
                let mut targets = Vec::new();
                let mut buttons = Vec::new();
                for (title, event) in bar
                    .tabs
                    .iter()
                    .map(|(id, n, title)| {
                        (
                            if *n == 0 {
                                title.clone()
                            } else {
                                format!("{n}  {title}")
                            },
                            TabNativeEvent::Activate(*id),
                        )
                    })
                    .chain(std::iter::once((
                        "×".into(),
                        TabNativeEvent::Dissolve(bar.group),
                    )))
                {
                    let allocated = Target::alloc(mtm).set_ivars(event);
                    // SAFETY: initialize the target on the AppKit thread, retain through button lifetime.
                    let target: Retained<Target> = unsafe { msg_send![super(allocated), init] };
                    // SAFETY: selector is implemented above; targets stay retained while buttons are installed.
                    let button = unsafe {
                        NSButton::buttonWithTitle_target_action(
                            &NSString::from_str(&title),
                            Some(&*target),
                            Some(sel!(selectTab:)),
                            mtm,
                        )
                    };
                    button.setKeyEquivalent(&NSString::from_str(""));
                    content.addSubview(&button);
                    targets.push(target);
                    buttons.push(button);
                }
                strip.panel.setContentView(Some(&content));
                strip.buttons = buttons;
                strip.targets = targets;
                strip.tabs = bar.tabs.clone();
            }
            let width = (bar.bounds.width - 74.0).max(1.0) / bar.tabs.len().max(1) as f64;
            for (i, button) in strip.buttons.iter().enumerate() {
                let close = i == bar.tabs.len();
                if layout_changed {
                    button.setFrame(NSRect::new(
                        NSPoint::new(
                            if close {
                                bar.bounds.width - 30.0
                            } else {
                                44.0 + i as f64 * width
                            },
                            0.0,
                        ),
                        NSSize::new(if close { 30.0 } else { width }, 30.0),
                    ));
                }
                if selection_changed {
                    button.setState(
                        if bar.tabs.get(i).is_some_and(|(id, _, _)| *id == bar.active) {
                            1
                        } else {
                            0
                        },
                    );
                }
            }
            strip.layout_width = bar.bounds.width;
            strip.active = Some(bar.active);
            let above = bar.bounds.y - 30.0;
            let y = above;
            strip.panel.setFrame_display(
                NSRect::new(
                    NSPoint::new(bar.bounds.x, top - y - 30.0),
                    NSSize::new(bar.bounds.width, 30.0),
                ),
                layout_changed || selection_changed,
            );
            if bar.visible
                && screens
                    .get(bar.screen)
                    .is_none_or(|screen| above >= screen.work_area.y - 1.0)
            {
                strip.panel.orderFrontRegardless();
            } else {
                strip.panel.orderOut(None);
            }
        }
    });
}
