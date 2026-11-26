use super::Result;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload, rand_core::RngCore},
};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(super) const MAGIC: &[u8] = b"VEIL-SQLCIPHER-1\n";
pub(super) const MAX_BOOTSTRAP: u64 = 8192;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Passphrase,
    Account,
    System,
}

impl Mode {
    pub fn requires_authentication(self) -> bool {
        self == Self::System
    }

    pub fn account_supported() -> bool {
        cfg!(any(windows, target_os = "macos"))
    }

    pub fn from_options(use_account: bool, require_authentication: bool) -> Result<Self> {
        match (use_account, require_authentication) {
            (false, false) => Ok(Self::Passphrase),
            (false, true) => Err("System sign-in requires account protection".into()),
            (true, required) if Self::account_supported() => Ok(if required {
                Self::System
            } else {
                Self::Account
            }),
            _ => Err("Account protection requires Windows or macOS".into()),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Bootstrap {
    pub generation: Uuid,
    pub(super) protection: Protection,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Protection {
    Passphrase {
        kdf_version: u32,
        salt: [u8; 16],
        nonce: [u8; 12],
        wrapped: Vec<u8>,
    },
    Account {
        platform: String,
        authentication_required: bool,
        wrapped: Vec<u8>,
    },
}

impl Bootstrap {
    pub fn mode(&self) -> Mode {
        match self.protection {
            Protection::Passphrase { .. } => Mode::Passphrase,
            Protection::Account {
                authentication_required: false,
                ..
            } => Mode::Account,
            Protection::Account {
                authentication_required: true,
                ..
            } => Mode::System,
        }
    }

    pub fn wrap(generation: Uuid, key: &[u8; 32], mode: Mode, password: &str) -> Result<Self> {
        let binding = binding(generation, mode);
        let protection = if mode == Mode::Passphrase {
            if password.chars().count() < 12 {
                return Err("Use a vault passphrase with at least 12 characters".into());
            }
            let mut salt = [0u8; 16];
            OsRng.fill_bytes(&mut salt);
            let wrapping_key = derive(password, &salt, 1)?;
            let cipher =
                Aes256Gcm::new_from_slice(&*wrapping_key).map_err(|_| "Invalid wrapping key")?;
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let wrapped = cipher
                .encrypt(
                    &nonce,
                    Payload {
                        msg: key,
                        aad: &binding,
                    },
                )
                .map_err(|_| "Unable to protect the database key")?;
            Protection::Passphrase {
                kdf_version: 1,
                salt,
                nonce: nonce.into(),
                wrapped,
            }
        } else {
            let mut payload = Zeroizing::new(Vec::with_capacity(binding.len() + key.len()));
            payload.extend_from_slice(&binding);
            payload.extend_from_slice(key);
            Protection::Account {
                platform: std::env::consts::OS.into(),
                authentication_required: mode.requires_authentication(),
                wrapped: crate::account_key::protect(&payload)?,
            }
        };
        Ok(Self {
            generation,
            protection,
        })
    }

    pub fn unwrap(&self, password: &str) -> Result<Zeroizing<[u8; 32]>> {
        let binding = binding(self.generation, self.mode());
        let raw = match &self.protection {
            Protection::Passphrase {
                kdf_version,
                salt,
                nonce,
                wrapped,
            } => {
                let wrapping_key = derive(password, salt, *kdf_version)?;
                let cipher = Aes256Gcm::new_from_slice(&*wrapping_key)
                    .map_err(|_| "Invalid wrapping key")?;
                Zeroizing::new(
                    cipher
                        .decrypt(
                            Nonce::from_slice(nonce),
                            Payload {
                                msg: wrapped,
                                aad: &binding,
                            },
                        )
                        .map_err(|_| "Incorrect passphrase or damaged Vault bootstrap")?,
                )
            }
            Protection::Account {
                platform, wrapped, ..
            } => {
                if platform != std::env::consts::OS {
                    return Err(
                        "This Vault uses another operating system's account protection".into(),
                    );
                }
                let protected = Zeroizing::new(crate::account_key::unprotect(wrapped)?);
                let tail = protected
                    .strip_prefix(binding.as_slice())
                    .ok_or("Invalid Vault account-key binding")?;
                return fixed_key(tail);
            }
        };
        fixed_key(&raw)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = MAGIC.to_vec();
        serde_json::to_writer(&mut bytes, self)
            .map_err(|_| "Unable to encode the Vault bootstrap")?;
        if bytes.len() as u64 > MAX_BOOTSTRAP {
            return Err("The Vault bootstrap exceeds its size limit".into());
        }
        Ok(bytes)
    }
}

fn fixed_key(bytes: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    if bytes.len() != key.len() {
        return Err("Invalid Vault database key length".into());
    }
    key.copy_from_slice(bytes);
    Ok(key)
}

fn binding(generation: Uuid, mode: Mode) -> Vec<u8> {
    let mut binding = b"Veil SQLCipher database key v1\0".to_vec();
    binding.extend_from_slice(generation.as_bytes());
    binding.push(match mode {
        Mode::Passphrase => 1,
        Mode::Account => 2,
        Mode::System => 3,
    });
    binding
}

fn derive(password: &str, salt: &[u8; 16], version: u32) -> Result<Zeroizing<[u8; 32]>> {
    if version != 1 {
        return Err("Unsupported Vault passphrase configuration".into());
    }
    let params = Params::new(65_536, 3, 1, Some(32))
        .map_err(|_| "Invalid Vault passphrase configuration")?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| "Unable to derive the Vault wrapping key")?;
    Ok(key)
}
