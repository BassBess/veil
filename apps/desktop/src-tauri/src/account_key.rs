#[cfg(windows)]
fn transform(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| "Invalid vault key size")?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI allocates output. Copy it before releasing it with LocalFree.
    // No machine-wide flag is used: this protection is scoped to the Windows user.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(
            "Windows could not unlock this Vault. Use the Windows account that created it.".into(),
        );
    }
    if output.pbData.is_null() {
        return Err("Windows returned an empty vault key".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(result)
}

pub fn protect(key: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        transform(key, true)
    }
    #[cfg(target_os = "macos")]
    {
        mac_protect(key)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = key;
        Err("Account protection requires Windows or macOS".into())
    }
}

pub fn unprotect(key: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        transform(key, false)
    }
    #[cfg(target_os = "macos")]
    {
        mac_unprotect(key)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = key;
        Err("Account protection requires Windows or macOS".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn windows_user_key_roundtrip_and_corruption() {
        let key = [42u8; 32];
        let encrypted = super::protect(&key).unwrap();
        assert_ne!(encrypted, key);
        assert_eq!(super::unprotect(&encrypted).unwrap(), key);
        let mut damaged = encrypted;
        damaged[0] ^= 1;
        assert!(super::unprotect(&damaged).is_err());
    }
}

#[cfg(target_os = "macos")]
fn mac_protect(key: &[u8]) -> Result<Vec<u8>, String> {
    let name = format!("vault-{}", uuid::Uuid::new_v4());
    keyring::Entry::new("com.veil.local-vault", &name)
        .map_err(|_| "Unable to access macOS Keychain")?
        .set_secret(key)
        .map_err(|_| "macOS Keychain did not save the Vault key")?;
    Ok(name.into_bytes())
}

#[cfg(target_os = "macos")]
fn mac_unprotect(reference: &[u8]) -> Result<Vec<u8>, String> {
    let name = std::str::from_utf8(reference).map_err(|_| "Invalid Keychain reference")?;
    keyring::Entry::new("com.veil.local-vault", name)
        .map_err(|_| "Unable to access macOS Keychain")?
        .get_secret()
        .map_err(|_| "macOS Keychain could not unlock the Vault")
}
