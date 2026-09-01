//! Strict staging and executable metadata validation for Windows updates.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use miniz_oxide::inflate::decompress_to_vec_with_limit;
use semver::Version;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::{PCWSTR, w};

const MAX_EXECUTABLE_BYTES: u64 = 96 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 10 * 1024 * 1024;
const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_FILE_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_SENTINEL_U16: u16 = u16::MAX;
const ZIP64_SENTINEL_U32: u32 = u32::MAX;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
const ENCRYPTED_FLAGS: u16 = 0x0041;
const DATA_DESCRIPTOR_FLAG: u16 = 0x0008;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";
const DOS_SIGNATURE: &[u8; 2] = b"MZ";
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
const FIXED_INFO_SIGNATURE: u32 = 0xFEEF_04BD;

struct ArchiveEntry<'a> {
    name: &'a [u8],
    flags: u16,
    compression: u16,
    crc32: u32,
    compressed_size: usize,
    uncompressed_size: usize,
    local_header_offset: usize,
    central_directory_offset: usize,
}

pub(super) fn extract_archive_executable(archive: &Path, destination: &Path) -> Result<(), String> {
    let mut input = File::open(archive).map_err(|error| {
        format!(
            "cannot open downloaded update {}: {error}",
            archive.display()
        )
    })?;
    let archive_size = input
        .metadata()
        .map_err(|error| format!("cannot inspect downloaded update size: {error}"))?
        .len();
    if archive_size == 0 || archive_size > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "downloaded update archive has an invalid size: {archive_size} bytes"
        ));
    }
    let mut archive_bytes = Vec::with_capacity(archive_size as usize);
    Read::by_ref(&mut input)
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut archive_bytes)
        .map_err(|error| format!("cannot read downloaded update archive: {error}"))?;
    if archive_bytes.len() as u64 != archive_size {
        return Err("downloaded update archive changed while it was being read".into());
    }

    let entry = locate_executable(&archive_bytes)?;
    let compressed = local_file_data(&archive_bytes, &entry)?;
    let executable = match entry.compression {
        STORED => {
            if entry.compressed_size != entry.uncompressed_size {
                return Err("stored update executable has inconsistent ZIP lengths".into());
            }
            compressed.to_vec()
        }
        DEFLATED => decompress_to_vec_with_limit(compressed, entry.uncompressed_size)
            .map_err(|_| "cannot decompress the update executable from the ZIP archive")?,
        _ => return Err("update executable uses an unsupported ZIP compression method".into()),
    };
    if executable.len() != entry.uncompressed_size {
        return Err(format!(
            "extracted executable length mismatch: expected {}, produced {}",
            entry.uncompressed_size,
            executable.len()
        ));
    }
    if crc32(&executable) != entry.crc32 {
        return Err("extracted update executable failed its ZIP CRC-32 check".into());
    }
    persist_extracted(destination, &executable)
}

fn locate_executable(archive: &[u8]) -> Result<ArchiveEntry<'_>, String> {
    let eocd = find_end_of_central_directory(archive)?;
    if read_u16(archive, eocd + 4)? != 0 || read_u16(archive, eocd + 6)? != 0 {
        return Err("multi-disk ZIP updates are unsupported".into());
    }
    let entries_on_disk = read_u16(archive, eocd + 8)?;
    let entry_count = read_u16(archive, eocd + 10)?;
    if entries_on_disk != entry_count || entry_count == 0 {
        return Err("update ZIP has an inconsistent central directory".into());
    }
    if entry_count == ZIP64_SENTINEL_U16 {
        return Err("ZIP64 update archives are unsupported".into());
    }
    let central_size = read_u32(archive, eocd + 12)?;
    let central_offset = read_u32(archive, eocd + 16)?;
    if central_size == ZIP64_SENTINEL_U32 || central_offset == ZIP64_SENTINEL_U32 {
        return Err("ZIP64 update archives are unsupported".into());
    }
    let central_offset = central_offset as usize;
    let central_end = central_offset
        .checked_add(central_size as usize)
        .ok_or_else(|| "update ZIP central directory overflows its archive".to_string())?;
    if central_end != eocd || central_end > archive.len() {
        return Err("update ZIP has an invalid central directory boundary".into());
    }

    let mut cursor = central_offset;
    let mut executable = None;
    for _ in 0..entry_count {
        if read_u32(archive, cursor)? != CENTRAL_FILE_SIGNATURE {
            return Err("update ZIP has an invalid central directory entry".into());
        }
        let name_len = read_u16(archive, cursor + 28)? as usize;
        let extra_len = read_u16(archive, cursor + 30)? as usize;
        let comment_len = read_u16(archive, cursor + 32)? as usize;
        let entry_len = 46_usize
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| "update ZIP entry length overflows its archive".to_string())?;
        let entry_end = cursor
            .checked_add(entry_len)
            .ok_or_else(|| "update ZIP entry boundary overflows its archive".to_string())?;
        if entry_end > central_end {
            return Err("update ZIP central directory entry is truncated".into());
        }
        let name = archive
            .get(cursor + 46..cursor + 46 + name_len)
            .ok_or_else(|| "update ZIP entry name is truncated".to_string())?;
        if is_executable_name(name) {
            if executable.is_some() {
                return Err("update ZIP contains duplicate KeySteer executables".into());
            }
            let flags = read_u16(archive, cursor + 8)?;
            if flags & ENCRYPTED_FLAGS != 0 {
                return Err("encrypted ZIP updates are unsupported".into());
            }
            let compression = read_u16(archive, cursor + 10)?;
            if !matches!(compression, STORED | DEFLATED) {
                return Err("update executable uses an unsupported ZIP compression method".into());
            }
            let compressed_size = read_u32(archive, cursor + 20)?;
            let uncompressed_size = read_u32(archive, cursor + 24)?;
            let local_header_offset = read_u32(archive, cursor + 42)?;
            if compressed_size == ZIP64_SENTINEL_U32
                || uncompressed_size == ZIP64_SENTINEL_U32
                || local_header_offset == ZIP64_SENTINEL_U32
            {
                return Err("ZIP64 update archives are unsupported".into());
            }
            if uncompressed_size == 0 || u64::from(uncompressed_size) > MAX_EXECUTABLE_BYTES {
                return Err(format!(
                    "update executable has an invalid size: {uncompressed_size} bytes"
                ));
            }
            if local_header_offset as usize >= central_offset {
                return Err("update executable local ZIP header has an invalid offset".into());
            }
            executable = Some(ArchiveEntry {
                name,
                flags,
                compression,
                crc32: read_u32(archive, cursor + 16)?,
                compressed_size: compressed_size as usize,
                uncompressed_size: uncompressed_size as usize,
                local_header_offset: local_header_offset as usize,
                central_directory_offset: central_offset,
            });
        }
        cursor = entry_end;
    }
    if cursor != central_end {
        return Err("update ZIP central directory entry count is inconsistent".into());
    }
    executable.ok_or_else(|| "update ZIP does not contain KeySteer/KeySteer.exe".into())
}

fn local_file_data<'a>(archive: &'a [u8], entry: &ArchiveEntry<'_>) -> Result<&'a [u8], String> {
    let offset = entry.local_header_offset;
    if read_u32(archive, offset)? != LOCAL_FILE_SIGNATURE {
        return Err("update executable has an invalid local ZIP header".into());
    }
    let local_flags = read_u16(archive, offset + 6)?;
    let local_compression = read_u16(archive, offset + 8)?;
    if local_flags != entry.flags || local_compression != entry.compression {
        return Err("update executable ZIP headers disagree".into());
    }
    let name_len = read_u16(archive, offset + 26)? as usize;
    let extra_len = read_u16(archive, offset + 28)? as usize;
    let data_start = offset
        .checked_add(30)
        .and_then(|value| value.checked_add(name_len))
        .and_then(|value| value.checked_add(extra_len))
        .ok_or_else(|| "update executable ZIP data offset overflows".to_string())?;
    let local_name = archive
        .get(offset + 30..offset + 30 + name_len)
        .ok_or_else(|| "update executable local ZIP name is truncated".to_string())?;
    if local_name != entry.name {
        return Err("update executable ZIP headers name different files".into());
    }
    if entry.flags & DATA_DESCRIPTOR_FLAG == 0
        && (read_u32(archive, offset + 14)? != entry.crc32
            || read_u32(archive, offset + 18)? as usize != entry.compressed_size
            || read_u32(archive, offset + 22)? as usize != entry.uncompressed_size)
    {
        return Err("update executable ZIP headers contain inconsistent metadata".into());
    }
    let data_end = data_start
        .checked_add(entry.compressed_size)
        .ok_or_else(|| "update executable ZIP data length overflows".to_string())?;
    if data_end > entry.central_directory_offset {
        return Err("compressed update executable overlaps the ZIP central directory".into());
    }
    archive
        .get(data_start..data_end)
        .ok_or_else(|| "compressed update executable is truncated".into())
}

fn find_end_of_central_directory(archive: &[u8]) -> Result<usize, String> {
    const HEADER_BYTES: usize = 22;
    const MAX_COMMENT_BYTES: usize = u16::MAX as usize;
    if archive.len() < HEADER_BYTES {
        return Err("downloaded update is not a complete ZIP archive".into());
    }
    let last = archive.len() - HEADER_BYTES;
    let first = archive
        .len()
        .saturating_sub(HEADER_BYTES + MAX_COMMENT_BYTES);
    for offset in (first..=last).rev() {
        if read_u32(archive, offset)? == END_OF_CENTRAL_DIRECTORY_SIGNATURE {
            let comment_len = read_u16(archive, offset + 20)? as usize;
            if offset + HEADER_BYTES + comment_len == archive.len() {
                return Ok(offset);
            }
        }
    }
    Err("downloaded update has no valid ZIP central directory".into())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset.saturating_add(2))
        .ok_or_else(|| "downloaded update ZIP is truncated".to_string())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or_else(|| "downloaded update ZIP is truncated".to_string())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn is_executable_name(name: &[u8]) -> bool {
    name == b"KeySteer/KeySteer.exe" || name == b"KeySteer\\KeySteer.exe"
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn persist_extracted(destination: &Path, executable: &[u8]) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            format!(
                "cannot create extracted update executable {}: {error}",
                destination.display()
            )
        })?;
    output
        .write_all(executable)
        .and_then(|()| output.flush())
        .and_then(|()| output.sync_all())
        .map_err(|error| format!("cannot persist extracted update executable: {error}"))
        .inspect_err(|_| {
            let _ = fs::remove_file(destination);
        })
}

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

    fn append_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn append_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn test_zip(entries: &[(&str, &[u8], u16)]) -> Vec<u8> {
        struct CentralEntry {
            name: Vec<u8>,
            crc32: u32,
            compressed_size: u32,
            uncompressed_size: u32,
            compression: u16,
            local_offset: u32,
        }

        let mut archive = Vec::new();
        let mut central_entries = Vec::new();
        for &(name, payload, compression) in entries {
            let compressed = match compression {
                STORED => payload.to_vec(),
                DEFLATED => miniz_oxide::deflate::compress_to_vec(payload, 6),
                _ => Vec::new(),
            };
            let name = name.as_bytes();
            let entry = CentralEntry {
                name: name.to_vec(),
                crc32: crc32(payload),
                compressed_size: compressed.len() as u32,
                uncompressed_size: payload.len() as u32,
                compression,
                local_offset: archive.len() as u32,
            };
            append_u32(&mut archive, LOCAL_FILE_SIGNATURE);
            append_u16(&mut archive, 20);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, compression);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, 0);
            append_u32(&mut archive, entry.crc32);
            append_u32(&mut archive, entry.compressed_size);
            append_u32(&mut archive, entry.uncompressed_size);
            append_u16(&mut archive, name.len() as u16);
            append_u16(&mut archive, 0);
            archive.extend_from_slice(name);
            archive.extend_from_slice(&compressed);
            central_entries.push(entry);
        }

        let central_offset = archive.len() as u32;
        for entry in &central_entries {
            append_u32(&mut archive, CENTRAL_FILE_SIGNATURE);
            append_u16(&mut archive, 20);
            append_u16(&mut archive, 20);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, entry.compression);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, 0);
            append_u32(&mut archive, entry.crc32);
            append_u32(&mut archive, entry.compressed_size);
            append_u32(&mut archive, entry.uncompressed_size);
            append_u16(&mut archive, entry.name.len() as u16);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, 0);
            append_u16(&mut archive, 0);
            append_u32(&mut archive, 0);
            append_u32(&mut archive, entry.local_offset);
            archive.extend_from_slice(&entry.name);
        }
        let central_size = archive.len() as u32 - central_offset;
        append_u32(&mut archive, END_OF_CENTRAL_DIRECTORY_SIGNATURE);
        append_u16(&mut archive, 0);
        append_u16(&mut archive, 0);
        append_u16(&mut archive, central_entries.len() as u16);
        append_u16(&mut archive, central_entries.len() as u16);
        append_u32(&mut archive, central_size);
        append_u32(&mut archive, central_offset);
        append_u16(&mut archive, 0);
        archive
    }

    fn temporary_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("keysteer-{label}-{}-{nonce}", std::process::id()));
        (base.with_extension("zip"), base.with_extension("exe"))
    }

    #[test]
    fn extracts_only_the_packaged_executable_with_bounded_deflate() {
        let payload = b"MZ signed update payload";
        for (name, compression) in [
            ("KeySteer/KeySteer.exe", STORED),
            ("KeySteer\\KeySteer.exe", DEFLATED),
        ] {
            let (archive, destination) = temporary_paths("extract-update");
            let bytes = test_zip(&[(name, payload, compression)]);
            assert!(fs::write(&archive, bytes).is_ok());
            let result = extract_archive_executable(&archive, &destination);
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(
                fs::read(&destination).ok().as_deref(),
                Some(payload.as_slice())
            );
            let _ = fs::remove_file(archive);
            let _ = fs::remove_file(destination);
        }
    }

    #[test]
    fn rejects_duplicate_or_renamed_executables() {
        for entries in [
            vec![
                ("KeySteer/KeySteer.exe", b"MZ one".as_slice(), STORED),
                ("KeySteer/KeySteer.exe", b"MZ two".as_slice(), STORED),
            ],
            vec![("../KeySteer.exe", b"MZ payload".as_slice(), STORED)],
        ] {
            let (archive, destination) = temporary_paths("reject-update-path");
            assert!(fs::write(&archive, test_zip(&entries)).is_ok());
            assert!(extract_archive_executable(&archive, &destination).is_err());
            assert!(!destination.exists());
            let _ = fs::remove_file(archive);
        }
    }

    #[test]
    fn rejects_an_executable_with_corrupted_zip_crc() {
        let name = "KeySteer/KeySteer.exe";
        let mut archive_bytes = test_zip(&[(name, b"MZ payload", STORED)]);
        let data_offset = 30 + name.len();
        archive_bytes[data_offset + 3] ^= 1;
        let (archive, destination) = temporary_paths("reject-update-crc");
        assert!(fs::write(&archive, archive_bytes).is_ok());
        assert!(extract_archive_executable(&archive, &destination).is_err());
        assert!(!destination.exists());
        let _ = fs::remove_file(archive);
    }

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
