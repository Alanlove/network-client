//! Sensitive-data protection.
//!
//! On Windows uses DPAPI (`CryptProtectData`, current user scope) — design
//! doc §47. Subscription URLs, UUIDs, passwords and tokens must go through
//! here before hitting disk. On non-Windows hosts (CI) it falls back to
//! base64, which is NOT encryption and is labelled as such in the output.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::common::{SvcError, SvcResult};

const APP_ENTROPY: &[u8] = b"NetworkClient/v1/DPAPI";
const WIN_PREFIX: &str = "dpapi:";
const DEV_PREFIX: &str = "devb64:";

pub fn protect(plaintext: &str) -> SvcResult<String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Cryptography::{
            CryptProtectData, CRYPT_INTEGER_BLOB,
        };

        let mut entropy = APP_ENTROPY.to_vec();
        let blob_in = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let blob_entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_mut_ptr(),
        };
        let mut blob_out = CRYPT_INTEGER_BLOB::default();

        // SAFETY: all buffers are local, valid for the call and not aliased;
        // the returned out-blob is released with LocalFree.
        unsafe {
            CryptProtectData(
                &blob_in,
                PCWSTR::null(),
                Some(&blob_entropy),
                None,
                None,
                1, // CRYPTPROTECT_UI_FORBIDDEN
                &mut blob_out,
            )
            .map_err(|e| SvcError::Other(format!("DPAPI protect failed: {e}")))?;

            let protected =
                std::slice::from_raw_parts(blob_out.pbData as *const u8, blob_out.cbData as usize)
                    .to_vec();
            LocalFree(HLOCAL(blob_out.pbData as *mut core::ffi::c_void));
            Ok(format!("{WIN_PREFIX}{}", STANDARD.encode(protected)))
        }
    }
    #[cfg(not(windows))]
    {
        Ok(format!(
            "{DEV_PREFIX}{}",
            STANDARD.encode(plaintext.as_bytes())
        ))
    }
}

pub fn unprotect(stored: &str) -> SvcResult<String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    if let Some(b64) = stored.strip_prefix(WIN_PREFIX) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{HLOCAL, LocalFree};
            use windows::Win32::Security::Cryptography::{
                CryptUnprotectData, CRYPT_INTEGER_BLOB,
            };

            let raw = STANDARD
                .decode(b64)
                .map_err(|e| SvcError::Other(format!("corrupt protected payload: {e}")))?;
            // Entropy MUST match the value used in `protect`, otherwise
            // CryptUnprotectData fails with ERROR_INVALID_DATA (0x8007000D).
            let mut entropy = APP_ENTROPY.to_vec();
            let blob_in = CRYPT_INTEGER_BLOB {
                cbData: raw.len() as u32,
                pbData: raw.as_ptr() as *mut u8,
            };
            let blob_entropy = CRYPT_INTEGER_BLOB {
                cbData: entropy.len() as u32,
                pbData: entropy.as_mut_ptr(),
            };
            let mut blob_out = CRYPT_INTEGER_BLOB::default();
            unsafe {
                CryptUnprotectData(
                    &blob_in,
                    None,
                    Some(&blob_entropy),
                    None,
                    None,
                    1, // CRYPTPROTECT_UI_FORBIDDEN
                    &mut blob_out,
                )
                .map_err(|e| SvcError::Other(format!("DPAPI unprotect failed: {e}")))?;
                let bytes = std::slice::from_raw_parts(
                    blob_out.pbData as *const u8,
                    blob_out.cbData as usize,
                )
                .to_vec();
                LocalFree(HLOCAL(blob_out.pbData as *mut core::ffi::c_void));
                String::from_utf8(bytes).map_err(|e| SvcError::Other(e.to_string()))
            }
        }
        #[cfg(not(windows))]
        {
            let _ = b64;
            Err(SvcError::Other(
                "cannot unprotect DPAPI payload off-Windows".into(),
            ))
        }
    } else if let Some(b64) = stored.strip_prefix(DEV_PREFIX) {
        let raw = STANDARD
            .decode(b64)
            .map_err(|e| SvcError::Other(format!("corrupt dev payload: {e}")))?;
        String::from_utf8(raw).map_err(|e| SvcError::Other(e.to_string()))
    } else {
        // Tolerate accidental plaintext from earlier builds.
        Ok(stored.to_string())
    }
}
