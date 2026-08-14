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
    // SAFETY: isatty only reads a file descriptor number.
    let is_tty = unsafe { libc::isatty(libc::STDERR_FILENO) } == 1;
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
    fn instructions_name_a_concrete_next_step() {
        let text = instructions();
        assert!(text.contains("Privacy & Security > Accessibility"));
        // Must always end with something the user can act on.
        assert!(text.contains("restart") || text.contains("reopen"));
    }

    #[test]
    fn instructions_mention_the_stale_signature_case() {
        // A rebuild silently invalidates the grant; users get stuck here.
        assert!(instructions().contains("toggle it off and on"));
    }

    #[test]
    fn instructions_carry_no_escape_codes_when_redirected() {
        // Test output is not a terminal, so the text must be plain.
        assert!(
            !instructions().contains('\x1b'),
            "escape codes would corrupt a log file"
        );
    }

    #[test]
    fn trust_check_does_not_panic() {
        // Whatever the answer, asking must be safe.
        let _ = is_trusted();
    }
}
