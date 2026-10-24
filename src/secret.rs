use serde::{Deserialize, Deserializer, Serializer};

#[cfg(windows)]
const PREFIX: &str = "dpapi:";
#[cfg(not(windows))]
const PREFIX: &str = "keyring:";

pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&seal(v))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(reveal(String::deserialize(d)?))
}

/// Prefixed values failed to decrypt on load; writing them back untouched keeps a locked keyring from wiping them.
fn seal(v: &str) -> String {
    if v.starts_with(PREFIX) {
        return v.to_string();
    }
    match crypt(v.as_bytes(), true) {
        Some(enc) => format!(
            "{PREFIX}{}",
            enc.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        None => v.to_string(),
    }
}

fn reveal(v: String) -> String {
    v.strip_prefix(PREFIX)
        .and_then(|hex| {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .and_then(|enc| crypt(&enc, false))
        .and_then(|dec| String::from_utf8(dec).ok())
        .unwrap_or(v)
}

#[cfg(windows)]
fn crypt(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::*;

    if data.is_empty() {
        return None;
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let flags = CRYPTPROTECT_UI_FORBIDDEN;
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, null(), null(), null(), null(), flags, &mut out)
        } else {
            CryptUnprotectData(&input, null_mut(), null(), null(), null(), flags, &mut out)
        }
    };
    if ok == 0 {
        return None;
    }
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData.cast()) };
    Some(v)
}

#[cfg(not(windows))]
fn crypt(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, Generate, Key, KeyInit};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    use std::sync::OnceLock;

    static CIPHER: OnceLock<Option<XChaCha20Poly1305>> = OnceLock::new();
    let cipher = CIPHER
        .get_or_init(|| {
            let entry = keyring::Entry::new("skybin", "master-key").ok()?;
            let key = match entry.get_secret() {
                Ok(k) => k,
                Err(keyring::Error::NoEntry) => {
                    let k = Key::<XChaCha20Poly1305>::generate().to_vec();
                    entry.set_secret(&k).ok()?;
                    k
                }
                Err(_) => return None,
            };
            XChaCha20Poly1305::new_from_slice(&key).ok()
        })
        .as_ref()?;

    if data.is_empty() {
        return None;
    }
    if encrypt {
        let nonce = XNonce::generate();
        let mut out = nonce.to_vec();
        out.extend(cipher.encrypt(&nonce, data).ok()?);
        Some(out)
    } else {
        let (nonce, ct) = data.split_at_checked(24)?;
        cipher.decrypt(&XNonce::try_from(nonce).ok()?, ct).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_legacy() {
        assert_eq!(reveal("plain".into()), "plain");
        assert_eq!(reveal(seal("")), "");
        assert_eq!(reveal(seal("AKIA/sëcret+")), "AKIA/sëcret+");
        let bad = format!("{PREFIX}00ff");
        assert_eq!(reveal(bad.clone()), bad);
        assert_eq!(seal(&bad), bad);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_hides_value() {
        let s = seal("AKIA");
        assert!(s.starts_with(PREFIX) && !s.contains("AKIA"));
    }
}
