//! Strict staging and executable metadata validation for Windows updates.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use semver::Version;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::{PCWSTR, w};

const MAX_EXECUTABLE_BYTES: u64 = 96 * 1024 * 1024;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";
const DOS_SIGNATURE: &[u8; 2] = b"MZ";
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
const FIXED_INFO_SIGNATURE: u32 = 0xFEEF_04BD;

pub(super) fn stage(source: &Path, destination: &Path) -> Result<(), String> {
    let mut input = File::open(source).map_err(|error| {
        format!(
            "cannot open downloaded update {}: {error}",
            source.display()
        )
    })?;
    let expected = input
        .metadata()
        .map_err(|error| format!("cannot inspect downloaded update size: {error}"))?
        .len();
    if expected == 0 || expected > MAX_EXECUTABLE_BYTES {
        return Err(format!(
            "downloaded update has an invalid size: {expected} bytes"
        ));
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            format!(
                "cannot stage update beside the installed executable at {}: {error}",
                destination.display()
            )
        })?;
    let copy_result = std::io::copy(
        &mut Read::by_ref(&mut input).take(MAX_EXECUTABLE_BYTES + 1),
        &mut output,
    )
    .map_err(|error| format!("cannot stage update executable: {error}"));
    let copied = match copy_result {
        Ok(copied) => copied,
        Err(error) => {
            drop(output);
            let _ = fs::remove_file(destination);
            return Err(error);
        }
    };
    if copied != expected {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(format!(
            "staged executable length mismatch: expected {expected}, wrote {copied}"
        ));
    }
    output
        .flush()
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("cannot persist staged update executable: {error}"))
        .inspect_err(|_| {
            let _ = fs::remove_file(destination);
        })
}

pub(super) fn validate_machine(path: &Path) -> Result<(), String> {
    let actual = pe_machine(path)?;
    let expected = if cfg!(target_arch = "x86_64") {
        IMAGE_FILE_MACHINE_AMD64
    } else if cfg!(target_arch = "aarch64") {
        IMAGE_FILE_MACHINE_ARM64
    } else {
        return Err("automatic updates are unsupported on this Windows architecture".into());
    };
    if actual != expected {
        return Err(format!(
            "update executable has machine type 0x{actual:04X}; expected 0x{expected:04X}"
        ));
    }
    Ok(())
}

pub(super) fn validate_version(path: &Path, expected: &Version) -> Result<(), String> {
    let actual = file_version(path)?;
    let expected = (
        u16::try_from(expected.major).map_err(|_| "release major version is too large")?,
        u16::try_from(expected.minor).map_err(|_| "release minor version is too large")?,
        u16::try_from(expected.patch).map_err(|_| "release patch version is too large")?,
        0,
    );
    if actual != expected {
        return Err(format!(
            "update executable version {}.{}.{}.{} does not match release {}.{}.{}.{}",
            actual.0, actual.1, actual.2, actual.3, expected.0, expected.1, expected.2, expected.3
        ));
    }
    Ok(())
}

fn pe_machine(path: &Path) -> Result<u16, String> {
    let mut file = File::open(path).map_err(|error| {
        format!(
            "cannot inspect update executable {}: {error}",
            path.display()
        )
    })?;
    let mut dos = [0_u8; 64];
    file.read_exact(&mut dos)
        .map_err(|error| format!("update executable has a truncated DOS header: {error}"))?;
    if &dos[..2] != DOS_SIGNATURE {
        return Err("update executable does not have an MZ header".into());
    }
    let pe_offset = u32::from_le_bytes(dos[0x3c..0x40].try_into().map_err(|_| "bad PE offset")?);
    if pe_offset < 64
        || u64::from(pe_offset)
            > file
                .metadata()
                .map_err(|e| e.to_string())?
                .len()
                .saturating_sub(6)
    {
        return Err("update executable has an invalid PE header offset".into());
    }
    file.seek(SeekFrom::Start(u64::from(pe_offset)))
        .map_err(|error| format!("cannot seek to update PE header: {error}"))?;
    let mut header = [0_u8; 6];
    file.read_exact(&mut header)
        .map_err(|error| format!("update executable has a truncated PE header: {error}"))?;
    if &header[..4] != PE_SIGNATURE {
        return Err("update executable does not have a PE signature".into());
    }
    Ok(u16::from_le_bytes([header[4], header[5]]))
}

fn file_version(path: &Path) -> Result<(u16, u16, u16, u16), String> {
    use std::os::windows::ffi::OsStrExt;

    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: path is NUL-terminated and remains live through the call.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None) };
    if size == 0 {
        return Err("update executable has no Windows version resource".into());
    }
    let mut data = vec![0_u8; size as usize];
    // SAFETY: the writable buffer is exactly `size` bytes and path is live.
    unsafe { GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, data.as_mut_ptr().cast()) }
        .map_err(|error| format!("cannot read update executable version: {error}"))?;
    let mut value = std::ptr::null_mut();
    let mut value_len = 0_u32;
    // SAFETY: `data` contains the version resource returned above. The root
    // query returns a pointer into that buffer, used before it is dropped.
    let found =
        unsafe { VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut value, &mut value_len) };
    if !found.as_bool()
        || value.is_null()
        || value_len < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
    {
        return Err("update executable has an invalid Windows version resource".into());
    }
    // SAFETY: size was checked and Windows aligns this fixed structure.
    let fixed = unsafe { &*(value.cast::<VS_FIXEDFILEINFO>()) };
    if fixed.dwSignature != FIXED_INFO_SIGNATURE {
        return Err("update executable has an invalid fixed version signature".into());
    }
    Ok((
        (fixed.dwFileVersionMS >> 16) as u16,
        fixed.dwFileVersionMS as u16,
        (fixed.dwFileVersionLS >> 16) as u16,
        fixed.dwFileVersionLS as u16,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_machine_constants_match_supported_targets() {
        assert_eq!(IMAGE_FILE_MACHINE_AMD64, 0x8664);
        assert_eq!(IMAGE_FILE_MACHINE_ARM64, 0xaa64);
    }

    #[test]
    fn rejects_a_non_pe_file() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "keysteer-not-pe-{}-{nonce}.bin",
            std::process::id(),
        ));
        assert!(std::fs::write(&path, [0_u8; 64]).is_ok());
        let result = pe_machine(&path);
        let _ = std::fs::remove_file(path);
        assert!(result.is_err());
    }
}
