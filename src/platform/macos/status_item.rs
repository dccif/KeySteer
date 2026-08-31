//! Native menu-bar controls.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage, NSImageView, NSMenu,
    NSMenuItem, NSPanel, NSSquareStatusItemLength, NSStatusBar, NSStatusItem, NSTextField, NSView,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString, ns_string};

use crate::api::Autostart;
use crate::api::backend::{BackendEvent, UpdateCheckResult, UpdateProgress};

use super::EventSender;

static SENDER: OnceLock<Mutex<Option<EventSender>>> = OnceLock::new();

const STATUS_ITEM_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_ITEM_REBUILD_INTERVAL: Duration = Duration::from_secs(3);
const STATUS_ITEM_RECOVERY_WINDOW: Duration = Duration::from_secs(10);
const STATUS_ITEM_REBUILDS: u8 = 3;
const STATUS_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/keysteer-icon.png");
const STATUS_ICON_SIZE: f64 = 18.0;

struct StatusTargetIvars {
    update_alert: RefCell<Option<Retained<NSPanel>>>,
    downloaded_update: RefCell<Option<PathBuf>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KeySteerStatusTarget"]
    #[ivars = StatusTargetIvars]
    struct StatusTarget;

    impl StatusTarget {
        #[unsafe(method(toggleEnabled:))]
        fn toggle_enabled(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::ToggleEnabled);
        }

        #[unsafe(method(reloadConfig:))]
        fn reload_config(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::ReloadConfig);
        }

        #[unsafe(method(openConfigSimulator:))]
        fn open_config_simulator(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::OpenConfigSimulator);
        }

        #[unsafe(method(toggleAutostart:))]
        fn toggle_autostart(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::ToggleAutostart);
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::CheckForUpdates);
        }

        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = show_panel(
                self.mtm(),
                self,
                "About KeySteer",
                &crate::app::about::details(),
                None,
            ) {
                crate::app::logging::report_error("macos-about", error);
            }
        }

        #[unsafe(method(dismissUpdateAlert:))]
        fn dismiss_update_alert_action(&self, _sender: Option<&AnyObject>) {
            self.dismiss_update_alert();
        }

        #[unsafe(method(showDownloadedUpdate:))]
        fn show_downloaded_update(&self, _sender: Option<&AnyObject>) {
            let downloaded_update = self.ivars().downloaded_update.borrow();
            let Some(path) = downloaded_update.as_deref() else {
                return;
            };
            let full_path = NSString::from_str(&path.to_string_lossy());
            let root_path = NSString::from_str(
                &path
                    .parent()
                    .unwrap_or(path)
                    .to_string_lossy(),
            );
            if !NSWorkspace::sharedWorkspace()
                .selectFile_inFileViewerRootedAtPath(Some(&full_path), &root_path)
            {
                crate::app::logging::report_error(
                    "macos-update",
                    format!("Finder could not reveal {}", path.display()),
                );
            }
        }

        #[unsafe(method(quitApplication:))]
        fn quit_application(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::Quit);
        }
    }
);

impl StatusTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(StatusTargetIvars {
            update_alert: RefCell::new(None),
            downloaded_update: RefCell::new(None),
        });
        // SAFETY: The NSObject superclass is initialized after Rust ivars.
        unsafe { msg_send![super(this), init] }
    }

    fn show_update_alert(&self, alert: Retained<NSPanel>, downloaded_update: Option<PathBuf>) {
        self.dismiss_update_alert();
        *self.ivars().downloaded_update.borrow_mut() = downloaded_update;
        *self.ivars().update_alert.borrow_mut() = Some(alert);
        let alert = self.ivars().update_alert.borrow();
        if let Some(window) = alert.as_ref() {
            window.center();
            NSApplication::sharedApplication(self.mtm()).activate();
            window.makeKeyAndOrderFront(None);
        }
    }

    fn dismiss_update_alert(&self) {
        self.ivars().downloaded_update.borrow_mut().take();
        if let Some(alert) = self.ivars().update_alert.borrow_mut().take() {
            alert.close();
        }
    }
}

pub struct StatusItem {
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    icon: Option<Retained<NSImage>>,
    _target: Retained<StatusTarget>,
    toggle_item: Retained<NSMenuItem>,
    autostart_item: Retained<NSMenuItem>,
    update_item: Retained<NSMenuItem>,
    enabled: bool,
    startup_repair: Option<StartupRepair>,
    button_configured: bool,
}

#[derive(Clone, Copy)]
struct StartupRepair {
    next_check: Instant,
    next_rebuild: Instant,
    deadline: Instant,
    rebuilds_remaining: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeItemState {
    Attached,
    Detached { visible: bool },
    Hidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepairAction {
    Wait,
    Complete,
    Rebuild,
    FallBack,
}

impl StartupRepair {
    fn observe(&mut self, now: Instant, state: NativeItemState) -> RepairAction {
        match state {
            NativeItemState::Hidden | NativeItemState::Detached { visible: false }
                if now >= self.deadline =>
            {
                RepairAction::FallBack
            }
            NativeItemState::Hidden | NativeItemState::Detached { visible: false }
                if self.rebuilds_remaining > 0 && now >= self.next_rebuild =>
            {
                self.rebuilds_remaining -= 1;
                self.next_rebuild = now + STATUS_ITEM_REBUILD_INTERVAL;
                self.next_check = now + STATUS_ITEM_CHECK_INTERVAL;
                RepairAction::Rebuild
            }
            NativeItemState::Hidden | NativeItemState::Detached { visible: false } => {
                self.next_check = now + STATUS_ITEM_CHECK_INTERVAL;
                RepairAction::Wait
            }
            NativeItemState::Attached if now >= self.deadline => RepairAction::Complete,
            NativeItemState::Attached => {
                self.next_check = now + STATUS_ITEM_CHECK_INTERVAL;
                RepairAction::Wait
            }
            NativeItemState::Detached { visible: true } if now >= self.deadline => {
                RepairAction::FallBack
            }
            NativeItemState::Detached { visible: true }
                if self.rebuilds_remaining > 0 && now >= self.next_rebuild =>
            {
                self.rebuilds_remaining -= 1;
                self.next_rebuild = now + STATUS_ITEM_REBUILD_INTERVAL;
                self.next_check = now + STATUS_ITEM_CHECK_INTERVAL;
                RepairAction::Rebuild
            }
            NativeItemState::Detached { visible: true } => {
                self.next_check = now + STATUS_ITEM_CHECK_INTERVAL;
                RepairAction::Wait
            }
        }
    }
}

impl StatusItem {
    pub(super) fn new(mtm: MainThreadMarker, sender: EventSender) -> Self {
        *SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(sender);

        let application = NSApplication::sharedApplication(mtm);
        if !application.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
            && application.activationPolicy() != NSApplicationActivationPolicy::Accessory
        {
            crate::app::logging::report_error(
                "macos-status-item",
                "AppKit rejected the accessory activation policy; the menu-bar item may be unavailable",
            );
        }
        application.finishLaunching();

        let target = StatusTarget::new(mtm);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let toggle_item = menu_item(mtm, "Pause", sel!(toggleEnabled:), &target);
        let reload_item = menu_item(mtm, "Reload Configuration", sel!(reloadConfig:), &target);
        let simulator_item = menu_item(
            mtm,
            "Configuration & Simulator...",
            sel!(openConfigSimulator:),
            &target,
        );
        let autostart_item = menu_item(mtm, "Start at Login", sel!(toggleAutostart:), &target);
        let update_item = menu_item(mtm, "Check for Updates...", sel!(checkForUpdates:), &target);
        let about_item = menu_item(mtm, "About KeySteer...", sel!(showAbout:), &target);
        let autostart_enabled = match super::autostart::MacosAutostart::new().is_enabled() {
            Ok(enabled) => enabled,
            Err(error) => {
                crate::app::logging::report_error("macos-autostart", error);
                false
            }
        };
        set_checked(&autostart_item, autostart_enabled);
        let quit_item = menu_item(mtm, "Quit KeySteer", sel!(quitApplication:), &target);
        menu.addItem(&toggle_item);
        menu.addItem(&reload_item);
        menu.addItem(&simulator_item);
        menu.addItem(&autostart_item);
        menu.addItem(&update_item);
        menu.addItem(&about_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&quit_item);

        let icon = status_icon(STATUS_ICON_SIZE);
        let (item, button_configured) = create_native_item(mtm, &menu, icon.as_deref());
        let startup_now = Instant::now();
        let startup_repair = Some(StartupRepair {
            next_check: startup_now,
            next_rebuild: startup_now + Duration::from_secs(1),
            deadline: startup_now + STATUS_ITEM_RECOVERY_WINDOW,
            rebuilds_remaining: STATUS_ITEM_REBUILDS,
        });

        Self {
            item,
            menu,
            icon,
            _target: target,
            toggle_item,
            autostart_item,
            update_item,
            enabled: true,
            startup_repair,
            button_configured,
        }
    }

    /// Verify the status item after AppKit has completed at least one run-loop
    /// turn. Login items can start while the menu-bar scene is still attaching;
    /// a detached item must not be accepted as permanently ready.
    pub(super) fn maintain_startup(&mut self) {
        let Some(repair) = self.startup_repair.as_ref() else {
            return;
        };
        let now = Instant::now();
        if now < repair.next_check {
            return;
        }
        let Some(mut repair) = self.startup_repair.take() else {
            return;
        };

        let mtm = self._target.mtm();
        let mut state = self.native_item_state(mtm);
        if matches!(state, NativeItemState::Detached { .. }) {
            NSApplication::sharedApplication(mtm).updateWindows();
            state = self.native_item_state(mtm);
        }
        match repair.observe(now, state) {
            RepairAction::Wait => self.startup_repair = Some(repair),
            RepairAction::Complete => {}
            RepairAction::Rebuild => {
                self.rebuild_native_item(mtm);
                self.startup_repair = Some(repair);
            }
            RepairAction::FallBack => self.fall_back_to_dock(mtm),
        }
    }

    fn fall_back_to_dock(&mut self, mtm: MainThreadMarker) {
        if let Some(status_bar) = self.item.statusBar() {
            status_bar.removeStatusItem(&self.item);
        }
        let application = NSApplication::sharedApplication(mtm);
        let dock_available = application
            .setActivationPolicy(NSApplicationActivationPolicy::Regular)
            || application.activationPolicy() == NSApplicationActivationPolicy::Regular;
        crate::app::logging::report_error(
            "macos-status-item",
            if dock_available {
                "the menu-bar item did not attach after startup recovery; showing KeySteer in the Dock instead"
            } else {
                "the menu-bar item did not attach after startup recovery, and AppKit rejected the Dock fallback"
            },
        );
    }

    fn native_item_state(&mut self, mtm: MainThreadMarker) -> NativeItemState {
        if !self.item.isVisible() {
            // `autosaveName` restores the user's previous visibility. KeySteer
            // has no Dock presence in accessory mode, so a stale hidden value
            // would otherwise leave no UI for pausing or quitting the app.
            self.item.setVisible(true);
        }
        let visible = self.item.isVisible();
        if self.item.statusBar().is_none() {
            return NativeItemState::Detached { visible };
        }
        let Some(button) = self.item.button(mtm) else {
            return NativeItemState::Detached { visible };
        };
        if !self.button_configured {
            configure_status_button(&button, self.icon.as_deref());
            self.button_configured = true;
        }
        if !visible {
            NativeItemState::Hidden
        } else if button.window().is_none() {
            NativeItemState::Detached { visible }
        } else {
            NativeItemState::Attached
        }
    }

    fn rebuild_native_item(&mut self, mtm: MainThreadMarker) {
        if let Some(status_bar) = self.item.statusBar() {
            status_bar.removeStatusItem(&self.item);
        }
        let (item, button_configured) = create_native_item(mtm, &self.menu, self.icon.as_deref());
        self.item = item;
        self.button_configured = button_configured;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.toggle_item.setTitle(&NSString::from_str(if enabled {
            "Pause"
        } else {
            "Resume"
        }));
    }

    pub(super) fn set_autostart_enabled(&mut self, enabled: bool) {
        set_checked(&self.autostart_item, enabled);
    }

    pub(super) fn present_update_progress(&mut self, progress: &UpdateProgress) {
        let title = match progress {
            UpdateProgress::Checking => "Checking for Updates...".to_string(),
            UpdateProgress::Downloading { latest, percent } => {
                format!("Downloading KeySteer {latest}... {}%", (*percent).min(100))
            }
        };
        self.set_update_menu(&title, false);
    }

    pub(super) fn present_update_result(
        &mut self,
        result: &UpdateCheckResult,
    ) -> Result<(), String> {
        let title = match result {
            UpdateCheckResult::UpdateDownloaded { latest, .. } => {
                format!("KeySteer {latest} Downloaded")
            }
            UpdateCheckResult::UpToDate { current } => {
                format!("KeySteer {current} Is Up to Date")
            }
            UpdateCheckResult::Failed(_) => "Update Check Failed - Retry...".to_string(),
        };
        self.set_update_menu(&title, true);
        match result {
            UpdateCheckResult::UpdateDownloaded {
                current,
                latest,
                path,
            } => self.show_alert(
                "KeySteer update downloaded",
                &format!(
                    "KeySteer {latest} was saved to {}. Quit KeySteer, extract the ZIP, then move the new app to Applications to replace version {current}.",
                    path.display()
                ),
                Some(path),
            ),
            UpdateCheckResult::UpToDate { current } => self.show_alert(
                "KeySteer is up to date",
                &format!("KeySteer {current} is already the latest version."),
                None,
            ),
            UpdateCheckResult::Failed(error) => {
                self.show_alert("Could not check for updates", error, None)
            }
        }
    }

    fn set_update_menu(&self, title: &str, enabled: bool) {
        self.update_item.setTitle(&NSString::from_str(title));
        self.update_item.setEnabled(enabled);
    }

    fn show_alert(
        &self,
        title: &str,
        details: &str,
        downloaded_update: Option<&Path>,
    ) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            "update result must be presented on the macOS main thread".to_string()
        })?;
        show_panel(mtm, &self._target, title, details, downloaded_update)
    }
}

fn show_panel(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    details: &str,
    downloaded_update: Option<&Path>,
) -> Result<(), String> {
    autoreleasepool(|_| {
        const WIDTH: f64 = 520.0;
        const HEIGHT: f64 = 210.0;
        let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            content_rect,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        );
        if panel.isReleasedWhenClosed() {
            return Err(
                "macOS panel unexpectedly releases itself when closed; refusing ambiguous ownership"
                    .into(),
            );
        }

        panel.setTitle(&NSString::from_str("KeySteer"));
        panel.setFloatingPanel(true);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(false);

        let content = NSView::initWithFrame(NSView::alloc(mtm), content_rect);
        if let Some(icon) = status_icon(64.0) {
            let image_view = NSImageView::imageViewWithImage(&icon, mtm);
            image_view.setFrame(NSRect::new(
                NSPoint::new(28.0, 118.0),
                NSSize::new(64.0, 64.0),
            ));
            content.addSubview(&image_view);
        }

        let title_label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
        title_label.setFont(Some(&NSFont::boldSystemFontOfSize(17.0)));
        title_label.setFrame(NSRect::new(
            NSPoint::new(112.0, 158.0),
            NSSize::new(380.0, 24.0),
        ));
        content.addSubview(&title_label);

        let details_label = NSTextField::wrappingLabelWithString(&NSString::from_str(details), mtm);
        details_label.setFrame(NSRect::new(
            NSPoint::new(112.0, 58.0),
            NSSize::new(380.0, 88.0),
        ));
        content.addSubview(&details_label);

        // SAFETY: both selectors are implemented by the retained target with
        // matching Objective-C signatures; AppKit retains no Rust borrow.
        let (button, reveal_button) = unsafe {
            let button = NSButton::buttonWithTitle_target_action(
                &NSString::from_str("OK"),
                Some(target),
                Some(sel!(dismissUpdateAlert:)),
                mtm,
            );
            let reveal_button = downloaded_update.is_some().then(|| {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Show in Finder"),
                    Some(target),
                    Some(sel!(showDownloadedUpdate:)),
                    mtm,
                )
            });
            (button, reveal_button)
        };
        button.setFrame(NSRect::new(
            NSPoint::new(412.0, 16.0),
            NSSize::new(80.0, 32.0),
        ));
        button.setKeyEquivalent(&NSString::from_str("\r"));
        content.addSubview(&button);

        if let Some(reveal_button) = reveal_button {
            reveal_button.setFrame(NSRect::new(
                NSPoint::new(276.0, 16.0),
                NSSize::new(124.0, 32.0),
            ));
            content.addSubview(&reveal_button);
        }

        panel.setContentView(Some(&content));
        target.show_update_alert(panel, downloaded_update.map(Path::to_path_buf));
        Ok(())
    })
}

fn create_native_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    icon: Option<&NSImage>,
) -> (Retained<NSStatusItem>, bool) {
    let status_bar = NSStatusBar::systemStatusBar();
    let item = status_bar.statusItemWithLength(NSSquareStatusItemLength);
    let button_configured = item.button(mtm).is_some_and(|button| {
        configure_status_button(&button, icon);
        true
    });
    item.setMenu(Some(menu));
    // Apply persisted visibility only after configuring the default-visible
    // button, so a later System Settings unhide cannot reveal a blank item.
    item.setAutosaveName(Some(ns_string!("com.keysteer.app.status-item")));
    item.setVisible(true);
    (item, button_configured)
}

fn configure_status_button(button: &objc2_app_kit::NSStatusBarButton, icon: Option<&NSImage>) {
    if let Some(image) = icon {
        button.setImage(Some(image));
        button.setTitle(&NSString::from_str(""));
    } else if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("cursorarrow.motionlines"),
        Some(&NSString::from_str("KeySteer")),
    ) {
        button.setImage(Some(&image));
        button.setTitle(&NSString::from_str(""));
    } else {
        button.setTitle(&NSString::from_str("KeySteer"));
    }
}

fn status_icon(size: f64) -> Option<Retained<NSImage>> {
    // SAFETY: the static PNG byte slice is live for the complete call and
    // NSData copies/retains the supplied bytes according to this initializer.
    let data = unsafe {
        NSData::dataWithBytes_length(STATUS_ICON_PNG.as_ptr().cast(), STATUS_ICON_PNG.len())
    };
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(size, size));
    image.setTemplate(false);
    image.setAccessibilityDescription(Some(&NSString::from_str("KeySteer")));
    Some(image)
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        self._target.dismiss_update_alert();
        if let Some(mutex) = SENDER.get() {
            *mutex.lock().unwrap_or_else(|error| error.into_inner()) = None;
        }
        if let Some(status_bar) = self.item.statusBar() {
            status_bar.removeStatusItem(&self.item);
        }
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: objc2::runtime::Sel,
    target: &StatusTarget,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setEnabled(true);
    // SAFETY: `action` names a method with the menu-item action signature and
    // `target` remains retained by StatusItem for the menu lifetime.
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(target));
    }
    item
}

fn set_checked(item: &NSMenuItem, checked: bool) {
    item.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
}

fn emit(event: BackendEvent) {
    let Some(sender) = SENDER
        .get()
        .and_then(|sender| sender.lock().ok())
        .and_then(|sender| sender.clone())
    else {
        return;
    };
    let _ = sender.send(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn startup_repair(start: Instant) -> StartupRepair {
        StartupRepair {
            next_check: start,
            next_rebuild: start + Duration::from_secs(1),
            deadline: start + STATUS_ITEM_RECOVERY_WINDOW,
            rebuilds_remaining: STATUS_ITEM_REBUILDS,
        }
    }

    #[test]
    fn startup_repair_watches_the_complete_horizon_for_scene_replacement() {
        let start = Instant::now();
        let mut repair = startup_repair(start);
        assert_eq!(
            repair.observe(start, NativeItemState::Attached),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(start + Duration::from_secs(2), NativeItemState::Attached),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(
                start + Duration::from_secs(3),
                NativeItemState::Detached { visible: true }
            ),
            RepairAction::Rebuild
        );
        assert_eq!(
            repair.observe(
                start + Duration::from_millis(9_900),
                NativeItemState::Attached
            ),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(
                start + STATUS_ITEM_RECOVERY_WINDOW,
                NativeItemState::Attached
            ),
            RepairAction::Complete
        );
    }

    #[test]
    fn startup_repair_recovers_a_cold_login_and_resets_transient_hidden_state() {
        let start = Instant::now();
        let mut repair = startup_repair(start);
        assert_eq!(
            repair.observe(
                start + Duration::from_secs(1),
                NativeItemState::Detached { visible: true }
            ),
            RepairAction::Rebuild
        );
        assert_eq!(
            repair.observe(start + Duration::from_secs(2), NativeItemState::Attached),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(start + Duration::from_secs(3), NativeItemState::Hidden),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(start + Duration::from_secs(4), NativeItemState::Attached),
            RepairAction::Wait
        );
        assert_eq!(
            repair.observe(
                start + STATUS_ITEM_RECOVERY_WINDOW,
                NativeItemState::Attached
            ),
            RepairAction::Complete
        );
    }

    #[test]
    fn startup_repair_restores_hidden_items_and_bounds_native_rebuilds() {
        let start = Instant::now();
        let mut hidden = startup_repair(start);
        assert_eq!(
            hidden.observe(start, NativeItemState::Hidden),
            RepairAction::Wait
        );
        assert_eq!(
            hidden.observe(start + Duration::from_secs(1), NativeItemState::Hidden),
            RepairAction::Rebuild
        );
        assert_eq!(hidden.rebuilds_remaining, STATUS_ITEM_REBUILDS - 1);
        assert_eq!(
            hidden.observe(start + STATUS_ITEM_RECOVERY_WINDOW, NativeItemState::Hidden),
            RepairAction::FallBack
        );

        let mut detached = startup_repair(start);
        for seconds in [1, 4, 7] {
            assert_eq!(
                detached.observe(
                    start + Duration::from_secs(seconds),
                    NativeItemState::Detached { visible: true }
                ),
                RepairAction::Rebuild
            );
        }
        assert_eq!(detached.rebuilds_remaining, 0);
        assert_eq!(
            detached.observe(
                start + Duration::from_secs(9),
                NativeItemState::Detached { visible: true }
            ),
            RepairAction::Wait
        );
        assert_eq!(
            detached.observe(
                start + STATUS_ITEM_RECOVERY_WINDOW,
                NativeItemState::Detached { visible: true }
            ),
            RepairAction::FallBack
        );

        let mut detached_hidden = startup_repair(start);
        for seconds in [1, 4, 7] {
            assert_eq!(
                detached_hidden.observe(
                    start + Duration::from_secs(seconds),
                    NativeItemState::Detached { visible: false }
                ),
                RepairAction::Rebuild
            );
        }
        assert_eq!(detached_hidden.rebuilds_remaining, 0);
        assert_eq!(
            detached_hidden.observe(
                start + STATUS_ITEM_RECOVERY_WINDOW,
                NativeItemState::Detached { visible: false }
            ),
            RepairAction::FallBack
        );
    }

    #[test]
    fn menu_actions_use_the_backend_event_channel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        *SENDER.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(EventSender::new(sender));
        emit(BackendEvent::ReloadConfig);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::ReloadConfig
        ));
        emit(BackendEvent::OpenConfigSimulator);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::OpenConfigSimulator
        ));
        emit(BackendEvent::CheckForUpdates);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::CheckForUpdates
        ));
        *SENDER.get().unwrap().lock().unwrap() = None;
    }
}
