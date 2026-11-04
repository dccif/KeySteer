//! Accessibility permission on macOS.
//!
//! Without it `CGEventTap` cannot be created, so no keystroke is ever seen and
//! no mode can be entered. That is the single most common reason the program
//! appears to do nothing, so it is detected explicitly and reported with the
//! exact steps to fix it rather than being inferred from a tap failure.

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2_core_foundation::{
    CFDictionary as NativeDictionary, CFNotificationCenter, CFNotificationSuspensionBehavior,
    CFRetained, CFString as NativeString,
};

/// Best-effort, event-driven fallback for macOS releases that revoke access
/// without disabling/invalidation callbacks on an existing event tap. These
/// notification names are undocumented; never treat their absence as a grant.
pub(super) struct ChangeObserver {
    centers: Vec<CFRetained<CFNotificationCenter>>,
    identity: Box<u8>,
}

extern "C-unwind" fn permission_list_changed(
    _center: *mut CFNotificationCenter,
    _observer: *mut c_void,
    _name: *const NativeString,
    _object: *const c_void,
    _info: *const NativeDictionary,
) {
    // Notifications describe any app/service, not necessarily KeySteer. Only
    // queue work here: no TCC/WindowServer query on the main thread.
    super::input::hook::permission_list_changed();
}

impl ChangeObserver {
    pub(super) fn new() -> Self {
        let mut observer = Self {
            centers: Vec::new(),
            identity: Box::new(0),
        };
        for (center, name) in [
            (
                CFNotificationCenter::distributed_center(),
                "com.apple.accessibility.api",
            ),
            (
                CFNotificationCenter::darwin_notify_center(),
                "com.apple.tcc.access.changed",
            ),
        ] {
            if let Some(center) = center {
                // SAFETY: the stable Box is only an identity, never dereferenced
                // by the callback. The exact ABI ignores all notification data;
                // Drop removes both registrations before freeing the identity.
                unsafe {
                    center.add_observer(
                        std::ptr::from_ref(observer.identity.as_ref()).cast(),
                        Some(permission_list_changed),
                        Some(&NativeString::from_str(name)),
                        std::ptr::null(),
                        CFNotificationSuspensionBehavior::DeliverImmediately,
                    );
                }
                observer.centers.push(center);
            }
        }
        observer
    }
}

impl Drop for ChangeObserver {
    fn drop(&mut self) {
        for center in &self.centers {
            // SAFETY: same retained centers and still-live identity registered
            // above. Late callbacks contain no pointer to application state.
            unsafe {
                center.remove_every_observer(std::ptr::from_ref(self.identity.as_ref()).cast());
            }
        }
    }
}

// AX* lives in ApplicationServices, which is not pulled in by core-graphics.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

/// Whether this process may observe and inject keyboard events.
pub fn is_trusted() -> bool {
    // SAFETY: no arguments, and the call has no side effects.
    unsafe { AXIsProcessTrusted() }
}

/// Ask the system to show the "grant Accessibility access" dialog.
///
/// Returns the trust state. The dialog only appears once per app per boot, and
/// never for a process launched from a terminal that is itself untrusted, so a
/// `false` return still needs the printed instructions below.
pub fn prompt_for_trust() -> bool {
    // SAFETY: the framework constant is a process-lifetime CFString borrowed
    // under the get rule; the wrapper does not consume it.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt as *const _) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    // SAFETY: `options` is a valid CFDictionary and outlives the call.
    unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef().cast()) }
}

/// Where the running executable lives, to tell the user what to add.
fn executable_label() -> String {
    match std::env::current_exe() {
        Ok(path) => super::app_bundle_for_executable(&path)
            .unwrap_or(path)
            .display()
            .to_string(),
        Err(_) => "the KeySteer binary".to_string(),
    }
}

/// The name of the app the user must actually grant, which is *not* always this
/// binary: a program started from a terminal inherits that terminal's trust.
fn responsible_app() -> Option<&'static str> {
    // TERM_PROGRAM is set by the terminal that owns this session.
    let term = std::env::var("TERM_PROGRAM").ok()?;
    Some(match term.as_str() {
        "Apple_Terminal" => "Terminal",
        "iTerm.app" => "iTerm",
        "vscode" => "Visual Studio Code",
        "WarpTerminal" => "Warp",
        "ghostty" => "Ghostty",
        "Alacritty" => "Alacritty",
        "kitty" => "kitty",
        "WezTerm" => "WezTerm",
        _ => return None,
    })
}

/// Wrap `text` in bold, but only when stderr is a terminal.
///
/// Emitting escapes into a pipe or a log file would corrupt it.
fn bold(text: &str) -> String {
    let is_tty = crate::support::logging::emergency_stderr_is_terminal();
    if is_tty {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// A complete, copy-pasteable explanation of how to grant permission.
///
/// Written out in full because the usual one-line version sends people to the
/// wrong entry: when launched from a terminal, macOS attributes the request to
/// the *terminal*, and adding the binary itself changes nothing.
pub fn instructions() -> String {
    let mut text = String::new();
    text.push_str(
        "Accessibility permission is required to read the keyboard.\n\
         Without it no mode can be entered, so the program cannot do anything.\n\n\
         To grant it:\n\
         \x20 1. Open System Settings > Privacy & Security > Accessibility\n",
    );

    match responsible_app() {
        Some(app) => {
            text.push_str(&format!(
                "\x20 2. Enable {} in the list.\n\
                 \x20    (You are running from {app}, and macOS grants keyboard\n\
                 \x20     access to the launching app, not to the binary. Adding\n\
                 \x20     the binary itself will not work.)\n\
                 \x20 3. Quit {app} completely and reopen it, then run this again.\n",
                bold(app)
            ));
        }
        None => {
            text.push_str(&format!(
                "\x20 2. Add and enable:\n\x20    {}\n\
                 \x20 3. Quit and restart the program.\n",
                executable_label()
            ));
        }
    }

    text.push_str(
        "\nIf it is already enabled, toggle it off and on again: the permission\n\
         is tied to the binary's signature and goes stale after a rebuild.\n\n\
         Pointer control and overlays work without it; only the keyboard does not.",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_are_plain_and_explain_accessibility_recovery() {
        let text = instructions();
        assert!(text.contains("Privacy & Security > Accessibility"));
        // Must always end with something the user can act on.
        assert!(text.contains("restart") || text.contains("reopen"));
        // A rebuild silently invalidates the grant; users get stuck here.
        assert!(text.contains("toggle it off and on"));
        // Test output is not a terminal, so the text must be plain.
        assert!(
            !text.contains('\x1b'),
            "escape codes would corrupt a log file"
        );
    }

    #[test]
    fn trust_check_does_not_panic() {
        // Whatever the answer, asking must be safe.
        let _ = is_trusted();
    }
}
