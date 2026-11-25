use super::{DbResult, Result};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;
use zeroize::Zeroizing;

const APPLICATION_ID: i64 = 0x5645494c;
const SCHEMA_VERSION: i64 = 1;

pub(super) fn open(
    path: &Path,
    generation: Uuid,
    key: &[u8; 32],
    create: bool,
) -> Result<Connection> {
    if create {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map_err(|_| "Unable to create the encrypted database")?;
    } else {
        let mut salt = [0u8; 16];
        File::open(path)
            .and_then(|mut file| file.read_exact(&mut salt))
            .map_err(|_| "The encrypted database is missing or damaged")?;
        if &salt != generation.as_bytes() {
            return Err("The encrypted database does not match the Vault bootstrap".into());
        }
    }

    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .vault()?;
    let cipher: String = connection
        .pragma_query_value(None, "cipher_version", |row| row.get(0))
        .vault()?;
    if !cipher.starts_with("4.") {
        return Err("The required SQLCipher library is unavailable".into());
    }
    apply_key(&connection, key, create.then_some(generation))?;
    let logging: String = connection
        .query_row("PRAGMA cipher_log_level=NONE", [], |r| r.get(0))
        .vault()?;
    if logging != "NONE" {
        return Err("Unable to disable SQLCipher diagnostics".into());
    }
    connection
        .execute_batch("PRAGMA cipher_compatibility=4; PRAGMA cipher_memory_security=ON;")
        .vault()?;
    connection
        .query_row("SELECT count(*) FROM sqlite_schema", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(|_| "The encrypted database could not be authenticated")?;
    connection.busy_timeout(Duration::from_secs(2)).vault()?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY;
        PRAGMA synchronous=EXTRA; PRAGMA secure_delete=ON; PRAGMA trusted_schema=OFF; PRAGMA recursive_triggers=ON;").vault()?;
    let journal = if create {
        connection
            .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))
            .vault()?
    } else {
        connection
            .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
            .vault()?
    };
    if journal != "delete" {
        return Err("The Vault database has an unsupported journal mode".into());
    }
    for (name, expected) in [
        ("foreign_keys", 1),
        ("temp_store", 2),
        ("synchronous", 3),
        ("secure_delete", 1),
        ("trusted_schema", 0),
        ("recursive_triggers", 1),
    ] {
        if connection
            .pragma_query_value(None, name, |r| r.get::<_, i64>(0))
            .vault()?
            != expected
        {
            return Err("Required Vault database settings are unavailable".into());
        }
    }
    let memory: String = connection
        .pragma_query_value(None, "cipher_memory_security", |r| r.get(0))
        .vault()?;
    if memory != "1" {
        return Err("SQLCipher memory security is unavailable".into());
    }
    migrate(&mut connection, create)?;
    Ok(connection)
}

fn apply_key(conn: &Connection, key: &[u8; 32], salt: Option<Uuid>) -> Result<()> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(99));
    encoded.extend_from_slice(b"x'");
    for byte in key.iter().chain(salt.iter().flat_map(|id| id.as_bytes())) {
        write!(&mut *encoded, "{byte:02x}").map_err(|_| "Unable to encode the database key")?;
    }
    encoded.push(b'\'');
    // SQLCipher parses its documented raw-key form from a counted buffer.
    // The live connection owns the native handle and retains no pointer to this buffer.
    let result = unsafe {
        rusqlite::ffi::sqlite3_key(conn.handle(), encoded.as_ptr().cast(), encoded.len() as i32)
    };
    if result != rusqlite::ffi::SQLITE_OK {
        return Err("Unable to apply the Vault database key".into());
    }
    Ok(())
}

fn migrate(conn: &mut Connection, create: bool) -> Result<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .vault()?;
    let application: i64 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .vault()?;
    match (create, version, application) {
        (true, 0, 0) => {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .vault()?;
            tx.execute_batch(include_str!("migrations/001_initial.sql"))
                .vault()?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)
                .vault()?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .vault()?;
            tx.commit().vault()?;
        }
        (false, SCHEMA_VERSION, APPLICATION_ID) => {}
        _ => return Err("The Vault database schema is damaged or unsupported".into()),
    }
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN
          ('projects','pii_categories','pseudonym_mappings','processing_history','detection_rules','pseudonym_counters')",
        [], |r| r.get(0)).vault()?;
    let general: i64 = conn.query_row(
        "SELECT count(*) FROM projects WHERE project_id='default' AND position=0 AND name='General' AND name_key='general'",
        [], |r| r.get(0)).vault()?;
    if tables != 6 || general != 1 {
        return Err("The Vault database schema is incomplete".into());
    }
    Ok(())
}

pub(super) fn verify(conn: &Connection) -> Result<()> {
    let integrity: String = conn
        .pragma_query_value(None, "integrity_check", |r| r.get(0))
        .vault()?;
    let mut foreign = conn.prepare("PRAGMA foreign_key_check").vault()?;
    let mut cipher = conn.prepare("PRAGMA cipher_integrity_check").vault()?;
    if integrity != "ok"
        || foreign.query([]).vault()?.next().vault()?.is_some()
        || cipher.query([]).vault()?.next().vault()?.is_some()
    {
        return Err("The encrypted database failed its integrity checks".into());
    }
    Ok(())
}
