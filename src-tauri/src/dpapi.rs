//! Protection of secrets with Windows DPAPI.
//!
//! Encrypted data can only be decrypted by the same Windows user on the
//! same PC: a `config.json` copied elsewhere does not reveal the pairing key.

const ENTROPY: &[u8] = b"tv-tray/v1";

#[cfg(windows)]
mod imp {
    use super::ENTROPY;
    use std::{ffi::c_void, ptr, slice};

    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            desc: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            desc: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    const UI_FORBIDDEN: u32 = 0x1;

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let input = Blob { len: data.len() as u32, data: data.as_ptr() as *mut u8 };
        let entropy = Blob { len: ENTROPY.len() as u32, data: ENTROPY.as_ptr() as *mut u8 };
        let mut out = Blob { len: 0, data: ptr::null_mut() };
        // SAFETY: the input buffers live for the whole call; `out` is
        // filled in by the system and freed with LocalFree after copying.
        unsafe {
            let ok = CryptProtectData(
                &input,
                ptr::null(),
                &entropy,
                ptr::null_mut(),
                ptr::null_mut(),
                UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 || out.data.is_null() {
                return None;
            }
            let v = slice::from_raw_parts(out.data, out.len as usize).to_vec();
            LocalFree(out.data as *mut c_void);
            Some(v)
        }
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let input = Blob { len: data.len() as u32, data: data.as_ptr() as *mut u8 };
        let entropy = Blob { len: ENTROPY.len() as u32, data: ENTROPY.as_ptr() as *mut u8 };
        let mut out = Blob { len: 0, data: ptr::null_mut() };
        // SAFETY: same as in `protect`.
        unsafe {
            let ok = CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &entropy,
                ptr::null_mut(),
                ptr::null_mut(),
                UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 || out.data.is_null() {
                return None;
            }
            let v = slice::from_raw_parts(out.data, out.len as usize).to_vec();
            LocalFree(out.data as *mut c_void);
            Some(v)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn protect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub fn unprotect(_: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Encrypts a text; returns the hex of the DPAPI blob.
pub fn seal(text: &str) -> Option<String> {
    imp::protect(text.as_bytes()).map(|b| to_hex(&b))
}

/// Decrypts a blob produced by `seal`.
pub fn open(hex: &str) -> Option<String> {
    let bytes = imp::unprotect(&from_hex(hex)?)?;
    String::from_utf8(bytes).ok()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_not_plaintext() {
        let sealed = seal("test-secret").expect("encryption");
        assert!(!sealed.contains("segreto"));
        assert_eq!(open(&sealed).as_deref(), Some("test-secret"));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(open("00ff"), None);
        assert_eq!(open("zz"), None);
    }
}
