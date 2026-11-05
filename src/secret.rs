use serde::{Deserialize, Deserializer, Serializer};

#[cfg(windows)]
const PREFIX: &str = "dpapi:";
#[cfg(target_os = "macos")]
const PREFIX: &str = "keyfile:";
#[cfg(not(any(windows, target_os = "macos")))]
const PREFIX: &str = "keyring:";

pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&seal(v))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(reveal(String::deserialize(d)?))
}

// failed to decrypt on load; written back as-is so a locked keyring can't wipe them
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
    // sealed with the old macOS keychain key, which is no longer read
    if cfg!(target_os = "macos") && v.starts_with("keyring:") {
        return String::new();
    }
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
            let new_key = || Key::<XChaCha20Poly1305>::generate().to_vec();
            XChaCha20Poly1305::new_from_slice(&master_key(new_key)?).ok()
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

// not the keychain: it asks for the password whenever the unsigned binary changes
#[cfg(target_os = "macos")]
fn master_key(new_key: impl FnOnce() -> Vec<u8>) -> Option<Vec<u8>> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = eframe::storage_dir("skybin")?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("master.key");
    if let Ok(k) = std::fs::read(&path) {
        return (k.len() == 32).then_some(k);
    }
    // linked into place so a concurrent start never reads half a key
    let tmp = dir.join(format!("master.key.{}", std::process::id()));
    let k = new_key();
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(&k).and_then(|()| f.sync_all()));
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, &path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Some(k),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read(&path).ok().filter(|k| k.len() == 32)
        }
        Err(_) => None,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn master_key(new_key: impl FnOnce() -> Vec<u8>) -> Option<Vec<u8>> {
    let entry = keyring::Entry::new("skybin", "master-key").ok()?;
    match entry.get_secret() {
        Ok(k) => Some(k),
        Err(keyring::Error::NoEntry) => {
            let k = new_key();
            entry.set_secret(&k).ok()?;
            Some(k)
        }
        Err(_) => None,
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

    #[cfg(target_os = "macos")]
    #[test]
    fn old_keychain_values_are_dropped() {
        assert_eq!(reveal("keyring:00ff".into()), "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn key_file_is_private_and_hides_value() {
        use std::os::unix::fs::PermissionsExt;
        let s = seal("AKIA");
        assert!(s.starts_with(PREFIX) && !s.contains("AKIA"));
        let path = eframe::storage_dir("skybin").unwrap().join("master.key");
        let meta = std::fs::metadata(path).unwrap();
        assert_eq!(meta.len(), 32);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_hides_value() {
        let s = seal("AKIA");
        assert!(s.starts_with(PREFIX) && !s.contains("AKIA"));
    }
}
