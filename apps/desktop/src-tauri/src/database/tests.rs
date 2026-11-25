use super::*;
use crate::{
    protection::{self, DetectionRule},
    review::ReviewedEntity,
    vault_storage::UnlockedVault,
};
use rusqlite::Connection;
use std::io::{Seek, SeekFrom};

const PASSPHRASE: &str = "synthetic test passphrase";
const CHANGED_PASSPHRASE: &str = "different synthetic passphrase";
const CANARY: &str = "SYNTHETIC_SQLCIPHER_PRIVATE_2026";

fn fresh(path: &Path) -> Vault {
    Vault::open(
        path.into(),
        inspect(path).unwrap(),
        PASSPHRASE,
        true,
        Mode::Passphrase,
    )
    .unwrap()
}

fn reopen(path: &Path, password: &str) -> Result<Vault> {
    Vault::open(
        path.into(),
        inspect(path)?,
        password,
        false,
        Mode::Passphrase,
    )
}

fn entity(id: &str, value: &str, start: usize, category: &str) -> ReviewedEntity {
    ReviewedEntity {
        id: id.into(),
        entity_type: category.into(),
        text: value.into(),
        start,
        end: start + value.chars().count(),
    }
}

fn process(
    vault: &mut Vault,
    project: &str,
    text: &str,
    entities: Vec<ReviewedEntity>,
) -> Result<protection::ProcessedRecord> {
    let approved = protection::approved_detections(text, entities, "pseudonymize")?;
    vault.repo.process(project, text.into(), approved, None)
}

fn has_canary(bytes: &[u8]) -> bool {
    bytes
        .windows(CANARY.len())
        .any(|window| window == CANARY.as_bytes())
}

#[test]
fn cipher_only_round_trip_and_private_journal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let mut vault = fresh(&path);
    let conn = &mut vault.repo.0;
    let cipher: String = conn
        .pragma_query_value(None, "cipher_version", |r| r.get(0))
        .unwrap();
    assert!(cipher.starts_with("4."));
    let identity = vault
        .repo
        .save_mapping("default", "PERSON", CANARY)
        .unwrap();
    let generation = vault.bootstrap.generation;
    let db = database_path(&path, generation).unwrap();
    let journal = db.with_extension("db-journal");
    {
        let tx = vault.repo.0.transaction().unwrap();
        tx.execute("DELETE FROM pseudonym_mappings", []).unwrap();
        assert!(!has_canary(&fs::read(&journal).unwrap()));
        tx.rollback().unwrap();
    }
    assert!(!journal.exists());
    assert!(!db.with_extension("db-wal").exists());
    assert!(!db.with_extension("db-shm").exists());
    let file = fs::read(&db).unwrap();
    assert!(!file.starts_with(b"SQLite format 3\0"));
    assert!(!has_canary(&file));
    assert!(!has_canary(&fs::read(&path).unwrap()));
    assert!(reopen(&path, PASSPHRASE).is_err());
    assert!(
        Connection::open(&db)
            .unwrap()
            .query_row("SELECT * FROM projects", [], |_| Ok(()))
            .is_err()
    );
    assert!(connection::open(&db, generation, &[0; 32], false).is_err());
    drop(vault);
    assert!(reopen(&path, "incorrect synthetic password").is_err());
    let vault = reopen(&path, PASSPHRASE).unwrap();
    assert_eq!(
        vault
            .repo
            .mapping("default", &identity.id)
            .unwrap()
            .original,
        CANARY
    );
    connection::verify(&vault.repo.0).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&db).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(
            fs::metadata(db.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}

#[test]
fn tampering_and_missing_bootstrap_are_never_reinitialized() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let vault = fresh(&path);
    let db = database_path(&path, vault.bootstrap.generation).unwrap();
    let before = fs::read(&path).unwrap();
    drop(vault);
    let mut file = fs::OpenOptions::new().write(true).open(&db).unwrap();
    file.seek(SeekFrom::Start(64)).unwrap();
    file.write_all(&[!fs::read(&db).unwrap()[64]]).unwrap();
    drop(file);
    let damaged = fs::read(&db).unwrap();
    assert!(reopen(&path, PASSPHRASE).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(&db).unwrap(), damaged);
    fs::remove_file(&path).unwrap();
    assert!(inspect(&path).is_err());
    assert_eq!(fs::read(&db).unwrap(), damaged);
}

#[test]
fn repository_ordering_constraints_scopes_and_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let mut vault = fresh(&temp.path().join("identities.vault"));
    let first = vault.repo.create_project(" Étude ".into()).unwrap();
    assert!(vault.repo.create_project("ÉTUDE".into()).is_err());
    let second = vault.repo.create_project("العربية".into()).unwrap();
    let rules = vec![
        DetectionRule {
            phrase: "سري".into(),
            entity_type: "INDIRECT_IDENTIFIER".into()
        };
        2
    ];
    vault.repo.replace_rules(&first, &rules).unwrap();
    assert_eq!(vault.repo.rules(&first).unwrap().len(), 2);
    assert!(vault.repo.rules(&second).unwrap().is_empty());

    let original = "  سارة    Quill ";
    let a = vault.repo.save_mapping(&first, "PERSON", original).unwrap();
    let same = vault
        .repo
        .save_mapping(&first, "PERSON", "سارة quill")
        .unwrap();
    let other = vault
        .repo
        .save_mapping(&second, "PERSON", "سارة quill")
        .unwrap();
    assert_eq!(a.id, same.id);
    assert_eq!(same.original, original.trim());
    assert_eq!(a.pseudonym, "PERSON_001");
    assert_eq!(other.pseudonym, "PERSON_002");
    let upper = vault
        .repo
        .save_mapping(&first, "EMAIL_ADDRESS", "A@example.test")
        .unwrap();
    let lower = vault
        .repo
        .save_mapping(&first, "EMAIL_ADDRESS", "a@example.test")
        .unwrap();
    assert_ne!(upper.id, lower.id);
    let source = "😀 سارة quill  ";
    let record = process(
        &mut vault,
        &first,
        source,
        vec![entity("first", "سارة quill", 2, "PERSON")],
    )
    .unwrap();
    let next = process(
        &mut vault,
        &first,
        "Avery met PERSON_003",
        vec![entity("next", "Avery", 0, "PERSON")],
    )
    .unwrap();
    assert_eq!(next.text, "PERSON_004 met PERSON_003");
    assert_eq!(vault.repo.records(&first).unwrap()[0].id, next.id);
    assert_eq!(vault.repo.restore(&first, &record.id).unwrap(), source);
    assert!(vault.repo.restore(&second, &record.id).is_err());
    assert!(vault.repo.delete_mapping(&second, &a.id).is_err());
    vault.repo.delete_mapping(&first, &a.id).unwrap();
    assert_eq!(vault.repo.restore(&first, &record.id).unwrap(), source);

    let conn = &vault.repo.0;
    assert!(
        conn.execute("DELETE FROM projects WHERE project_id='default'", [])
            .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE projects SET name='changed' WHERE project_id='default'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT OR REPLACE INTO projects VALUES ('default','General','general',0)",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO projects VALUES ('default','Other','other',10)",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO detection_rules VALUES ('missing',0,'bad','PERSON')",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO detection_rules VALUES ('default',0,'bad','MISSING')",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO pseudonym_mappings VALUES ('new','default','PERSON','other','other',?1,0)",
            [&other.pseudonym]
        )
        .is_err()
    );
    assert!(conn.execute("INSERT INTO pseudonym_mappings VALUES ('new',?1,'PERSON','duplicate','سارة quill','PERSON_100',1)", [&second]).is_err());

    vault.repo.delete_project(&first, "Étude").unwrap();
    for table in [
        "pseudonym_mappings",
        "processing_history",
        "detection_rules",
    ] {
        // Fixed table names above, never command inputs.
        let sql = format!("SELECT count(*) FROM {table} WHERE project_id=?1");
        assert_eq!(
            vault
                .repo
                .0
                .query_row(&sql, [&first], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        vault
            .repo
            .save_mapping("default", "PERSON", "Avery")
            .unwrap()
            .pseudonym,
        "PERSON_005"
    );
    assert_eq!(vault.repo.projects().unwrap()[1].id, second);
    vault.repo.reset().unwrap();
    assert_eq!(vault.repo.projects().unwrap().len(), 1);
    assert_eq!(
        vault
            .repo
            .save_mapping("default", "PERSON", "Avery")
            .unwrap()
            .pseudonym,
        "PERSON_001"
    );
}

#[test]
fn failed_protection_rolls_back_mappings_counters_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let mut vault = fresh(&temp.path().join("identities.vault"));
    assert!(
        process(
            &mut vault,
            "default",
            "Avery ",
            vec![
                entity("a", "Avery", 0, "PERSON"),
                entity("b", " ", 5, "PERSON"),
            ]
        )
        .is_err()
    );
    assert!(vault.repo.mappings("default").unwrap().is_empty());
    assert!(vault.repo.records("default").unwrap().is_empty());
    assert_eq!(
        vault
            .repo
            .0
            .query_row("SELECT count(*) FROM pseudonym_counters", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let record = process(
        &mut vault,
        "default",
        "Avery",
        vec![entity("a", "Avery", 0, "PERSON")],
    )
    .unwrap();
    assert_eq!(record.text, "PERSON_001");
    vault.repo.clear_history("default").unwrap();
    assert!(vault.repo.restore("default", &record.id).is_err());
    assert_eq!(vault.repo.mappings("default").unwrap().len(), 1);
}

#[test]
fn serialized_concurrent_requests_reuse_one_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let vault = std::sync::Mutex::new(fresh(&temp.path().join("identities.vault")));
    std::thread::scope(|threads| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                threads.spawn(|| {
                    vault
                        .lock()
                        .unwrap()
                        .repo
                        .save_mapping("default", "PERSON", "Same Synthetic Name")
                        .unwrap()
                })
            })
            .collect();
        let identities: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(
            identities
                .iter()
                .all(|item| item.id == identities[0].id && item.pseudonym == "PERSON_001")
        );
    });
}

#[test]
fn passphrase_change_keeps_data_and_rejects_old_metadata_tampering() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let mut vault = fresh(&path);
    let id = vault
        .repo
        .save_mapping("default", "PERSON", "Synthetic")
        .unwrap()
        .id;
    let before = fs::read(&path).unwrap();
    assert!(vault.change_protection(Mode::Passphrase, "short").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let generation = vault.bootstrap.generation;
    vault
        .change_protection(Mode::Passphrase, CHANGED_PASSPHRASE)
        .unwrap();
    assert_eq!(vault.bootstrap.generation, generation);
    drop(vault);
    assert!(reopen(&path, PASSPHRASE).is_err());
    let mut vault = reopen(&path, CHANGED_PASSPHRASE).unwrap();
    assert_eq!(
        vault.repo.mapping("default", &id).unwrap().original,
        "Synthetic"
    );
    vault.bootstrap.generation = Uuid::new_v4();
    assert!(vault.bootstrap.unwrap(CHANGED_PASSPHRASE).is_err());
}

#[test]
fn legacy_migration_reopens_exact_data_and_preserves_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let mut old = UnlockedVault::open(path.clone(), PASSPHRASE, true).unwrap();
    let (project, mapping_id, record_id, absent_id) = old
        .update(|data| {
            let project = data.create_project("Synthetic Project".into())?;
            let mut child = SessionVault::default();
            let source = "سارة met PERSON_001.\n ";
            let record = child.process(
                source.into(),
                vec![entity("a", "سارة", 0, "PERSON")],
                "pseudonymize",
            )?;
            let mapping = child.pseudonyms[0].id.clone();
            child.rules = vec![
                DetectionRule {
                    phrase: "معلومات".into(),
                    entity_type: "INDIRECT_IDENTIFIER".into()
                };
                2
            ];
            let absent_id = Uuid::new_v4().to_string();
            child.records.push(
                serde_json::from_value(serde_json::json!({
                    "public": {"id":absent_id,"mode":"anonymize","text":"LEGACY_001","count":1}
                }))
                .unwrap(),
            );
            child.counters.insert("PERSON".into(), 1);
            data.projects[0].data = child;
            data.counters.insert("PERSON".into(), 50);
            Ok((project, mapping, record.id, absent_id))
        })
        .unwrap();
    drop(old);
    let encrypted = fs::read(&path).unwrap();
    assert!(reopen(&path, "wrong synthetic password").is_err());
    assert_eq!(fs::read(&path).unwrap(), encrypted);
    // Simulate interruption after a recovery copy and private generation were staged.
    preserve_legacy(&path).unwrap();
    let root = generation_root(&path).unwrap();
    create_private_directory(&root, false).unwrap();
    create_private_directory(&root.join(Uuid::new_v4().to_string()), false).unwrap();
    let mut vault = reopen(&path, PASSPHRASE).unwrap();
    assert_eq!(
        vault.repo.mapping(&project, &mapping_id).unwrap().pseudonym,
        "PERSON_002"
    );
    assert_eq!(
        vault.repo.restore(&project, &record_id).unwrap(),
        "سارة met PERSON_001.\n "
    );
    assert!(vault.repo.restore(&project, &absent_id).is_err());
    assert_eq!(vault.repo.rules(&project).unwrap().len(), 2);
    assert_eq!(
        vault
            .repo
            .save_mapping("default", "PERSON", "Synthetic Later")
            .unwrap()
            .pseudonym,
        "PERSON_051"
    );
    assert_eq!(
        fs::read(path.with_extension("vault.legacy-recovery")).unwrap(),
        encrypted
    );
    assert!(!inspect(&path).unwrap().needs_migration());
    let generation = vault.bootstrap.generation;
    drop(vault);
    assert_eq!(
        reopen(&path, PASSPHRASE).unwrap().bootstrap.generation,
        generation
    );
}

#[test]
fn migration_duplicate_and_publication_failure_preserve_old_vault() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let mut old = UnlockedVault::open(path.clone(), PASSPHRASE, true).unwrap();
    old.update(|data| {
        let first = data.save_identity("PERSON", "Synthetic", "")?;
        let mut duplicate = first;
        duplicate.id = Uuid::new_v4().to_string();
        data.pseudonyms.push(duplicate);
        Ok(())
    })
    .unwrap();
    drop(old);
    let before = fs::read(&path).unwrap();
    assert!(
        matches!(reopen(&path, PASSPHRASE), Err(error) if error.contains("migration invariant"))
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let blocked = temp.path().join("blocked");
    fs::create_dir(&blocked).unwrap();
    let bootstrap =
        Bootstrap::wrap(Uuid::new_v4(), &[7; 32], Mode::Passphrase, PASSPHRASE).unwrap();
    assert!(publish_bootstrap(&blocked, &bootstrap).is_err());
    assert!(blocked.is_dir());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn category_catalog_matches_python_typescript_and_sql() {
    let temp = tempfile::tempdir().unwrap();
    let vault = fresh(&temp.path().join("identities.vault"));
    let mut stmt = vault
        .repo
        .0
        .prepare("SELECT code,token_prefix FROM pii_categories")
        .unwrap();
    let sql = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
        .unwrap();
    let expected: std::collections::HashMap<_, _> = protection::CATEGORY_PREFIXES
        .iter()
        .map(|(code, prefix)| (code.to_string(), prefix.to_string()))
        .collect();
    assert_eq!(sql, expected);
    let python = include_str!("../../../../../engines/python/veil_engine/detector.py");
    let block = python
        .split("SUPPORTED_ENTITIES = {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    let python_codes: std::collections::HashSet<_> = block.split('"').skip(1).step_by(2).collect();
    let typescript = include_str!("../../../src/App.tsx");
    let block = typescript
        .split("const PRIVACY_CATEGORIES")
        .nth(1)
        .unwrap()
        .split("];")
        .next()
        .unwrap();
    let ts_codes: std::collections::HashSet<_> = block
        .split("value: \"")
        .skip(1)
        .map(|fragment| fragment.split('"').next().unwrap())
        .collect();
    let rust_codes: std::collections::HashSet<_> = protection::CATEGORY_PREFIXES
        .iter()
        .map(|(code, _)| *code)
        .collect();
    assert_eq!(rust_codes, python_codes);
    assert_eq!(rust_codes, ts_codes);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn legacy_account_and_system_formats_and_binding() {
    let temp = tempfile::tempdir().unwrap();
    for required in [false, true] {
        let path = temp.path().join(format!("{required}.vault"));
        let mut old = UnlockedVault::open_account_with_auth(path.clone(), false, required).unwrap();
        old.update(|data| data.save_identity("PERSON", "Synthetic account fixture", ""))
            .unwrap();
        drop(old);
        #[cfg(windows)]
        if !required {
            let mut envelope: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let fields = envelope.as_object_mut().unwrap();
            let wrapped = fields.remove("account_key").unwrap();
            fields.insert("windows_key".into(), wrapped);
            fields.remove("account_platform");
            fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        }
        let mode = if required {
            Mode::System
        } else {
            Mode::Account
        };
        let snapshot = inspect(&path).unwrap();
        assert!(snapshot.mode() == Some(mode));
        // Native dialog is tested at the command/platform boundary. Here we
        // exercise the authenticated reader/wrapper after the dialog succeeds.
        let mut vault = Vault::open(path.clone(), snapshot, "", false, mode).unwrap();
        assert_eq!(vault.repo.mappings("default").unwrap().len(), 1);
        assert!(vault.mode() == mode);
        if let bootstrap::Protection::Account {
            authentication_required,
            ..
        } = &mut vault.bootstrap.protection
        {
            *authentication_required = !required;
        }
        assert!(vault.bootstrap.unwrap("").is_err());
    }
}

#[test]
fn request_id_retries_and_history_capacity_leave_counters_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let mut vault = fresh(&temp.path().join("identities.vault"));
    let request = Uuid::new_v4();
    let submit = |vault: &mut Vault| {
        let spans = protection::approved_detections(
            "Avery",
            vec![entity("a", "Avery", 0, "PERSON")],
            "pseudonymize",
        )
        .unwrap();
        vault
            .repo
            .process("default", "Avery".into(), spans, Some(request))
    };
    assert_eq!(submit(&mut vault).unwrap().id, request.to_string());
    assert!(matches!(submit(&mut vault), Err(error) if error.contains("already saved")));
    vault.repo.0.execute_batch("WITH RECURSIVE slots(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM slots WHERE i<99)
        INSERT INTO processing_history SELECT 'synthetic-'||i,'default','pseudonymize','synthetic',NULL,1,i FROM slots;").unwrap();
    assert!(
        process(
            &mut vault,
            "default",
            "Different",
            vec![entity("a", "Different", 0, "PERSON")]
        )
        .is_err()
    );
    assert_eq!(vault.repo.mappings("default").unwrap().len(), 1);
    assert_eq!(vault.repo.records("default").unwrap().len(), 100);
    assert_eq!(
        vault
            .repo
            .save_mapping("default", "PERSON", "Later")
            .unwrap()
            .pseudonym,
        "PERSON_002"
    );
}

#[test]
fn future_schema_and_temporary_materialization_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identities.vault");
    let vault = fresh(&path);
    let conn = &vault.repo.0;
    conn.execute_batch("CREATE TEMP TABLE materialized(text TEXT);")
        .unwrap();
    conn.execute("INSERT INTO temp.materialized VALUES (?1)", [CANARY])
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT file FROM pragma_database_list WHERE name='temp'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        ""
    );
    conn.pragma_update(None, "user_version", 999).unwrap();
    drop(vault);
    assert!(matches!(reopen(&path, PASSPHRASE), Err(error) if error.contains("schema")));
}
