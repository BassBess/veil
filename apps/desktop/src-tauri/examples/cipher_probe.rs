use aes_gcm::aead::{OsRng, rand_core::RngCore};
use rusqlite::Connection;
use std::io::Write;
use zeroize::Zeroizing;

fn key(conn: &Connection, bytes: &[u8; 32]) {
    let mut value = Zeroizing::new(b"x'".to_vec());
    for byte in bytes {
        write!(&mut *value, "{byte:02x}").unwrap();
    }
    value.push(b'\'');
    let result = unsafe {
        rusqlite::ffi::sqlite3_key(conn.handle(), value.as_ptr().cast(), value.len() as i32)
    };
    assert_eq!(result, rusqlite::ffi::SQLITE_OK);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Provide a new synthetic output path")?,
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let canary = b"SYNTHETIC_PRIVATE_SQLCIPHER_CANARY";
    let mut secret = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut *secret);
    let mut conn = Connection::open(&path)?;
    key(&conn, &secret);
    let cipher: String = conn.query_row("PRAGMA cipher_version", [], |row| row.get(0))?;
    assert!(cipher.starts_with("4."));
    let provider: String = conn.query_row("PRAGMA cipher_provider", [], |row| row.get(0))?;
    let provider_version: String =
        conn.query_row("PRAGMA cipher_provider_version", [], |row| row.get(0))?;
    println!(
        "SQLCipher {cipher}, SQLite {}, provider {provider} {provider_version}",
        rusqlite::version()
    );
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY; PRAGMA synchronous=EXTRA;
        CREATE TABLE project(id INTEGER PRIMARY KEY) STRICT;
        CREATE TABLE probe(project INTEGER REFERENCES project(id), text TEXT) STRICT;
        INSERT INTO project VALUES (1);",
    )?;
    for (name, expected) in [("foreign_keys", 1), ("temp_store", 2), ("synchronous", 3)] {
        assert_eq!(
            conn.pragma_query_value(None, name, |row| row.get::<_, i64>(0))?,
            expected
        );
    }
    assert_eq!(
        conn.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
        "delete"
    );
    assert!(
        conn.execute("INSERT INTO probe VALUES (2,'invalid')", [])
            .is_err()
    );
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO probe VALUES (1,'SYNTHETIC_PRIVATE_SQLCIPHER_CANARY')",
        [],
    )?;
    tx.commit()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM probe", [])?;
    let journal = std::path::PathBuf::from(&path).with_extension("db-journal");
    let bytes = std::fs::read(journal)?;
    assert!(!bytes.windows(canary.len()).any(|w| w == canary));
    tx.rollback()?;
    drop(conn);
    let bytes = std::fs::read(&path)?;
    assert!(!bytes.starts_with(b"SQLite format 3\0"));
    assert!(!bytes.windows(canary.len()).any(|w| w == canary));
    let read =
        |conn: &Connection| conn.query_row("SELECT text FROM probe", [], |r| r.get::<_, String>(0));
    let conn = Connection::open(&path)?;
    assert!(read(&conn).is_err());
    let conn = Connection::open(&path)?;
    key(&conn, &[0; 32]);
    assert!(read(&conn).is_err());
    let conn = Connection::open(&path)?;
    key(&conn, &secret);
    assert_eq!(read(&conn)?.as_bytes(), canary);
    println!(
        "verified keyed reopen, wrong/unkeyed rejection, encrypted canaries, rollback, STRICT, FKs and memory temp store"
    );
    Ok(())
}
