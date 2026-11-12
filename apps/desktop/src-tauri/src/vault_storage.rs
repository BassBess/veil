use crate::protection::SessionVault;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, rand_core::RngCore},
};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
};
use zeroize::Zeroizing;

const FORMAT_VERSION: u32 = 1;
const MAX_VAULT_BYTES: u64 = 100 * 1024 * 1024;
const ASSOCIATED_DATA: &[u8] = b"Veil encrypted vault v1";
const AUTHENTICATED_DATA: &[u8] = b"Veil encrypted vault v3 system verification";
fn associated_data(authentication_required: bool) -> &'static [u8] {
    if authentication_required {
        AUTHENTICATED_DATA
    } else {
        ASSOCIATED_DATA
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    salt: [u8; 16],
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
    #[serde(default, alias = "windows_key")]
    account_key: Option<Vec<u8>>,
    #[serde(default)]
    account_platform: Option<String>,
}

pub struct UnlockedVault {
    pub data: SessionVault,
    key: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    path: PathBuf,
    account_key: Option<Vec<u8>>,
    pub authentication_required: bool,
    // Keep the OS file lock for the entire unlocked lifetime.
    _lock: File,
}

fn acquire_lock(path: &std::path::Path) -> Result<File, String> {
    let parent = path.parent().ok_or("Invalid vault location")?;
    std::fs::create_dir_all(parent).map_err(|_| "Unable to access the vault folder")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|_| "Unable to open the vault lock")?;
    lock.try_lock()
        .map_err(|_| "This Vault is open in another Veil window. Lock it there first.")?;
    Ok(lock)
}

fn read_envelope(path: &std::path::Path) -> Result<Envelope, String> {
    if std::fs::metadata(path)
        .map_err(|_| "Unable to read the Vault")?
        .len()
        > MAX_VAULT_BYTES
    {
        return Err("The vault file exceeds the supported size".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|_| "Unable to read the Vault")?)
        .map_err(|_| "The vault file is damaged or unsupported".into())
}

fn derive_key(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| "Unable to derive the vault encryption key")?;
    Ok(key)
}

impl UnlockedVault {
    pub fn requires_authentication(path: &std::path::Path) -> Result<bool, String> {
        Ok(read_envelope(path)?.version == 3)
    }

    pub fn needs_passphrase_migration(path: &std::path::Path) -> Result<bool, String> {
        Ok(read_envelope(path)?.version == 1)
    }

    pub fn open_account(path: PathBuf, reset: bool) -> Result<Self, String> {
        Self::open_account_with_auth(path, reset, false)
    }

    pub fn open_account_with_auth(
        path: PathBuf,
        reset: bool,
        require_authentication: bool,
    ) -> Result<Self, String> {
        let lock = acquire_lock(&path)?;
        let should_save = reset || !path.exists();
        let (data, key, salt, protected_key, authentication_required) = if path.exists() && !reset {
            let envelope = read_envelope(&path)?;
            if envelope.version == 1 {
                return Err(
                    "Enter your existing Vault passphrase once to switch to account protection."
                        .into(),
                );
            }
            if !matches!(envelope.version, 2 | 3) {
                return Err("This vault version is unsupported".into());
            }
            if envelope.account_platform.as_deref().unwrap_or("windows") != std::env::consts::OS {
                return Err("This Vault uses another operating system's account protection. Use its original computer.".into());
            }
            let protected = envelope
                .account_key
                .ok_or("account-protected vault key is missing")?;
            let decrypted_key = Zeroizing::new(crate::account_key::unprotect(&protected)?);
            let key: [u8; 32] = decrypted_key
                .as_slice()
                .try_into()
                .map_err(|_| "Invalid account-protected vault key")?;
            let key = Zeroizing::new(key);
            let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| "Invalid encryption key")?;
            let plaintext = Zeroizing::new(
                cipher
                    .decrypt(
                        Nonce::from_slice(&envelope.nonce),
                        aes_gcm::aead::Payload {
                            msg: &envelope.ciphertext,
                            aad: associated_data(envelope.version == 3),
                        },
                    )
                    .map_err(|_| "The encrypted vault could not be read")?,
            );
            let data = serde_json::from_slice(&plaintext)
                .map_err(|_| "The vault contents are unsupported")?;
            (data, key, envelope.salt, protected, envelope.version == 3)
        } else {
            let mut key = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(&mut *key);
            let protected = crate::account_key::protect(&*key)?;
            (
                SessionVault::default(),
                key,
                [0u8; 16],
                protected,
                require_authentication,
            )
        };
        let vault = Self {
            data,
            key,
            salt,
            path,
            account_key: Some(protected_key),
            authentication_required,
            _lock: lock,
        };
        // A new/reset vault is saved before the UI receives success.
        if should_save {
            vault.persist(&vault.data)?;
        }
        Ok(vault)
    }

    pub fn migrate_to_account(mut self) -> Result<Self, String> {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *key);
        self.account_key = Some(crate::account_key::protect(&*key)?);
        self.key = key;
        self.salt = [0u8; 16];
        self.persist(&self.data)?;
        Ok(self)
    }

    pub fn open(path: PathBuf, password: &str, create: bool) -> Result<Self, String> {
        let parent = path.parent().ok_or("Invalid vault location")?;
        std::fs::create_dir_all(parent).map_err(|_| "Unable to access the vault folder")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|_| "Unable to open the vault lock")?;
        lock.try_lock()
            .map_err(|_| "This vault is open in another Veil window. Lock it there first.")?;
        let (data, key, salt) = if create {
            if path.exists() {
                return Err("A vault already exists. Unlock it using its passphrase.".into());
            }
            if password.chars().count() < 12 {
                return Err("Use a vault passphrase with at least 12 characters".into());
            }
            let mut salt = [0u8; 16];
            OsRng.fill_bytes(&mut salt);
            (SessionVault::default(), derive_key(password, &salt)?, salt)
        } else {
            let metadata = std::fs::metadata(&path).map_err(|_| "No saved vault was found")?;
            if metadata.len() > MAX_VAULT_BYTES {
                return Err("The vault file exceeds the supported size".into());
            }
            let bytes = std::fs::read(&path).map_err(|_| "Unable to read the vault file")?;
            let envelope: Envelope = serde_json::from_slice(&bytes)
                .map_err(|_| "The vault file is damaged or unsupported")?;
            if envelope.version != FORMAT_VERSION {
                return Err("This vault version is unsupported".into());
            }
            let key = derive_key(password, &envelope.salt)?;
            let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| "Invalid encryption key")?;
            let plaintext = Zeroizing::new(
                cipher
                    .decrypt(
                        Nonce::from_slice(&envelope.nonce),
                        aes_gcm::aead::Payload {
                            msg: &envelope.ciphertext,
                            aad: ASSOCIATED_DATA,
                        },
                    )
                    .map_err(|_| "Incorrect passphrase or damaged vault file")?,
            );
            let data: SessionVault = serde_json::from_slice(&plaintext)
                .map_err(|_| "The vault contents are unsupported")?;
            (data, key, envelope.salt)
        };
        let vault = Self {
            data,
            key,
            salt,
            path,
            account_key: None,
            authentication_required: false,
            _lock: lock,
        };
        if create {
            vault.persist(&vault.data)?;
        }
        Ok(vault)
    }

    #[cfg(test)]
    pub fn change_protection(&mut self, use_account: bool, password: &str) -> Result<(), String> {
        self.change_protection_with_auth(use_account, password, false)
    }

    pub fn change_protection_with_auth(
        &mut self,
        use_account: bool,
        password: &str,
        require_authentication: bool,
    ) -> Result<(), String> {
        if require_authentication && !use_account {
            return Err("System sign-in requires account protection".into());
        }
        let (key, salt, wrapped) = if use_account {
            let mut key = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(&mut *key);
            let wrapped = crate::account_key::protect(&*key)?;
            (key, [0u8; 16], Some(wrapped))
        } else {
            if password.chars().count() < 12 {
                return Err("Use a passphrase with at least 12 characters".into());
            }
            let mut salt = [0u8; 16];
            OsRng.fill_bytes(&mut salt);
            (derive_key(password, &salt)?, salt, None)
        };
        let old_key = std::mem::replace(&mut self.key, key);
        let old_salt = std::mem::replace(&mut self.salt, salt);
        let old_wrapped = std::mem::replace(&mut self.account_key, wrapped);
        let old_auth = std::mem::replace(&mut self.authentication_required, require_authentication);
        if let Err(error) = self.persist(&self.data) {
            self.key = old_key;
            self.salt = old_salt;
            self.account_key = old_wrapped;
            self.authentication_required = old_auth;
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self, data: &SessionVault) -> Result<(), String> {
        let plaintext = Zeroizing::new(
            serde_json::to_vec(data).map_err(|_| "Unable to prepare the vault for saving")?,
        );
        let cipher = Aes256Gcm::new_from_slice(&*self.key).map_err(|_| "Invalid encryption key")?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                aes_gcm::aead::Payload {
                    msg: &plaintext,
                    aad: associated_data(self.authentication_required),
                },
            )
            .map_err(|_| "Unable to encrypt the vault")?;
        let bytes = serde_json::to_vec(&Envelope {
            version: if self.authentication_required {
                3
            } else if self.account_key.is_some() {
                2
            } else {
                FORMAT_VERSION
            },
            salt: self.salt,
            nonce: nonce.into(),
            ciphertext,
            account_key: self.account_key.clone(),
            account_platform: self
                .account_key
                .as_ref()
                .map(|_| std::env::consts::OS.into()),
        })
        .map_err(|_| "Unable to encode the encrypted vault")?;
        if bytes.len() as u64 > MAX_VAULT_BYTES {
            return Err("The vault is full. Clear document history to free space. Saved identities will be kept.".into());
        }
        let parent = self.path.parent().ok_or("Invalid vault location")?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Unable to create a vault save file")?;
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| "Unable to save the encrypted vault")?;
        file.persist(&self.path)
            .map_err(|_| "Unable to replace the encrypted vault file")?;
        Ok(())
    }

    pub fn update<T>(
        &mut self,
        change: impl FnOnce(&mut SessionVault) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut candidate = self.data.clone();
        let result = change(&mut candidate)?;
        // Commit in memory only after the encrypted file has been saved.
        self.persist(&candidate)?;
        self.data = candidate;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::ReviewedEntity;
    const PASSWORD: &str = "a long test passphrase";
    fn john() -> Vec<ReviewedEntity> {
        vec![ReviewedEntity {
            id: "1".into(),
            entity_type: "PERSON".into(),
            text: "John Smith".into(),
            start: 0,
            end: 10,
        }]
    }

    #[test]
    fn reopening_preserves_short_pseudonyms_and_restoration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let mut vault = UnlockedVault::open(path.clone(), PASSWORD, true).unwrap();
        let record = vault
            .update(|data| data.process("John Smith".into(), john(), "pseudonymize"))
            .unwrap();
        assert_eq!(record.text, "PERSON_001");
        let encrypted = std::fs::read_to_string(&path).unwrap();
        assert!(!encrypted.contains("John Smith"));
        assert!(!encrypted.contains("PERSON_001"));
        drop(vault);
        let mut reopened = UnlockedVault::open(path, PASSWORD, false).unwrap();
        let again = reopened
            .update(|data| data.process("John Smith".into(), john(), "pseudonymize"))
            .unwrap();
        assert_eq!(again.text, record.text);
        assert_eq!(reopened.data.restore(&record.id).unwrap(), "John Smith");
        assert_eq!(reopened.data.pseudonyms.len(), 1);
    }

    #[test]
    fn wrong_password_and_tampering_do_not_overwrite_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        drop(UnlockedVault::open(path.clone(), PASSWORD, true).unwrap());
        let original = std::fs::read(&path).unwrap();
        assert!(UnlockedVault::open(path.clone(), "wrong password", false).is_err());
        assert!(UnlockedVault::open(path.clone(), PASSWORD, true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut envelope: Envelope = serde_json::from_slice(&original).unwrap();
        envelope.ciphertext[0] ^= 1;
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(UnlockedVault::open(path, PASSWORD, false).is_err());
    }

    #[test]
    fn simultaneous_open_is_blocked_and_failed_save_preserves_memory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let mut vault = UnlockedVault::open(path.clone(), PASSWORD, true).unwrap();
        assert!(UnlockedVault::open(path.clone(), PASSWORD, false).is_err());
        // A directory at the destination makes atomic replacement fail.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            vault
                .update(|data| data.process("John Smith".into(), john(), "pseudonymize"))
                .is_err()
        );
        assert!(vault.data.pseudonyms.is_empty());
        assert!(vault.data.records.is_empty());
    }

    #[test]
    fn history_clear_keeps_identity_mapping_after_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let mut vault = UnlockedVault::open(path.clone(), PASSWORD, true).unwrap();
        vault
            .update(|data| data.process("John Smith".into(), john(), "pseudonymize"))
            .unwrap();
        vault
            .update(|data| {
                data.records.clear();
                Ok(())
            })
            .unwrap();
        drop(vault);
        let mut vault = UnlockedVault::open(path, PASSWORD, false).unwrap();
        assert!(vault.data.records.is_empty());
        let record = vault
            .update(|data| data.process("John Smith".into(), john(), "pseudonymize"))
            .unwrap();
        assert_eq!(record.text, "PERSON_001");
    }
}

#[cfg(all(test, windows))]
mod account_tests {
    use super::*;
    use crate::review::ReviewedEntity;

    #[test]
    fn account_reopen_password_switch_and_reset_preserve_expected_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let mut vault = UnlockedVault::open(path.clone(), "first long passphrase", true).unwrap();
        let result = vault
            .update(|data| {
                data.process(
                    "John Smith".into(),
                    vec![ReviewedEntity {
                        id: "1".into(),
                        entity_type: "PERSON".into(),
                        text: "John Smith".into(),
                        start: 0,
                        end: 10,
                    }],
                    "pseudonymize",
                )
            })
            .unwrap();
        let vault = vault.migrate_to_account().unwrap();
        drop(vault);
        assert!(!UnlockedVault::needs_passphrase_migration(&path).unwrap());
        let mut vault = UnlockedVault::open_account(path.clone(), false).unwrap();
        assert_eq!(vault.data.restore(&result.id).unwrap(), "John Smith");
        vault
            .change_protection(false, "second long passphrase")
            .unwrap();
        drop(vault);
        assert!(UnlockedVault::open_account(path.clone(), false).is_err());
        assert!(UnlockedVault::open(path.clone(), "first long passphrase", false).is_err());
        let vault = UnlockedVault::open(path.clone(), "second long passphrase", false).unwrap();
        assert_eq!(vault.data.restore(&result.id).unwrap(), "John Smith");
        drop(vault);
        let reset = UnlockedVault::open_account(path.clone(), true).unwrap();
        assert!(reset.data.records.is_empty());
        assert!(reset.data.pseudonyms.is_empty());
        drop(reset);
        let reopened = UnlockedVault::open_account(path, false).unwrap();
        assert!(reopened.data.restore(&result.id).is_err());
    }
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod system_auth_storage_tests {
    use super::*;

    #[test]
    fn authenticated_account_vault_preserves_policy_after_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("system.vault");
        let vault = UnlockedVault::open_account_with_auth(path.clone(), false, true).unwrap();
        assert!(vault.authentication_required);
        assert_eq!(read_envelope(&path).unwrap().version, 3);
        drop(vault);
        assert!(UnlockedVault::requires_authentication(&path).unwrap());
        let vault = UnlockedVault::open_account(path, false).unwrap();
        assert!(vault.authentication_required);
    }

    #[test]
    fn editing_format_cannot_disable_authenticated_policy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("system.vault");
        drop(UnlockedVault::open_account_with_auth(path.clone(), false, true).unwrap());
        let mut envelope = read_envelope(&path).unwrap();
        envelope.version = 2;
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(UnlockedVault::open_account(path, false).is_err());
    }

    #[test]
    fn protection_changes_preserve_identity_and_authentication_policy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("system.vault");
        let mut vault = UnlockedVault::open_account(path.clone(), false).unwrap();
        let identity = vault
            .update(|data| data.save_identity("PERSON", "John Smith", ""))
            .unwrap();
        vault.change_protection_with_auth(true, "", true).unwrap();
        assert!(vault.authentication_required);
        assert!(
            vault
                .change_protection_with_auth(false, "a long test password", true)
                .is_err()
        );
        assert!(vault.authentication_required);
        vault
            .change_protection_with_auth(false, "a long test password", false)
            .unwrap();
        assert!(!vault.authentication_required);
        drop(vault);
        let vault = UnlockedVault::open(path, "a long test password", false).unwrap();
        assert_eq!(vault.data.pseudonyms[0].pseudonym, identity.pseudonym);
        assert!(!vault.authentication_required);
    }
}
