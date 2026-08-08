//! Current-user startup registration through the native Windows Run key.

use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::w;

use crate::api::Autostart;

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: windows::core::PCWSTR = w!("KeySteer");

pub(super) struct WindowsAutostart;

impl WindowsAutostart {
    pub(super) fn new() -> Self {
        Self
    }

    fn command() -> Result<Vec<u16>, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot determine the KeySteer executable: {error}"))?;
        let mut command = Vec::with_capacity(executable.as_os_str().len() + 3);
        command.push(b'"' as u16);
        command.extend(executable.as_os_str().encode_wide());
        command.push(b'"' as u16);
        command.push(0);
        Ok(command)
    }

    fn registered_command() -> Result<Option<Vec<u16>>, String> {
        let mut byte_count = 0u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE_NAME,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut byte_count),
            )
        };
        if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
            return Ok(None);
        }
        result
            .ok()
            .map_err(|error| format!("cannot read the Windows startup entry: {error}"))?;

        let mut command = vec![0u16; (byte_count as usize).div_ceil(2)];
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE_NAME,
                RRF_RT_REG_SZ,
                None,
                Some(command.as_mut_ptr().cast()),
                Some(&mut byte_count),
            )
        };
        result
            .ok()
            .map_err(|error| format!("cannot read the Windows startup entry: {error}"))?;
        command.truncate((byte_count as usize).div_ceil(2));
        while command.last() == Some(&0) {
            command.pop();
        }
        Ok(Some(command))
    }
}

impl Autostart for WindowsAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        let Some(registered) = Self::registered_command()? else {
            return Ok(false);
        };
        let mut current = Self::command()?;
        current.pop();
        Ok(registered == current)
    }

    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            let command = Self::command()?;
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    VALUE_NAME,
                    REG_SZ.0,
                    Some(command.as_ptr().cast()),
                    (command.len() * size_of::<u16>()) as u32,
                )
            }
            .ok()
            .map_err(|error| format!("cannot enable Windows startup: {error}"))
        } else {
            let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) };
            if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
                Ok(())
            } else {
                result
                    .ok()
                    .map_err(|error| format!("cannot disable Windows startup: {error}"))
            }
        }
    }
}
