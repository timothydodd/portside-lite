//! Protects saved credentials at rest.
//!
//! On Windows this is DPAPI (`CryptProtectData`, current-user scope): only the
//! same Windows account on the same machine can decrypt, so a copied database
//! file is useless elsewhere. Other platforms have no protection here;
//! [`protect`] returns `None` and callers keep the value as it was.

/// Marks a value produced by [`protect`].
const PREFIX: &str = "dpapi:";

/// Whether [`protect`] can encrypt on this platform.
pub const AVAILABLE: bool = cfg!(windows);

/// True for values produced by [`protect`].
pub fn is_protected(value: &str) -> bool {
    value.starts_with(PREFIX)
}

/// Encrypt `plain`; `None` when this platform can't (or the OS call failed).
pub fn protect(plain: &str) -> Option<String> {
    imp::protect(plain.as_bytes()).map(|blob| format!("{PREFIX}{}", hex(&blob)))
}

/// Decrypt a value from [`protect`]. Values without the marker are returned as
/// they are (saved before encryption existed).
pub fn unprotect(value: &str) -> Result<String, String> {
    let Some(hex_blob) = value.strip_prefix(PREFIX) else { return Ok(value.to_string()) };
    let blob = unhex(hex_blob).ok_or("The saved value is damaged.")?;
    let plain = imp::unprotect(&blob)?;
    String::from_utf8(plain).map_err(|_| "The saved value is damaged.".to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Extra input to the key, so another app's plain `CryptUnprotectData`
    /// call on the same account doesn't decrypt these by accident.
    const ENTROPY: &[u8] = b"portside-lite credentials v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copy out and free a blob the OS allocated.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: input/entropy point at live slices for the call; on success
        // the OS allocates `out`, which `take` copies and frees.
        let ok = unsafe { CryptProtectData(&input, null(), &entropy, null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        (ok != 0).then(|| unsafe { take(out) })
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: as in `protect`.
        let ok = unsafe { CryptUnprotectData(&input, null_mut(), &entropy, null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 {
            return Err("It was saved by another Windows account or on another computer.".into());
        }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn protect(_plain: &[u8]) -> Option<Vec<u8>> {
        None
    }

    pub fn unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
        Err("Saved on Windows; this platform can't decrypt it.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 0x7f, 0x80, 0xff];
        assert_eq!(unhex(&hex(&bytes)).unwrap(), bytes);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }

    #[test]
    fn plain_values_pass_through() {
        assert_eq!(unprotect("hunter2").unwrap(), "hunter2");
        assert!(!is_protected("hunter2"));
    }

    #[test]
    fn protect_round_trips_where_available() {
        match protect("pässword") {
            Some(p) => {
                assert!(AVAILABLE && is_protected(&p) && !p.contains("pässword"));
                assert_eq!(unprotect(&p).unwrap(), "pässword");
            }
            None => assert!(!AVAILABLE),
        }
    }
}
