//! Current-user startup registration through `SMAppService.mainAppService`.
//!
//! Registering `/usr/bin/open KeySteer.app` as a LaunchAgent makes macOS show
//! the login item as “open”. Registering the main application through Service
//! Management preserves the bundle identity, display name and icon.

use std::ffi::{CStr, c_char};
use std::io::ErrorKind;
use std::path::PathBuf;

use crate::api::Autostart;

const LEGACY_AGENT_FILE: &str = "com.keysteer.app.plist";

unsafe extern "C" {
    fn NmkMainAppLoginItemIsRegistered() -> bool;
    fn NmkSetMainAppLoginItemEnabled(enabled: bool) -> *mut c_char;
    fn NmkFreeNativeString(value: *mut c_char);
}

pub(super) struct MacosAutostart;

impl MacosAutostart {
    pub(super) fn new() -> Self {
        Self
    }

    fn current_bundle() -> Result<Option<PathBuf>, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot determine the KeySteer executable: {error}"))?;
        Ok(super::app_bundle_for_executable(&executable))
    }

    fn legacy_agent_path() -> Result<PathBuf, String> {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| "cannot determine the macOS home directory".to_string())?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(LEGACY_AGENT_FILE))
    }

    fn legacy_agent_exists() -> Result<bool, String> {
        let path = Self::legacy_agent_path()?;
        match std::fs::metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "cannot inspect the legacy macOS startup agent {}: {error}",
                path.display()
            )),
        }
    }

    fn remove_legacy_agent() -> Result<(), String> {
        let path = Self::legacy_agent_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "cannot remove the legacy macOS startup agent {}: {error}",
                path.display()
            )),
        }
    }

    fn native_is_registered() -> bool {
        unsafe { NmkMainAppLoginItemIsRegistered() }
    }

    fn native_set_enabled(enabled: bool) -> Result<(), String> {
        let error = unsafe { NmkSetMainAppLoginItemEnabled(enabled) };
        if error.is_null() {
            return Ok(());
        }
        let message = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        unsafe { NmkFreeNativeString(error) };
        Err(format!("cannot update the macOS login item: {message}"))
    }
}

impl Autostart for MacosAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        if Self::current_bundle()?.is_none() {
            return Ok(false);
        }
        if Self::native_is_registered() {
            Self::remove_legacy_agent()?;
            return Ok(true);
        }

        // Transparently replace the old `/usr/bin/open` LaunchAgent the first
        // time an upgraded packaged app reads its login-item state.
        if Self::legacy_agent_exists()? {
            Self::native_set_enabled(true)?;
            Self::remove_legacy_agent()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let packaged = Self::current_bundle()?.is_some();
        if enabled && !packaged {
            return Err(
                "Start at Login requires launching the packaged KeySteer.app, not a bare binary"
                    .into(),
            );
        }

        if packaged {
            Self::native_set_enabled(enabled)?;
        }
        Self::remove_legacy_agent()
    }
}
