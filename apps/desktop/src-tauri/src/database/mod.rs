#[cfg(test)]
mod benchmark;
mod bootstrap;
mod connection;
mod legacy_migration;
mod repository;
#[cfg(test)]
mod tests;

use crate::protection::SessionVault;
use crate::vault_storage;
use aes_gcm::aead::{OsRng, rand_core::RngCore};
pub use bootstrap::Mode;
use bootstrap::{Bootstrap, MAGIC, MAX_BOOTSTRAP};
use repository::Repository;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, String>;

// Driver errors may contain SQL inputs. Only static diagnostics cross IPC.
trait DbResult<T> {
    fn vault(self) -> Result<T>;
}
impl<T> DbResult<T> for rusqlite::Result<T> {
    fn vault(self) -> Result<T> {
        self.map_err(|_| "The encrypted Vault database operation failed".into())
    }
}

fn write_integer(value: usize) -> Result<i64> {
    i64::try_from(value).map_err(|_| "Vault data exceeds the supported integer range".into())
}

fn read_integer(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    let value: i64 = row.get(column)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

#[derive(PartialEq, Eq)]
pub struct Snapshot(Source);

#[derive(PartialEq, Eq)]
enum Source {
    Absent,
    Legacy(vault_storage::LegacyHeader),
    Database(Bootstrap),
}

impl Snapshot {
    pub fn mode(&self) -> Option<Mode> {
        match &self.0 {
            Source::Absent => None,
            Source::Legacy(header) => Some(header.mode()),
            Source::Database(bootstrap) => Some(bootstrap.mode()),
        }
    }

    pub fn needs_migration(&self) -> bool {
        matches!(self.0, Source::Legacy(_))
    }
}

pub fn inspect(path: &Path) -> Result<Snapshot> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::read_dir(generation_root(path)?) {
                Ok(mut entries) => {
                    if entries.next().is_some() {
                        return Err("The Vault bootstrap is missing. Existing encrypted Vault data was preserved.".into());
                    }
                }
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err("Unable to inspect the encrypted Vault directory".into());
                }
                _ => {}
            }
            return Ok(Snapshot(Source::Absent));
        }
        Err(_) => return Err("Unable to read the Vault bootstrap".into()),
    };
    let mut prefix = [0; MAGIC.len()];
    match file.read_exact(&mut prefix) {
        Ok(()) if prefix == MAGIC => {
            let mut bytes = Vec::new();
            file.take(MAX_BOOTSTRAP + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Unable to read the Vault bootstrap")?;
            if bytes.len() as u64 + MAGIC.len() as u64 > MAX_BOOTSTRAP {
                return Err("The Vault bootstrap exceeds its size limit".into());
            }
            let bootstrap: Bootstrap = serde_json::from_slice(&bytes)
                .map_err(|_| "The Vault bootstrap is damaged or unsupported")?;
            Ok(Snapshot(Source::Database(bootstrap)))
        }
        Ok(()) | Err(_) => {
            vault_storage::legacy_header(path).map(|header| Snapshot(Source::Legacy(header)))
        }
    }
}

pub struct Vault {
    pub repo: Repository,
    key: Zeroizing<[u8; 32]>,
    bootstrap: Bootstrap,
    path: PathBuf,
    // Drop the repository and clear the key before releasing the legacy-compatible lock.
    _lock: File,
}

impl Vault {
    pub fn open(
        path: PathBuf,
        expected: Snapshot,
        password: &str,
        create: bool,
        new_mode: Mode,
    ) -> Result<Self> {
        let lock = vault_storage::acquire_lock(&path)?;
        let current = inspect(&path)?;
        if current != expected {
            return Err("The Vault changed. Start sign-in again.".into());
        }
        match current.0 {
            Source::Absent if create => Self::create(path, lock, password, new_mode, None),
            Source::Absent => Err("No saved Vault was found".into()),
            _ if create => Err("A Vault already exists. Unlock the existing Vault.".into()),
            Source::Legacy(header) => {
                let data = vault_storage::read_legacy(&path, &header, password)?;
                Self::create(path, lock, password, header.mode(), Some(&data))
            }
            Source::Database(bootstrap) => {
                let key = bootstrap.unwrap(password)?;
                let repo = Repository(connection::open(
                    &database_path(&path, bootstrap.generation)?,
                    bootstrap.generation,
                    &key,
                    false,
                )?);
                Ok(Self {
                    repo,
                    key,
                    bootstrap,
                    path,
                    _lock: lock,
                })
            }
        }
    }

    pub fn mode(&self) -> Mode {
        self.bootstrap.mode()
    }

    pub fn change_protection(&mut self, mode: Mode, password: &str) -> Result<()> {
        let generation = self.bootstrap.generation;
        let bootstrap = Bootstrap::wrap(generation, &self.key, mode, password)?;
        let verified = bootstrap.unwrap(password)?;
        let connection = connection::open(
            &database_path(&self.path, generation)?,
            generation,
            &verified,
            false,
        )?;
        drop(connection);
        publish_bootstrap(&self.path, &bootstrap)?;
        self.bootstrap = bootstrap;
        Ok(())
    }

    pub fn reset_locked(path: PathBuf) -> Result<Self> {
        if !Mode::account_supported() {
            return Err("Unlock your Vault before resetting it".into());
        }
        let lock = vault_storage::acquire_lock(&path)?;
        // No decryption is attempted during a locked reset. Preserve the old artifact
        // if it could be a legacy Vault. Existing DB generations become unreferenced.
        if path.is_file() && !has_magic(&path)? {
            preserve_legacy(&path)?;
        }
        Self::create(path, lock, "", Mode::Account, None)
    }

    fn create(
        path: PathBuf,
        lock: File,
        password: &str,
        mode: Mode,
        legacy: Option<&SessionVault>,
    ) -> Result<Self> {
        let generation = Uuid::new_v4();
        let root = generation_root(&path)?;
        create_private_directory(&root, true)?;
        let dir = root.join(generation.to_string());
        create_private_directory(&dir, false)?;
        let db = dir.join("vault.db");
        let result = (|| {
            let mut key = Zeroizing::new([0; 32]);
            OsRng.fill_bytes(&mut *key);
            let bootstrap = Bootstrap::wrap(generation, &key, mode, password)?;
            let mut conn = connection::open(&db, generation, &key, true)?;
            if let Some(data) = legacy {
                legacy_migration::import(&mut conn, data)?;
            }
            connection::verify(&conn)?;
            conn.close()
                .map_err(|_| "Unable to close the staged Vault")?;
            let reopened_key = bootstrap.unwrap(password)?;
            let conn = connection::open(&db, generation, &reopened_key, false)?;
            connection::verify(&conn)?;
            if let Some(data) = legacy {
                legacy_migration::verify(&conn, data)?;
                preserve_legacy(&path)?;
            }
            fs::OpenOptions::new()
                .write(true)
                .open(&db)
                .and_then(|file| file.sync_all())
                .map_err(|_| "Unable to sync the staged Vault database")?;
            sync_directory(&dir)?;
            sync_directory(&root)?;
            publish_bootstrap(&path, &bootstrap)?;
            Ok(Self {
                repo: Repository(conn),
                key,
                bootstrap,
                path,
                _lock: lock,
            })
        })();
        if result.is_err() {
            // This operation exclusively created the generation and did not publish it.
            let _ = fs::remove_dir_all(&dir);
        }
        result
    }
}

fn parent(path: &Path) -> Result<&Path> {
    path.parent().ok_or_else(|| "Invalid Vault location".into())
}

fn generation_root(path: &Path) -> Result<PathBuf> {
    Ok(parent(path)?.join("identities.sqlcipher"))
}

fn database_path(path: &Path, generation: Uuid) -> Result<PathBuf> {
    Ok(generation_root(path)?
        .join(generation.to_string())
        .join("vault.db"))
}

fn create_private_directory(path: &Path, allow_existing: bool) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error)
            if allow_existing && error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() =>
        {
            Ok(())
        }
        Err(_) => Err("Unable to create the encrypted Vault directory".into()),
    }
}

fn publish_bootstrap(path: &Path, bootstrap: &Bootstrap) -> Result<()> {
    let folder = parent(path)?;
    let mut pending = tempfile::NamedTempFile::new_in(folder)
        .map_err(|_| "Unable to prepare the Vault bootstrap")?;
    pending
        .write_all(&bootstrap.encode()?)
        .and_then(|_| pending.as_file().sync_all())
        .map_err(|_| "Unable to save the Vault bootstrap")?;
    sync_directory(folder)?;
    pending
        .persist(path)
        .map_err(|_| "Unable to publish the Vault bootstrap")?;
    // The rename is the commit point. A post-commit sync failure must not trigger
    // cleanup of the now-referenced database or revert in-memory policy.
    let _ = sync_directory(folder);
    Ok(())
}

fn has_magic(path: &Path) -> Result<bool> {
    let mut file = File::open(path).map_err(|_| "Unable to read the legacy Vault")?;
    let mut buffer = [0; MAGIC.len()];
    Ok(file.read_exact(&mut buffer).is_ok() && buffer == MAGIC)
}

fn preserve_legacy(path: &Path) -> Result<()> {
    let backup = path.with_extension("vault.legacy-recovery");
    if backup.exists() {
        if files_equal(path, &backup)? {
            return Ok(());
        }
        return Err(
            "An existing legacy recovery copy differs. Existing Vault data was preserved.".into(),
        );
    }
    let mut input = File::open(path).map_err(|_| "Unable to open legacy Vault recovery data")?;
    let mut pending = tempfile::NamedTempFile::new_in(parent(path)?)
        .map_err(|_| "Unable to prepare legacy Vault recovery data")?;
    io::copy(&mut input, &mut pending)
        .and_then(|_| pending.as_file().sync_all())
        .map_err(|_| "Unable to save legacy Vault recovery data")?;
    pending
        .persist_noclobber(&backup)
        .map_err(|_| "Unable to retain legacy Vault recovery data")?;
    sync_directory(parent(path)?)
}

fn files_equal(first: &Path, second: &Path) -> Result<bool> {
    let compare = || -> io::Result<bool> {
        let (mut a, mut b) = (File::open(first)?, File::open(second)?);
        let len = a.metadata()?.len();
        if len != b.metadata()?.len() {
            return Ok(false);
        }
        let (mut left, mut right) = ([0; 8192], [0; 8192]);
        let mut remaining = len;
        while remaining > 0 {
            let count = remaining.min(left.len() as u64) as usize;
            a.read_exact(&mut left[..count])?;
            b.read_exact(&mut right[..count])?;
            if left[..count] != right[..count] {
                return Ok(false);
            }
            remaining -= count as u64;
        }
        Ok(true)
    };
    compare().map_err(|_| "Unable to validate legacy Vault recovery data".into())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Unable to sync the Vault folder".into())
}
#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
