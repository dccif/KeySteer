//! Authenticode verification and signer identity extraction.

use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CERT_E_UNTRUSTEDROOT, HWND};
use windows::Win32::Security::Cryptography::{
    CERT_SHA256_HASH_PROP_ID, CertGetCertificateContextProperty,
};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_WHOLECHAIN,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::core::PCWSTR;

const SIGNER_HASH_BYTES: usize = 32;

/// Verify an embedded Authenticode signature and return the leaf certificate's
/// SHA-256 fingerprint. A sole untrusted-root result is accepted because a
/// self-signed release certificate is pinned by comparing it with the already
/// running executable; digest, expiry and every other trust failure remain
/// fatal. Users therefore do not need to install the self-signed certificate.
pub(super) fn verified_signer(path: &Path) -> Result<[u8; SIGNER_HASH_BYTES], String> {
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide_path.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    // SAFETY: `file`, `data`, the path buffer and action GUID remain valid for
    // the synchronous verification. State is explicitly closed below.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    let result = if status == 0 || status == CERT_E_UNTRUSTEDROOT.0 {
        // SAFETY: successful WTD_STATEACTION_VERIFY owns valid provider state
        // until the matching CLOSE. All pointers are checked before use.
        unsafe { signer_hash_from_state(&data) }
    } else {
        Err(format!(
            "{} does not have a valid Authenticode signature (0x{:08X})",
            path.display(),
            status as u32
        ))
    };

    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closes exactly the state opened by the verification call above.
    let _ = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    result
}

unsafe fn signer_hash_from_state(data: &WINTRUST_DATA) -> Result<[u8; SIGNER_HASH_BYTES], String> {
    // SAFETY: caller guarantees `hWVTStateData` is live WinVerifyTrust state.
    let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
    if provider.is_null() {
        return Err("Windows trust provider did not expose signature state".into());
    }
    // SAFETY: provider was checked and remains owned by WinVerifyTrust state.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
    if signer.is_null() {
        return Err("Windows trust provider did not expose the primary signer".into());
    }
    // SAFETY: signer was checked and its first chain certificate is the leaf.
    let provider_cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if provider_cert.is_null() {
        return Err("Windows trust provider did not expose the signing certificate".into());
    }
    // SAFETY: provider_cert belongs to live trust state and is checked above.
    let cert = unsafe { (*provider_cert).pCert };
    if cert.is_null() {
        return Err("Windows trust provider returned an empty signing certificate".into());
    }

    let mut size = SIGNER_HASH_BYTES as u32;
    let mut hash = [0_u8; SIGNER_HASH_BYTES];
    // SAFETY: `cert` remains live and `hash` is a writable `size`-byte buffer.
    unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_SHA256_HASH_PROP_ID,
            Some(hash.as_mut_ptr().cast()),
            &mut size,
        )
    }
    .map_err(|error| format!("cannot read Authenticode signer fingerprint: {error}"))?;
    if size as usize != SIGNER_HASH_BYTES {
        return Err(format!(
            "Windows returned an unexpected signer fingerprint length: {size}"
        ));
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires two locally Authenticode-signed executables"]
    fn accepts_a_pair_signed_by_the_same_certificate() -> Result<(), Box<dyn std::error::Error>> {
        let current = std::env::var_os("KEYSTEER_TEST_SIGNED_CURRENT")
            .ok_or("KEYSTEER_TEST_SIGNED_CURRENT is not set")?;
        let candidate = std::env::var_os("KEYSTEER_TEST_SIGNED_CANDIDATE")
            .ok_or("KEYSTEER_TEST_SIGNED_CANDIDATE is not set")?;
        let current = verified_signer(Path::new(&current))?;
        let candidate = verified_signer(Path::new(&candidate))?;
        assert_eq!(current, candidate);
        Ok(())
    }

    #[test]
    #[ignore = "requires a locally Authenticode-signed executable with a tampered copy"]
    fn rejects_a_tampered_signed_executable() -> Result<(), Box<dyn std::error::Error>> {
        let tampered = std::env::var_os("KEYSTEER_TEST_TAMPERED_SIGNED")
            .ok_or("KEYSTEER_TEST_TAMPERED_SIGNED is not set")?;
        assert!(verified_signer(Path::new(&tampered)).is_err());
        Ok(())
    }
}
