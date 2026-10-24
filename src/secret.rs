use serde::{Deserialize, Deserializer, Serializer};

const PREFIX: &str = "dpapi:";

pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&seal(v))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(reveal(String::deserialize(d)?))
}

fn seal(v: &str) -> String {
    match dpapi(v.as_bytes(), true) {
        Some(enc) => format!(
            "{PREFIX}{}",
            enc.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        None => v.to_string(),
    }
}

/// Unprefixed values are plaintext from before encryption. A value that fails to decrypt
/// (other Windows user or machine) becomes empty: failing here would drop every saved profile.
fn reveal(v: String) -> String {
    let Some(hex) = v.strip_prefix(PREFIX) else {
        return v;
    };
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()
        .and_then(|enc| dpapi(&enc, false))
        .and_then(|dec| String::from_utf8(dec).ok())
        .unwrap_or_default()
}

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
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
        if protect {
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

// ponytail: plaintext outside Windows; add libsecret/Keychain if other platforms need it.
#[cfg(not(windows))]
fn dpapi(_: &[u8], _: bool) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_legacy() {
        assert_eq!(reveal("plain".into()), "plain");
        assert_eq!(reveal(seal("")), "");
        assert_eq!(reveal(seal("AKIA/sëcret+")), "AKIA/sëcret+");
        assert_eq!(reveal(format!("{PREFIX}zz")), "");
        #[cfg(windows)]
        {
            let s = seal("AKIA");
            assert!(s.starts_with(PREFIX) && !s.contains("AKIA"));
            assert_eq!(reveal(format!("{PREFIX}00ff")), "");
        }
    }
}
