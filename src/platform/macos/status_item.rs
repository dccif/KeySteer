//! Native top-status-item controls.

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
    NSCellImagePosition, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage,
    NSImageView, NSMenu, NSMenuItem, NSPanel, NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
    NSTextField, NSView, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::api::Autostart;
use crate::api::backend::{BackendEvent, UpdateCheckResult, UpdateProgress};

use super::EventSender;

static SENDER: OnceLock<Mutex<Option<EventSender>>> = OnceLock::new();

const STATUS_ICON_ATTACH_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const STATUS_ICON_ATTACH_RETRY_ATTEMPTS: u8 = 120;
const STATUS_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/keysteer-icon.png");
const STATUS_ICON_SIZE: f64 = 18.0;

struct StatusTargetIvars {
    note: RefCell<Option<NotePanel>>,
    update_alert: RefCell<Option<Retained<NSPanel>>>,
    downloaded_update: RefCell<Option<PathBuf>>,
}

struct NotePanel {
    id: u64,
    panel: Retained<NSPanel>,
    field: Retained<NSTextField>,
}

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "KeySteerInlineInputPanel"]
    struct InlineInputPanel;
    impl InlineInputPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool { true }
    }
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KeySteerStatusTarget"]
    #[ivars = StatusTargetIvars]
    struct StatusTarget;

    impl StatusTarget {
        #[unsafe(method(saveLayoutNote:))]
        fn save_layout_note(&self, _sender: Option<&AnyObject>) { self.finish_note(true); }
        #[unsafe(method(cancelLayoutNote:))]
        fn cancel_layout_note(&self, _sender: Option<&AnyObject>) { self.finish_note(false); }
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
                &crate::platform::common::app_info::details(),
                PanelAction::OpenRepository,
            ) {
                crate::support::logging::report_error("macos-about", error);
            }
        }

        #[unsafe(method(openRepository:))]
        fn open_repository(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = open_https_url(crate::platform::common::app_info::REPOSITORY_URL) {
                crate::support::logging::report_error("macos-about", error);
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
                crate::support::logging::report_error(
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
    fn finish_note(&self, save: bool) {
        let note = self.ivars().note.borrow_mut().take();
        if let Some(note) = note {
            let value = save.then(|| note.field.stringValue().to_string());
            note.panel.close();
            emit(BackendEvent::TextPromptResult {
                id: note.id,
                value: Ok(value),
            });
        }
    }
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Allocated<Self> = mtm.alloc();
        let this = this.set_ivars(StatusTargetIvars {
            note: RefCell::new(None),
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
    _menu: Retained<NSMenu>,
    _icon: Option<Retained<NSImage>>,
    _target: Retained<StatusTarget>,
    toggle_item: Retained<NSMenuItem>,
    autostart_item: Retained<NSMenuItem>,
    update_item: Retained<NSMenuItem>,
    enabled: bool,
    icon_attach_retry: Option<IconAttachRetry>,
}

struct IconAttachRetry {
    next_attempt: Instant,
    attempts_remaining: u8,
}

/// Finish AppKit startup on the main thread before creating the top status
/// item. The backend owns subsequent event dispatch, so the portable runtime
/// remains unaware of NSApplication.
pub(super) fn prepare_application(mtm: MainThreadMarker) -> Result<(), String> {
    let application = NSApplication::sharedApplication(mtm);
    if !application.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
        && application.activationPolicy() != NSApplicationActivationPolicy::Accessory
    {
        return Err("AppKit rejected the accessory activation policy".into());
    }
    application.finishLaunching();
    Ok(())
}

impl StatusItem {
    pub(super) fn cancel_text_prompt(&self, id: u64) {
        if self
            ._target
            .ivars()
            .note
            .borrow()
            .as_ref()
            .is_some_and(|n| n.id == id)
        {
            self._target.finish_note(false);
        }
    }
    pub(super) fn request_text_prompt(
        &self,
        request: crate::api::window_presets::TextPrompt,
    ) -> Result<(), String> {
        let target = &self._target;
        target.finish_note(false);
        let mtm = target.mtm();
        let screens = super::screens::list_screens()?;
        let primary = crate::api::Screen::primary(&screens).ok_or("No display for layout input")?;
        let bounds = request.bounds;
        let rect = NSRect::new(
            NSPoint::new(bounds.x, primary.bounds.bottom() - bounds.bottom()),
            NSSize::new(bounds.width, bounds.height),
        );
        let allocated = InlineInputPanel::alloc(mtm).set_ivars(());
        // SAFETY: initializes this retained NSPanel subclass on the AppKit thread.
        let panel: Retained<InlineInputPanel> = unsafe {
            msg_send![super(allocated),
            initWithContentRect: rect, styleMask: NSWindowStyleMask::Borderless,
            backing: NSBackingStoreType::Buffered, defer: false]
        };
        if panel.isReleasedWhenClosed() {
            return Err("Note panel has ambiguous ownership".into());
        }
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setTitle(&NSString::from_str(&request.title));
        panel.setLevel(26);
        panel.setHasShadow(false);
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), rect.size),
        );
        let label =
            NSTextField::wrappingLabelWithString(&NSString::from_str(&request.message), mtm);
        label.setFrame(NSRect::new(
            NSPoint::new(12.0, 62.0),
            NSSize::new(bounds.width - 24.0, 28.0),
        ));
        content.addSubview(&label);
        let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        field.setPlaceholderString(Some(&NSString::from_str(&request.placeholder)));
        field.setFrame(NSRect::new(
            NSPoint::new(12.0, 20.0),
            NSSize::new((bounds.width - 204.0).max(40.0), 30.0),
        ));
        content.addSubview(&field);
        for (title, selector, x, key) in [
            ("Save", sel!(saveLayoutNote:), bounds.width - 180.0, "\r"),
            (
                "Cancel",
                sel!(cancelLayoutNote:),
                bounds.width - 90.0,
                "\u{1b}",
            ),
        ] {
            // SAFETY: selectors are implemented above and the retained status target
            // outlives all buttons. AppKit retains no temporary Rust references.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&**target),
                    Some(selector),
                    mtm,
                )
            };
            button.setFrame(NSRect::new(NSPoint::new(x, 20.0), NSSize::new(86.0, 30.0)));
            button.setKeyEquivalent(&NSString::from_str(key));
            content.addSubview(&button);
        }
        panel.setContentView(Some(&content));
        *target.ivars().note.borrow_mut() = Some(NotePanel {
            id: request.id,
            panel: Retained::into_super(panel.clone()),
            field: field.clone(),
        });
        NSApplication::sharedApplication(mtm).activate();
        panel.makeKeyAndOrderFront(None);
        panel.makeFirstResponder(Some(&field));
        Ok(())
    }
    pub(super) fn new(mtm: MainThreadMarker, sender: EventSender) -> Self {
        *SENDER
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(sender);

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
                crate::support::logging::report_error("macos-autostart", error);
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
        let (item, icon_attached) = create_native_item(mtm, &menu, icon.as_deref());

        Self {
            item,
            _menu: menu,
            _icon: icon,
            _target: target,
            toggle_item,
            autostart_item,
            update_item,
            enabled: true,
            icon_attach_retry: (!icon_attached).then(|| IconAttachRetry {
                next_attempt: Instant::now(),
                attempts_remaining: STATUS_ICON_ATTACH_RETRY_ATTEMPTS,
            }),
        }
    }

    /// Complete icon attachment when a cold login creates the status item
    /// before its native button is hosted. This reuses Backend::poll and never
    /// creates a timer, thread, item replacement, or visibility loop.
    pub(super) fn maintain_icon_attachment(&mut self) {
        let Some(retry) = self.icon_attach_retry.as_mut() else {
            return;
        };
        let now = Instant::now();
        if now < retry.next_attempt {
            return;
        }
        if let Some(button) = self.item.button(self._target.mtm()) {
            configure_status_button(&button, self._icon.as_deref());
            self.icon_attach_retry = None;
            return;
        }
        retry.attempts_remaining = retry.attempts_remaining.saturating_sub(1);
        if retry.attempts_remaining == 0 {
            self.icon_attach_retry = None;
            crate::support::logging::report_error(
                "macos-status-item",
                "AppKit did not attach a button to the top status item after login",
            );
        } else {
            retry.next_attempt = now + STATUS_ICON_ATTACH_RETRY_INTERVAL;
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
        let action = downloaded_update
            .map(PanelAction::RevealInFinder)
            .unwrap_or(PanelAction::None);
        show_panel(mtm, &self._target, title, details, action)
    }
}

#[derive(Clone, Copy)]
enum PanelAction<'a> {
    None,
    OpenRepository,
    RevealInFinder(&'a Path),
}

fn show_panel(
    mtm: MainThreadMarker,
    target: &StatusTarget,
    title: &str,
    details: &str,
    action: PanelAction<'_>,
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
        let (button, secondary_button) = unsafe {
            let button = NSButton::buttonWithTitle_target_action(
                &NSString::from_str("OK"),
                Some(target),
                Some(sel!(dismissUpdateAlert:)),
                mtm,
            );
            let secondary_button = match action {
                PanelAction::None => None,
                PanelAction::OpenRepository => Some(NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("KeySteer"),
                    Some(target),
                    Some(sel!(openRepository:)),
                    mtm,
                )),
                PanelAction::RevealInFinder(_) => Some(NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Show in Finder"),
                    Some(target),
                    Some(sel!(showDownloadedUpdate:)),
                    mtm,
                )),
            };
            (button, secondary_button)
        };
        button.setFrame(NSRect::new(
            NSPoint::new(412.0, 16.0),
            NSSize::new(80.0, 32.0),
        ));
        button.setKeyEquivalent(&NSString::from_str("\r"));
        content.addSubview(&button);

        if let Some(secondary_button) = secondary_button {
            secondary_button.setFrame(NSRect::new(
                NSPoint::new(276.0, 16.0),
                NSSize::new(124.0, 32.0),
            ));
            content.addSubview(&secondary_button);
        }

        panel.setContentView(Some(&content));
        let downloaded_update = match action {
            PanelAction::RevealInFinder(path) => Some(path.to_path_buf()),
            PanelAction::None | PanelAction::OpenRepository => None,
        };
        target.show_update_alert(panel, downloaded_update);
        Ok(())
    })
}

pub(super) fn open_https_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") || url.contains('\0') {
        return Err("macOS refused an invalid HTTPS URL".into());
    }
    let text = NSString::from_str(url);
    let url = NSURL::URLWithString(&text)
        .ok_or_else(|| "macOS could not parse the HTTPS URL".to_string())?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err("macOS could not open the default browser".into())
    }
}

fn create_native_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    icon: Option<&NSImage>,
) -> (Retained<NSStatusItem>, bool) {
    let status_bar = NSStatusBar::systemStatusBar();
    let item = status_bar.statusItemWithLength(NSSquareStatusItemLength);
    // This is the status item's pop-up control menu, not NSApplication's main
    // menu. AppKit presents it when the user clicks the top status icon.
    item.setMenu(Some(menu));
    item.setVisible(true);
    let icon_attached = if let Some(button) = item.button(mtm) {
        configure_status_button(&button, icon);
        true
    } else {
        false
    };
    (item, icon_attached)
}

fn configure_status_button(button: &objc2_app_kit::NSStatusBarButton, icon: Option<&NSImage>) {
    button.setImagePosition(NSCellImagePosition::ImageOnly);
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
    let data = NSData::with_bytes(STATUS_ICON_PNG);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(size, size));
    image.setTemplate(false);
    image.setAccessibilityDescription(Some(&NSString::from_str("KeySteer")));
    Some(image)
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        self._target.finish_note(false);
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
