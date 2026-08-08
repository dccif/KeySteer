//! Native menu-bar controls.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSControlStateValueOff,
    NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

use crate::api::Autostart;
use crate::api::backend::{BackendEvent, UpdateCheckResult};

use super::EventSender;

static SENDER: OnceLock<Mutex<Option<EventSender>>> = OnceLock::new();
const STATUS_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/keysteer-icon.png");
const STATUS_ICON_SIZE: f64 = 18.0;

struct StatusTargetIvars {
    update_alert: RefCell<Option<Retained<NSAlert>>>,
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

        #[unsafe(method(toggleAutostart:))]
        fn toggle_autostart(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::ToggleAutostart);
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: Option<&AnyObject>) {
            emit(BackendEvent::CheckForUpdates);
        }

        #[unsafe(method(dismissUpdateAlert:))]
        fn dismiss_update_alert_action(&self, _sender: Option<&AnyObject>) {
            self.dismiss_update_alert();
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
        });
        // SAFETY: The NSObject superclass is initialized after Rust ivars.
        unsafe { msg_send![super(this), init] }
    }

    fn show_update_alert(&self, alert: Retained<NSAlert>) {
        self.dismiss_update_alert();
        let window = alert.window();
        *self.ivars().update_alert.borrow_mut() = Some(alert);
        window.center();
        NSApplication::sharedApplication(self.mtm()).activate();
        window.makeKeyAndOrderFront(None);
    }

    fn dismiss_update_alert(&self) {
        if let Some(alert) = self.ivars().update_alert.borrow_mut().take() {
            alert.window().close();
        }
    }
}

pub struct StatusItem {
    item: Retained<NSStatusItem>,
    _target: Retained<StatusTarget>,
    toggle_item: Retained<NSMenuItem>,
    autostart_item: Retained<NSMenuItem>,
    enabled: bool,
}

impl StatusItem {
    pub(super) fn new(mtm: MainThreadMarker, sender: EventSender) -> Self {
        *SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(sender);

        let application = NSApplication::sharedApplication(mtm);
        application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        application.finishLaunching();

        let target = StatusTarget::new(mtm);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let toggle_item = menu_item(mtm, "Pause", sel!(toggleEnabled:), &target);
        let reload_item = menu_item(mtm, "Reload Configuration", sel!(reloadConfig:), &target);
        let autostart_item = menu_item(mtm, "Start at Login", sel!(toggleAutostart:), &target);
        let update_item = menu_item(mtm, "Check for Updates...", sel!(checkForUpdates:), &target);
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
        menu.addItem(&autostart_item);
        menu.addItem(&update_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&quit_item);

        let status_bar = NSStatusBar::systemStatusBar();
        let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            if let Some(image) = status_icon() {
                button.setImage(Some(&image));
                button.setTitle(&NSString::from_str(""));
            } else {
                let symbol = NSString::from_str("cursorarrow.motionlines");
                let description = NSString::from_str("KeySteer");
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &symbol,
                    Some(&description),
                ) {
                    button.setImage(Some(&image));
                    button.setTitle(&NSString::from_str(""));
                } else {
                    button.setTitle(&NSString::from_str("KeySteer"));
                }
            }
        }
        item.setMenu(Some(&menu));

        Self {
            item,
            _target: target,
            toggle_item,
            autostart_item,
            enabled: true,
        }
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

    pub(super) fn present_update_result(
        &mut self,
        result: &UpdateCheckResult,
    ) -> Result<(), String> {
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
            ),
            UpdateCheckResult::UpToDate { current } => self.show_alert(
                "KeySteer is up to date",
                &format!("KeySteer {current} is already the latest version."),
            ),
            UpdateCheckResult::Failed(error) => {
                self.show_alert("Could not check for updates", error)
            }
        }
    }

    fn show_alert(&mut self, title: &str, details: &str) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            "update result must be presented on the macOS main thread".to_string()
        })?;
        autoreleasepool(|_| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(title));
            alert.setInformativeText(&NSString::from_str(details));
            let button = alert.addButtonWithTitle(&NSString::from_str("OK"));
            // NSAlert only wires its buttons while using runModal or sheet APIs.
            // KeySteer stays non-modal so the engine loop can keep servicing the
            // keyboard hook; explicitly close and release the retained alert.
            unsafe {
                button.setTarget(Some(&self._target));
                button.setAction(Some(sel!(dismissUpdateAlert:)));
            }
            self._target.show_update_alert(alert);
        });
        Ok(())
    }
}

fn status_icon() -> Option<Retained<NSImage>> {
    let data = unsafe {
        NSData::dataWithBytes_length(STATUS_ICON_PNG.as_ptr().cast(), STATUS_ICON_PNG.len())
    };
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(STATUS_ICON_SIZE, STATUS_ICON_SIZE));
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

    #[test]
    fn menu_actions_use_the_backend_event_channel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        *SENDER.get_or_init(|| Mutex::new(None)).lock().unwrap() =
            Some(EventSender::Channel(sender));
        emit(BackendEvent::ReloadConfig);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::ReloadConfig
        ));
        emit(BackendEvent::CheckForUpdates);
        assert!(matches!(
            receiver.recv().unwrap(),
            BackendEvent::CheckForUpdates
        ));
        *SENDER.get().unwrap().lock().unwrap() = None;
    }
}
